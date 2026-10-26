// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-25

package org.openxplay.stream.web

import java.net.Socket
import java.nio.file.Path
import kotlinx.serialization.json.*
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.io.TempDir
import org.openxplay.stream.core.*
import org.openxplay.stream.core.wire.*
import kotlin.test.*

class HostUiTest {
    @TempDir lateinit var root: Path
    private val host = object : OutputHost {
        override fun capabilities(capabilities: OutputCapabilities) = Unit
        override fun control(event: ControlEvent) = Unit
        override fun stats(stats: OutputStats) = Unit
    }

    private fun output(hostUi: Boolean = false, clock: MonotonicClock = ManualClock()): WebOutput = WebOutput(
        WebOutputConfig(port = 0, staticAssetRoot = root, sourceUrl = "https://example.org/source", hostUi = hostUi),
        clock = clock,
    )

    private fun connect(web: WebOutput): TestClient = TestClient(Socket("127.0.0.1", web.awaitListening()))

    private fun TestClient.expectHostUi(expected: Boolean) {
        send(1, "{\"type\":\"ping\",\"t0Us\":\"1\"}".toByteArray())
        if (expected) assertEquals(Json.parseToJsonElement(vector("host-ui")), json())
        assertEquals("pong", json()["type"]!!.jsonPrimitive.content)
    }

    private fun TestClient.touch(pressed: Boolean = true) {
        val contacts = listOf(TouchContact(0, 0.5, 0.25, pressed))
        send(2, OxpWebCodec.encode(BinaryMessage(BinaryHeader(BinaryKind.TOUCH, 0, 0, 0u, 0, 0), OxpWebCodec.encodeTouch(contacts))))
        expectHostUi(false)
    }

    @Test fun disabledByDefaultDoesNotAdvertiseOrSendRequests() {
        assertFalse(WebOutputConfig(staticAssetRoot = root, sourceUrl = "https://example.org/source").hostUi)
        val web = output()
        web.open(host)
        try {
            connect(web).use { client ->
                client.hello()
                assertNull(client.json()["features"])
                client.touch()
                web.onHostUiRequested()
                client.expectHostUi(false)
            }
        } finally { web.close() }
    }

    @Test fun hostUiFeatureCoexistsWithDisplaySettingsAndViewArea() {
        val state = SessionState(SessionId(1), SessionPhase.CONNECTING)
        for (enabled in listOf(false, true)) {
            val welcome = receiverWelcome("browser", state, "https://example.org/source",
                displaySettings = true, viewArea = true, hostUi = enabled)
            val expected = listOf("display-settings", "view-area") + if (enabled) listOf("host-ui") else emptyList()
            assertEquals(expected, welcome["features"]!!.jsonArray.map { it.jsonPrimitive.content })
            WebJsonCodec.validate(welcome, WebJsonCodec.Direction.RELAY_TO_CLIENT)
        }
    }

    @Test fun withoutTouchRequestsReachAllReadyClientsAndAreNotReplayed() {
        val web = output(hostUi = true)
        web.onHostUiRequested()
        web.open(host)
        try {
            web.onHostUiRequested()
            connect(web).use { first ->
                first.hello()
                assertEquals(listOf("host-ui"), first.json()["features"]!!.jsonArray.map { it.jsonPrimitive.content })
                first.expectHostUi(false)
                connect(web).use { second ->
                    second.hello(); second.json(); second.expectHostUi(false)
                    connect(web).use { pending ->
                        web.onHostUiRequested()
                        first.expectHostUi(true); second.expectHostUi(true)
                        pending.hello()
                        assertEquals("welcome", pending.json()["type"]!!.jsonPrimitive.content)
                        pending.expectHostUi(false)
                    }
                }
            }
        } finally { web.close() }
    }

    @Test fun requestsFollowTheLastTouchThroughFiveSecondsThenBroadcast() {
        val clock = ManualClock()
        val web = output(hostUi = true, clock = clock)
        web.open(host)
        try {
            connect(web).use { first ->
                first.hello(); first.json()
                connect(web).use { second ->
                    second.hello(); second.json()
                    first.touch()
                    web.onHostUiRequested()
                    first.expectHostUi(true); second.expectHostUi(false)
                    clock.advanceNanos(1_000_000_000)
                    second.touch()
                    web.onHostUiRequested()
                    first.expectHostUi(false); second.expectHostUi(true)
                    clock.advanceNanos(1_000_000_000)
                    first.touch(pressed = false)
                    clock.advanceNanos(5_000_000_000)
                    web.onHostUiRequested()
                    first.expectHostUi(true); second.expectHostUi(false)
                    clock.advanceNanos(1)
                    web.onHostUiRequested()
                    first.expectHostUi(true); second.expectHostUi(true)
                }
            }
        } finally { web.close() }
    }

    @Test fun disconnectedLastTouchFallsBackToAllConnectedClients() {
        val clock = ManualClock()
        val web = output(hostUi = true, clock = clock)
        web.open(host)
        try {
            connect(web).use { first ->
                first.hello(); first.json(); first.touch()
                connect(web).use { second ->
                    second.hello(); second.json()
                    connect(web).use { last ->
                        last.hello(); last.json()
                        clock.advanceNanos(1)
                        last.touch()
                        last.send(8, closePayload(1000))
                        assertEquals(8, last.read().opcode)
                        web.onHostUiRequested()
                        first.expectHostUi(true); second.expectHostUi(true)
                    }
                }
            }
        } finally { web.close() }
        web.onHostUiRequested()
    }
}
