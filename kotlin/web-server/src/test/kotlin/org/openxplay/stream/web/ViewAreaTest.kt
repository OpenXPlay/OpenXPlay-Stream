// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-18
package org.openxplay.stream.web

import kotlinx.serialization.json.*
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.io.TempDir
import org.openxplay.stream.core.*
import org.openxplay.stream.core.wire.WireFormatException
import java.net.Socket
import java.nio.file.Path
import java.util.concurrent.CopyOnWriteArrayList
import kotlin.test.*

class ViewAreaTest {
    @TempDir lateinit var root: Path
    private class Port : ViewAreaPort {
        var value: ViewArea? = ViewArea(0, 0, 1920, 1080, 1920, 1080)
        val listeners = CopyOnWriteArrayList<(ViewArea) -> Unit>()
        override fun current() = value
        override fun listen(callback: (ViewArea) -> Unit): AutoCloseable {
            listeners.add(callback); return AutoCloseable { listeners.remove(callback) }
        }
        fun publish(area: ViewArea) { value = area; listeners.forEach { it(area) } }
    }
    @Test fun sendsCurrentAndChangesToAllPagesAndReleasesListener() {
        val port = Port()
        val reports = CopyOnWriteArrayList<String>()
        val output = WebOutput(WebOutputConfig(port = 0, staticAssetRoot = root,
            sourceUrl = "https://example.org/source", viewArea = port))
        output.open(object : OutputHost {
            override fun capabilities(value: OutputCapabilities) = Unit
            override fun control(event: ControlEvent) = Unit
            override fun stats(stats: OutputStats) = Unit
            override fun receiverDiagnostics(origin: Origin, type: String, json: String) { reports.add("$type:$json") }
        })
        try {
            TestClient(Socket("127.0.0.1", output.awaitListening())).use { first ->
                first.hello()
                assertEquals(listOf("view-area"), first.json()["features"]!!.jsonArray.map { it.jsonPrimitive.content })
                assertEquals(port.current()!!.message(), first.json())
                val area = ViewArea(120, 40, 1440, 900, 1920, 1080)
                port.publish(area); assertEquals(area.message(), first.json())
                TestClient(Socket("127.0.0.1", output.awaitListening())).use { second ->
                    second.hello(); second.json(); assertEquals(area.message(), second.json())
                    assertEquals(1, port.listeners.size)
                    port.publish(ViewArea(0, 0, 1920, 1080, 1920, 1080))
                    assertEquals(port.current()!!.message(), first.json()); assertEquals(port.current()!!.message(), second.json())
                    first.send(1, vector("fullscreen-exit").toByteArray())
                    eventually { reports.any { it.startsWith("fullscreen-exit:") } }
                }
            }
        } finally { output.close() }
        assertTrue(port.listeners.isEmpty())
    }
    @Test fun validatesRectanglesAndDiagnosticDirections() {
        val area = WebJsonCodec.parse(vector("view-area"), WebJsonCodec.Direction.RELAY_TO_CLIENT)
        assertFailsWith<WireFormatException> { WebJsonCodec.validate(area, WebJsonCodec.Direction.CLIENT_TO_RELAY) }
        for ((key, value) in listOf("x" to JsonPrimitive(-1), "y" to JsonPrimitive(.5), "width" to JsonPrimitive(0),
            "canvasWidth" to JsonPrimitive(1500), "canvasHeight" to JsonNull)) {
            assertFailsWith<WireFormatException> { WebJsonCodec.validate(JsonObject(area + (key to value))) }
        }
        assertFailsWith<IllegalArgumentException> { ViewArea(Int.MAX_VALUE, 0, 1, 1, Int.MAX_VALUE, 1) }
        val exit = WebJsonCodec.parse(vector("fullscreen-exit"), WebJsonCodec.Direction.CLIENT_TO_RELAY)
        WebJsonCodec.validate(JsonObject(exit + ("hiddenMs" to JsonNull)))
        assertFailsWith<WireFormatException> { WebJsonCodec.validate(exit, WebJsonCodec.Direction.RELAY_TO_CLIENT) }
        assertFailsWith<WireFormatException> { WebJsonCodec.validate(JsonObject(exit + ("hiddenMs" to JsonPrimitive(-1)))) }
    }
    @Test fun absentPortDoesNotAdvertiseAndCombinedPortsAdvertiseBoth() {
        val state = SessionState(SessionId(1), SessionPhase.CONNECTING)
        assertNull(receiverWelcome("1", state, "https://example.org")["features"])
        assertEquals(listOf("display-settings", "view-area"),
            receiverWelcome("1", state, "https://example.org", displaySettings = true, viewArea = true)
                ["features"]!!.jsonArray.map { it.jsonPrimitive.content })
    }
}
