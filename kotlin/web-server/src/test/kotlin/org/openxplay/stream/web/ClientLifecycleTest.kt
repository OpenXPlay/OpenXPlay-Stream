// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-30

package org.openxplay.stream.web

import java.io.ByteArrayOutputStream
import java.net.Socket
import java.nio.file.Path
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.TimeUnit
import kotlinx.serialization.json.*
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.Timeout
import org.junit.jupiter.api.io.TempDir
import org.openxplay.stream.core.*
import org.openxplay.stream.core.wire.WireFormatException
import kotlin.test.*

@Timeout(20)
class ClientLifecycleTest {
    @TempDir lateinit var root: Path
    private class Reports(private val throwing: Boolean = false) : OutputHost {
        val events = CopyOnWriteArrayList<Pair<String, JsonObject>>()
        override fun capabilities(capabilities: OutputCapabilities): Unit = Unit
        override fun control(event: ControlEvent): Unit = Unit
        override fun stats(stats: OutputStats): Unit = Unit
        override fun receiverDiagnostics(origin: Origin, type: String, json: String) {
            events += type to Json.parseToJsonElement(json).jsonObject
            if (throwing) throw IllegalStateException("observer")
        }
        fun ends(): List<JsonObject> = events.filter { it.first == "client-closed" }.map { it.second }
    }
    private fun output(reports: Reports): WebOutput = WebOutput(WebOutputConfig(port = 0, staticAssetRoot = root,
        sourceUrl = "https://example.org/source")).also { it.open(reports) }
    private fun ended(reports: Reports, by: String, cause: String, code: Int): JsonObject {
        eventually { reports.ends().isNotEmpty() }
        val end = reports.ends().single()
        assertEquals(by, end.getValue("by").jsonPrimitive.content)
        assertEquals(cause, end.getValue("cause").jsonPrimitive.content)
        assertEquals(code, end.getValue("code").jsonPrimitive.int)
        for (field in listOf("durationMs", "rxIdleMs", "txStalledMs", "videoFramesSent", "droppedFrames", "bytesSent"))
            assertTrue(end.getValue(field).jsonPrimitive.long >= 0, field)
        return end
    }
    @Test fun peerCloseIncludesCodeAndSanitizedReasonAndReportsOpenOnce() {
        val reports = Reports(); val web = output(reports)
        try {
            TestClient(Socket("127.0.0.1", web.awaitListening())).use { client ->
                client.hello(); client.json(); client.hello()
                client.send(1, vector("ping").toByteArray()); client.json()
                client.send(8, closePayload(1001) + "private peer text".toByteArray())
                assertEquals(8, client.read().opcode)
                val end = ended(reports, "peer", "peer_close", 1001)
                assertEquals("[redacted]", end.getValue("peerReason").jsonPrimitive.content)
                assertEquals(1, reports.events.count { it.first == "client-opened" })
                val opened = reports.events.single { it.first == "client-opened" }.second
                assertFalse(opened.getValue("secure").jsonPrimitive.boolean)
                assertEquals(1, opened.getValue("activeClients").jsonPrimitive.int)
            }
        } finally { web.close() }
        assertEquals(1, reports.ends().size)
    }
    @Test fun eofWithoutCloseFrameIsNotACleanClose() {
        val reports = Reports(); val web = output(reports)
        try {
            TestClient(Socket("127.0.0.1", web.awaitListening())).use { it.hello(); it.json(); it.socket.shutdownOutput() }
            ended(reports, "peer", "peer_eof", 1002)
        } finally { web.close() }
    }
    @Test fun invalidFrameUsesWebSocketFailureCode() {
        val reports = Reports(); val web = output(reports)
        try {
            TestClient(Socket("127.0.0.1", web.awaitListening())).use { client ->
                client.send(8, byteArrayOf(1))
                assertContentEquals(closePayload(1002), client.read().payload)
                ended(reports, "relay", "protocol_error", 1002)
            }
        } finally { web.close() }
    }
    @Test fun otherExceptionRetainsClassWithoutRawPayload() {
        val reports = Reports(); val web = output(reports)
        try {
            TestClient(Socket("127.0.0.1", web.awaitListening())).use { client ->
                client.send(1, "private invalid json".toByteArray())
                assertContentEquals(closePayload(1002), client.read().payload)
                val end = ended(reports, "relay", "io_error", 1002)
                assertEquals("WireFormatException", end.getValue("peerReason").jsonPrimitive.content)
                assertFalse(end.toString().contains("private"))
            }
        } finally { web.close() }
    }
    @Test fun helloDeadlineIsReportedAndShutdownDoesNotDuplicateIt() {
        val reports = Reports(); val web = output(reports)
        try {
            TestClient(Socket("127.0.0.1", web.awaitListening())).use {
                // The peer's test read timeout is also five seconds; wait for the server's deadline first.
                Thread.sleep(5100)
                assertContentEquals(closePayload(1008), it.read().payload)
                ended(reports, "relay", "read_timeout", 1008)
            }
        } finally { web.close() }
        assertEquals(1, reports.ends().size)
    }
    @Test fun stopDeliversAnEndEvenAfterTheRuntimeIsInactive() {
        val reports = Reports(); val web = output(reports)
        try {
            TestClient(Socket("127.0.0.1", web.awaitListening())).use { it.hello(); it.json(); web.close() }
            ended(reports, "relay", "stopped", 1001)
        } finally { web.close() }
    }
    @Test fun aSocketUpgradedAfterStopIsClosedWithoutClientLifecycleReports() {
        val reports = Reports(); val web = output(reports)
        web.awaitListening()
        fun field(owner: Any, name: String): Any = owner.javaClass.getDeclaredField(name).run { isAccessible = true; get(owner) }
        val runtime = field(web, "runtime")
        val executor = field(runtime, "executor") as ScheduledExecutorService
        val gate = CountDownLatch(1); val entered = CountDownLatch(1)
        try {
            // Keep the host queue alive so an incorrect lifecycle report cannot be hidden by shutdown.
            executor.execute { entered.countDown(); gate.await(10, TimeUnit.SECONDS) }
            assertTrue(entered.await(5, TimeUnit.SECONDS))
            web.close()
            val response = ByteArrayOutputStream()
            val socket = object : Socket() {
                override fun getInputStream() = ("GET /oxp/v0 HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\n" +
                    "Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n" +
                    "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n").byteInputStream()
                override fun getOutputStream() = response
                override fun setSoTimeout(timeout: Int): Unit = Unit
            }
            runtime.javaClass.getDeclaredMethod("connection", Socket::class.java).run { isAccessible = true; invoke(runtime, socket) }
            (field(runtime, "hostQueue") as HostQueue).awaitIdle(5000)
            assertTrue(response.toString(Charsets.US_ASCII).startsWith("HTTP/1.1 101"))
            assertTrue(socket.isClosed)
            assertTrue((field(runtime, "clients") as Map<*, *>).isEmpty())
            assertTrue(reports.events.isEmpty())
        } finally {
            gate.countDown(); web.close()
            assertTrue(executor.awaitTermination(5, TimeUnit.SECONDS))
        }
    }
    @Test fun receiverCloseIsOptionalForwardedOnceAndDoesNotAffectTheSocket() {
        val reports = Reports(); val web = output(reports)
        try {
            TestClient(Socket("127.0.0.1", web.awaitListening())).use { client ->
                client.hello(); client.json()
                client.send(1, vector("receiver-close").toByteArray())
                client.send(1, vector("ping").toByteArray()); assertEquals("pong", client.json().getValue("type").jsonPrimitive.content)
                eventually { reports.events.any { it.first == "receiver-close" } }
                assertEquals(WebJsonCodec.parse(vector("receiver-close")), reports.events.single { it.first == "receiver-close" }.second)
                assertTrue(reports.ends().isEmpty())
            }
        } finally { web.close() }
    }
    @Test fun messageAndAudioFormatQueueOverflowsReport1009Once() {
        for (audio in listOf(false, true)) {
            val reports = Reports()
            val web = WebOutput(WebOutputConfig(port = 0, staticAssetRoot = root,
                sourceUrl = "https://example.org/source", hostUi = true)).also { it.open(reports) }
            try {
                TestClient(Socket("127.0.0.1", web.awaitListening())).use { client ->
                    val hello = WebJsonCodec.parse(vector("hello"))
                    client.send(1, JsonObject(hello + ("audio" to JsonArray(listOf(JsonPrimitive("PCM_S16LE"))))).toString().toByteArray())
                    client.json()
                    val lock = WebOutput::class.java.getDeclaredField("lock").run { isAccessible = true; get(web) }
                    // Hold the inbox lock so the writer cannot drain either bounded queue.
                    synchronized(lock) {
                        repeat(257) {
                            if (audio) web.onAudioFormat(0, AudioFormat(0, AudioRole.MEDIA, AudioCodec.PCM_S16LE, 48000, 2))
                            else web.onHostUiRequested()
                        }
                    }
                    assertContentEquals(closePayload(1009), client.read().payload)
                    ended(reports, "relay", "queue_overflow", 1009)
                }
            } finally { web.close() }
            assertEquals(1, reports.ends().size)
        }
    }
    @Test fun aWriterExceptionCannotBeOverwrittenByReaderOrStop() {
        val reports = Reports(); val web = output(reports)
        try {
            TestClient(Socket("127.0.0.1", web.awaitListening())).use { peer ->
                peer.hello(); peer.json()
                fun field(owner: Any, name: String): Any = owner.javaClass.getDeclaredField(name).run { isAccessible = true; get(owner) }
                val lock = field(web, "lock")
                synchronized(lock) {
                    val runtime = field(web, "runtime")
                    val client = (field(runtime, "clients") as Map<*, *>).values.single()!!
                    val socket = field(client, "socket") as Socket
                    // Force getOutputStream to fail before the reader can claim the end under the same lock.
                    socket.close()
                    client.javaClass.getDeclaredMethod("write").invoke(client)
                    (field(client, "wake") as java.util.concurrent.Semaphore).release()
                }
                val end = ended(reports, "peer", "io_error", 1001)
                assertEquals("SocketException", end.getValue("peerReason").jsonPrimitive.content)
            }
        } finally { web.close() }
        assertEquals(1, reports.ends().size)
    }
    @Test fun peerResetBeforeHelloIsReportedOnce() {
        val reports = Reports(); val web = output(reports)
        try {
            TestClient(Socket("127.0.0.1", web.awaitListening())).use {
                it.socket.setSoLinger(true, 0)
            }
            ended(reports, "peer", "peer_reset", 1002)
        } finally { web.close() }
        assertEquals(1, reports.ends().size)
    }
    @Test fun callbackExceptionsDoNotChangePlaybackOrSuppressTheEndCounters() {
        val reports = Reports(throwing = true); val web = output(reports)
        web.onSession(SessionState(SessionId(1), SessionPhase.ACTIVE), SessionConfig(emptyMap()))
        web.onVideoFormat(ScreenId.MAIN, fixtureFormat())
        try {
            TestClient(Socket("127.0.0.1", web.awaitListening())).use { client ->
                client.hello(); client.json(); client.json()
                val bytes = byteArrayOf(0, 0, 0, 2, 0x65, 0)
                web.onVideoFrame(VideoFrame(ScreenId.MAIN, SessionId(1), bytes, 0, true, 0))
                assertEquals(2, client.read().opcode)
                client.send(1, vector("ping").toByteArray()); client.json()
                client.send(8, closePayload(1000)); client.read()
                val end = ended(reports, "peer", "peer_close", 1000)
                assertEquals(1L, end.getValue("videoFramesSent").jsonPrimitive.long)
                assertTrue(end.getValue("bytesSent").jsonPrimitive.long > bytes.size)
            }
        } finally { web.close() }
    }
    @Test fun receiverCloseSchemaRejectsWrongDirectionAndUnsafeFields() {
        val report = WebJsonCodec.parse(vector("receiver-close"), WebJsonCodec.Direction.CLIENT_TO_RELAY)
        assertFailsWith<WireFormatException> { WebJsonCodec.validate(report, WebJsonCodec.Direction.RELAY_TO_CLIENT) }
        for ((key, value) in listOf("reason" to JsonPrimitive("x\ny"), "reason" to JsonPrimitive("字".repeat(42)),
            "code" to JsonPrimitive(65536), "cause" to JsonPrimitive("untrusted"), "openForMs" to JsonPrimitive(-1))) {
            assertFailsWith<WireFormatException> { WebJsonCodec.validate(JsonObject(report + (key to value))) }
        }
    }
}
