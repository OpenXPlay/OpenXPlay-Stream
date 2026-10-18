// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

package org.openxplay.stream.web

import java.net.Socket
import java.nio.file.Path
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlinx.serialization.json.*
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.io.TempDir
import org.openxplay.stream.core.*
import kotlin.test.*

class HostQueueTest {
    @TempDir lateinit var root: Path

    @Test fun essentialCallbacksAreKeptInOrderWhileReportsAreCoalescedOrDropped() {
        val queue = HostQueue("test-host", capacity = 4)
        val gate = CountDownLatch(1)
        val delivered = CopyOnWriteArrayList<String>()
        try {
            queue.essential { gate.await(5, TimeUnit.SECONDS) }
            queue.report("a") { delivered.add("a1") }
            queue.report("a") { delivered.add("a2") }
            queue.report("b") { delivered.add("b") }
            repeat(3) { i -> queue.essential { delivered.add("control$i") } }
            // Full: a report with a new key is dropped, while a waiting report still takes the newest value.
            queue.report("c") { delivered.add("c") }
            queue.report("a") { delivered.add("a3") }
            queue.essential { throw IllegalStateException("A failing host callback") }
            queue.essential { delivered.add("control3") }
            assertEquals(1, queue.droppedReports)
            assertEquals(8, queue.size)
            gate.countDown()
            queue.awaitIdle(5000)
            assertEquals(listOf("a3", "b", "control0", "control1", "control2", "control3"), delivered)
            assertEquals(0, queue.size)
            queue.report("c") { delivered.add("c") }
            queue.awaitIdle(5000)
            assertEquals("c", delivered.last())
        } finally { queue.shutdown() }
    }

    @Test fun aStalledHostKeepsTheQueueBoundedAndStillReceivesEveryControlEvent() {
        val gate = CountDownLatch(1)
        val diagnostics = CopyOnWriteArrayList<String>()
        val observed = CopyOnWriteArrayList<OutputStats>()
        val controls = CopyOnWriteArrayList<ControlEvent>()
        val web = WebOutput(WebOutputConfig(port = 0, staticAssetRoot = root,
            sourceUrl = "https://example.org/source"))
        web.open(object : OutputHost {
            override fun capabilities(capabilities: OutputCapabilities): Unit = Unit
            override fun control(event: ControlEvent) { controls.add(event) }
            override fun stats(stats: OutputStats) { observed.add(stats) }
            override fun receiverDiagnostics(origin: Origin, type: String, json: String) {
                gate.await(10, TimeUnit.SECONDS)
                if (type == "stats") diagnostics.add(json)
            }
        })
        try {
            TestClient(Socket("127.0.0.1", web.awaitListening())).use { client ->
                client.hello(); client.json()
                val report = WebJsonCodec.parse(vector("stats"))
                repeat(2000) { i ->
                    client.send(1, JsonObject(report + ("rendered" to JsonPrimitive(i))).toString().toByteArray())
                    if (i % 100 == 0) client.send(1, vector("keyframe").toByteArray())
                }
                // The reader handles messages in order, so the pong follows every report above.
                client.send(1, vector("ping").toByteArray())
                assertEquals("pong", client.json()["type"]!!.jsonPrimitive.content)
                // One waiting report per kind and client, plus the 20 control events, while the host is stalled.
                assertTrue(web.hostQueueSize() <= 30, "${web.hostQueueSize()} host callbacks waiting")
                gate.countDown()
                web.awaitHostIdle()
                assertEquals(20, controls.count { it is ControlEvent.KeyFrameRequest })
                assertTrue(diagnostics.size < 10, "${diagnostics.size} diagnostics")
                assertEquals(1999L, WebJsonCodec.parse(diagnostics.last())["rendered"]!!.jsonPrimitive.long)
                assertEquals(1999L, observed.last().rendered)
            }
        } finally { gate.countDown(); web.close() }
    }
}
