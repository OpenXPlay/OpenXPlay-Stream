// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

package org.openxplay.stream.web

import java.io.OutputStream
import java.net.InetSocketAddress
import java.net.Socket
import java.net.SocketTimeoutException
import java.nio.file.Path
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.serialization.json.*
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.io.TempDir
import org.openxplay.stream.core.*
import org.openxplay.stream.core.wire.*
import kotlin.test.*

class WriteDeadlineTest {
    @TempDir lateinit var root: Path

    /** A socket stream that accepts two chunks slowly, then blocks like a peer that stopped reading. */
    private class SlowThenStalled(private val gate: CountDownLatch) : OutputStream() {
        val written = AtomicInteger()
        val largest = AtomicInteger()
        override fun write(b: Int) = write(byteArrayOf(b.toByte()), 0, 1)
        override fun write(b: ByteArray, off: Int, len: Int) {
            if (written.get() >= 32 * 1024) gate.await(10, TimeUnit.SECONDS) else Thread.sleep(30)
            largest.accumulateAndGet(len, ::maxOf)
            written.addAndGet(len)
        }
    }

    @Test fun progressRestartsTheStallClockAndABlockedChunkKeepsIt() {
        val gate = CountDownLatch(1)
        val socket = SlowThenStalled(gate)
        val stream = ProgressOutputStream(socket, chunkBytes = 16 * 1024)
        assertEquals(0, stream.stalledNanos(System.nanoTime()))
        val writer = Thread { stream.write(ByteArray(64 * 1024)); stream.flush() }.apply { isDaemon = true; start() }
        // Each chunk takes 30 ms, so a slow but moving write never looks stalled for longer.
        var longestWhileMoving = 0L
        while (socket.written.get() < 32 * 1024) {
            longestWhileMoving = maxOf(longestWhileMoving, stream.stalledNanos(System.nanoTime()))
            Thread.sleep(1)
        }
        assertTrue(longestWhileMoving < TimeUnit.MILLISECONDS.toNanos(200), "${longestWhileMoving / 1_000_000} ms")
        Thread.sleep(250)
        assertTrue(stream.stalledNanos(System.nanoTime()) >= TimeUnit.MILLISECONDS.toNanos(250))
        gate.countDown()
        writer.join(5000)
        assertEquals(64 * 1024, socket.written.get())
        assertEquals(64L * 1024, stream.bytesSent.get())
        assertEquals(16 * 1024, socket.largest.get())
        assertEquals(0, stream.stalledNanos(System.nanoTime()))
    }

    @Test fun aBrowserThatStopsReadingIsClosedAfterTheWriteTimeoutWithAReason() {
        val diagnostics = CopyOnWriteArrayList<Pair<String, String>>()
        val web = WebOutput(WebOutputConfig(port = 0, staticAssetRoot = root,
            sourceUrl = "https://example.org/source", writeTimeoutMillis = 300))
        web.open(object : OutputHost {
            override fun capabilities(capabilities: OutputCapabilities): Unit = Unit
            override fun control(event: ControlEvent): Unit = Unit
            override fun stats(stats: OutputStats): Unit = Unit
            override fun receiverDiagnostics(origin: Origin, type: String, json: String) { diagnostics.add(type to json) }
        })
        web.onSession(SessionState(SessionId(1), SessionPhase.ACTIVE), SessionConfig(emptyMap()))
        web.onVideoFormat(ScreenId.MAIN, fixtureFormat())
        try {
            val port = web.awaitListening()
            val socket = Socket().apply { receiveBufferSize = 4096; connect(InetSocketAddress("127.0.0.1", port)) }
            TestClient(socket).use { stalled ->
                stalled.hello(); stalled.json(); stalled.json()
                TestClient(Socket("127.0.0.1", port)).use { reading ->
                    reading.hello(); reading.json(); reading.json()
                    // Far larger than every socket buffer on the way, so the stalled browser's writer blocks inside it.
                    val frame = VideoFrame(ScreenId.MAIN, SessionId(1), ByteArray(4 * 1024 * 1024), 0, true, 0)
                    val sent = System.nanoTime()
                    web.onVideoFrame(frame)
                    // A browser that keeps reading receives the same frame however long it takes.
                    assertEquals(4 * 1024 * 1024 + OxpWebCodec.HEADER_SIZE, reading.read().payload.size)
                    eventually { diagnostics.any { it.first == "relay-close" } }
                    val elapsed = System.nanoTime() - sent
                    assertTrue(elapsed >= TimeUnit.MILLISECONDS.toNanos(300), "${elapsed / 1_000_000} ms")
                    val reason = Json.parseToJsonElement(diagnostics.single { it.first == "relay-close" }.second).jsonObject
                    assertEquals("write-timeout", reason["reason"]!!.jsonPrimitive.content)
                    assertTrue(reason["stalledMs"]!!.jsonPrimitive.long >= 300)
                    // The relay closed the stalled connection: reading ends in a reset or a truncated frame, not a timeout.
                    val failure = runCatching { repeat(4) { stalled.read() } }.exceptionOrNull()
                    assertNotNull(failure)
                    assertFalse(failure is SocketTimeoutException, "$failure")
                    reading.send(1, vector("ping").toByteArray())
                    assertEquals("pong", reading.json()["type"]!!.jsonPrimitive.content)
                    assertEquals(1, diagnostics.count { it.first == "relay-close" })
                    eventually { diagnostics.any { it.first == "client-closed" } }
                    val closed = Json.parseToJsonElement(diagnostics.single { it.first == "client-closed" }.second).jsonObject
                    assertEquals("write_timeout", closed["cause"]!!.jsonPrimitive.content)
                    assertEquals("relay", closed["by"]!!.jsonPrimitive.content)
                    assertEquals(1011, closed["code"]!!.jsonPrimitive.int)
                    assertTrue(closed["txStalledMs"]!!.jsonPrimitive.long >= 300)
                }
            }
        } finally { web.close() }
    }
}
