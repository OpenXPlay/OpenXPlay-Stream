// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-22

package org.openxplay.stream.cast

import java.net.InetSocketAddress
import java.net.Socket
import java.util.concurrent.atomic.AtomicLong
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.Timeout
import kotlin.test.*

internal class RunningStream(options: ServerOptions = ServerOptions(), now: () -> Long = System::nanoTime) : AutoCloseable {
    val threads = TrackedThreads()
    val server = CastStreamServer(0, "0123456789abcdef0123456789abcdef", threads, options, now)
    init { server.reset(1); server.bind(); threads.newThread { server.accept { true } }.apply { name = "test-accept"; start() } }
    fun request(method: String = "GET", path: String = server.path(1), extra: String = ""): Pair<Socket, HttpInput> {
        val socket = Socket(); socket.receiveBufferSize = 1024; socket.connect(InetSocketAddress(LOOPBACK, server.port), 1000)
        socket.getOutputStream().apply { write("$method $path HTTP/1.1\r\nHost: 127.0.0.1\r\n$extra\r\n".toByteArray()); flush() }
        return socket to HttpInput(socket, 2000)
    }
    fun keyframe(): CastChunk {
        val chunks = mutableListOf<CastChunk>()
        CastMediaMuxer(videoFormat(), false) { chunks += it }.write(MediaEvent.Video(frame(), 1_000_000), System.nanoTime())
        return chunks.single()
    }
    override fun close() { server.close(); server.awaitWriters(); threads.awaitEnded() }
}

@Timeout(10)
class StreamServerTest {
    @Test fun staleCacheWaitsForANewerKeyframeAndSkipsTheOldGop() {
        val now = AtomicLong(2_000_000_000)
        RunningStream(now = now::get).use { stream ->
            val old = stream.keyframe().copy(atNanos = now.get() - 501_000_000); stream.server.publish(1, old)
            val request = stream.request()
            request.first.use {
                await("Stale GET did not request a keyframe") { stream.server.reservedClients == 1 && stream.server.needsKeyframe.get() }
                assertEquals(0, stream.server.clients); assertNull(stream.server.streamStart)
                stream.server.publish(1, CastChunk(ByteArray(188), false, now.get()))
                assertEquals(0, stream.server.clients)
                val fresh = old.copy(bytes = old.bytes + ByteArray(188) { 0x47 }, atNanos = now.get())
                stream.server.publish(1, fresh)
                assertEquals("HTTP/1.1 200 OK", request.second.head().start)
                assertContentEquals(fresh.bytes, request.second.exact(fresh.bytes.size))
                await("First chunk time missing") { stream.server.streamStart?.atNanos == fresh.atNanos }
            }
        }
    }
    @Test fun staleCacheFallsBackAtTheFreshStartDeadlineIncludingBufferedDeltas() {
        val now = AtomicLong(2_000_000_000)
        val options = ServerOptions(keyframeWaitMs = 1000, freshKeyframeWaitMs = 120)
        RunningStream(options, now::get).use { stream ->
            val old = stream.keyframe().copy(atNanos = now.get() - 1_000_000_000); stream.server.publish(1, old)
            val began = System.nanoTime(); val request = stream.request()
            request.first.use {
                await("Stale GET did not wait") { stream.server.needsKeyframe.get() }
                assertEquals(0, stream.server.clients)
                val delta = CastChunk(ByteArray(188) { 0x47 }, false, now.get()); stream.server.publish(1, delta)
                assertEquals("HTTP/1.1 200 OK", request.second.head().start)
                assertTrue(System.nanoTime() - began >= 100_000_000)
                assertContentEquals(old.bytes + delta.bytes, request.second.exact(old.bytes.size + delta.bytes.size))
                await("Fallback chunk time missing") { stream.server.streamStart?.atNanos == old.atNanos }
            }
        }
    }
    @Test fun freshCacheIncludingTheFiveHundredMillisecondBoundaryStartsWithoutARequest() {
        for (age in listOf(0L, 500_000_000)) {
            val now = AtomicLong(2_000_000_000)
            RunningStream(now = now::get).use { stream ->
                val key = stream.keyframe().copy(atNanos = now.get() - age); stream.server.publish(1, key)
                val request = stream.request()
                request.first.use {
                    assertEquals("HTTP/1.1 200 OK", request.second.head().start)
                    assertContentEquals(key.bytes, request.second.exact(key.bytes.size))
                    assertFalse(stream.server.needsKeyframe.get())
                }
            }
        }
    }
    @Test fun staleWaitRetainsTheFallbackIfTheLiveCacheExpires() {
        val now = AtomicLong(2_000_000_000)
        RunningStream(ServerOptions(freshKeyframeWaitMs = 120), now::get).use { stream ->
            val old = stream.keyframe().copy(atNanos = now.get() - 1_000_000_000); stream.server.publish(1, old)
            val request = stream.request()
            request.first.use {
                await("Stale GET did not wait") { stream.server.needsKeyframe.get() }
                now.addAndGet(11_000_000_000)
                stream.server.publish(1, CastChunk(ByteArray(188), false, now.get()))
                assertEquals("HTTP/1.1 200 OK", request.second.head().start)
                assertContentEquals(old.bytes, request.second.exact(old.bytes.size))
            }
        }
    }
    @Test fun newestStreamingClientOwnsTheStartTimeAndWaitingGetAndHeadDoNotReplaceIt() {
        val now = AtomicLong(2_000_000_000)
        RunningStream(now = now::get).use { stream ->
            val key = stream.keyframe().copy(atNanos = now.get()); stream.server.publish(1, key)
            val first = stream.request()
            first.first.use {
                first.second.head(); first.second.exact(key.bytes.size)
                await("Initial stream start missing") { stream.server.streamStart != null }
                val initial = stream.server.streamStart
                now.addAndGet(100_000_000); val fresh = key.copy(atNanos = now.get()); stream.server.publish(1, fresh)
                val second = stream.request()
                second.first.use {
                    second.second.head(); second.second.exact(fresh.bytes.size)
                    await("New stream did not replace the start") { stream.server.streamStart?.atNanos == fresh.atNanos }
                    val newest = stream.server.streamStart; assertNotSame(initial, newest)
                    val head = stream.request("HEAD")
                    head.first.use { assertEquals("HTTP/1.1 200 OK", head.second.head().start) }
                    assertSame(newest, stream.server.streamStart)
                    now.addAndGet(501_000_000)
                    val waiting = stream.request()
                    waiting.first.use {
                        await("Third GET did not wait") { stream.server.reservedClients == 3 && stream.server.needsKeyframe.get() }
                        assertSame(newest, stream.server.streamStart)
                        stream.server.reset(2)
                        assertEquals("HTTP/1.1 404 Not Found", waiting.second.head().start)
                        assertNull(stream.server.streamStart)
                    }
                }
            }
        }
    }
    @Test fun headAcceptsLfAndDuplicateRequestHeaders() {
        RunningStream().use { stream ->
            Socket(LOOPBACK, stream.server.port).use { socket ->
                socket.getOutputStream().apply {
                    write("HEAD ${stream.server.path(1)} HTTP/1.1\nHost: first\nhOsT: second\n\n".toByteArray()); flush()
                }
                val input = HttpInput(socket, 2000)
                assertEquals("HTTP/1.1 200 OK", input.head().start); assertEquals(-1, input.byte())
            }
        }
    }
    @Test fun getHeadHeadersPacketLayoutAndSecondClientGetsGopCache() {
        RunningStream().use { stream ->
            val key = stream.keyframe(); stream.server.publish(1, key)
            val first = stream.request(extra = "Range: bytes=0-\r\n")
            first.first.use {
                val head = first.second.head()
                assertEquals("HTTP/1.1 200 OK", head.start)
                assertEquals("video/mp2t", head.headers["content-type"]); assertEquals("no-cache", head.headers["cache-control"])
                assertEquals("Streaming", head.headers["transfermode.dlna.org"])
                assertEquals(DLNA_FEATURES, head.headers["contentfeatures.dlna.org"])
                assertEquals(USER_AGENT, head.headers["server"]); assertNull(head.headers["content-length"])
                val bytes = first.second.exact(key.bytes.size)
                assertContentEquals(key.bytes, bytes)
                val packets = bytes.toList().chunked(188).map { it.toByteArray() }
                assertTrue(packets.all { it.size == 188 && it[0] == 0x47.toByte() })
                fun pid(packet: ByteArray) = ((packet[1].toInt() and 31) shl 8) or (packet[2].toInt() and 255)
                assertEquals(listOf(0, 0x100, 0x101), packets.take(3).map(::pid))
                assertTrue(packets[2][5].toInt() and 0x40 != 0)
                val offset = 5 + (packets[2][4].toInt() and 255)
                assertContentEquals(byteArrayOf(0, 0, 1, 0xe0.toByte()), packets[2].copyOfRange(offset, offset + 4))
                val live = CastChunk(key.bytes.copyOfRange(376, key.bytes.size), false, System.nanoTime())
                stream.server.publish(1, live)
                val second = stream.request()
                second.first.use {
                    assertEquals("HTTP/1.1 200 OK", second.second.head().start)
                    assertContentEquals(key.bytes + live.bytes, second.second.exact(key.bytes.size + live.bytes.size))
                }
            }
            val head = stream.request("HEAD")
            head.first.use { assertEquals("HTTP/1.1 200 OK", head.second.head().start); assertEquals(-1, head.second.byte()) }
        }
    }
    @Test fun invalidPathMethodMissingKeyframeAndClientLimit() {
        RunningStream(ServerOptions(keyframeWaitMs = 50)).use { stream ->
            for ((method, path, code) in listOf(Triple("GET", "/wrong", 404), Triple("POST", stream.server.path(1), 405),
                Triple("GET", stream.server.path(1), 503))) {
                val request = stream.request(method, path)
                request.first.use { assertTrue(request.second.head().start.startsWith("HTTP/1.1 $code ")) }
            }
            stream.server.publish(1, stream.keyframe())
            val clients = (1..4).map { stream.request() }
            try {
                clients.forEach { assertEquals("HTTP/1.1 200 OK", it.second.head().start) }
                assertEquals(4, stream.server.clients)
                val fifth = stream.request()
                fifth.first.use { assertTrue(fifth.second.head().start.startsWith("HTTP/1.1 503 ")) }
                stream.server.reset(2)
                val stale = stream.request(path = stream.server.path(1))
                stale.first.use { assertTrue(stale.second.head().start.startsWith("HTTP/1.1 404 ")) }
            } finally { clients.forEach { it.first.close() } }
        }
    }
    @Test fun getWaitsForFirstKeyframeAndCacheExpiresAtTenSecondsOrFourMiB() {
        var now = System.nanoTime()
        RunningStream(ServerOptions(keyframeWaitMs = 1000), now = { now }).use { stream ->
            val request = stream.request()
            await("GET registered") { stream.server.reservedClients == 1 }
            assertEquals(0, stream.server.clients)
            assertTrue(stream.server.needsKeyframe.getAndSet(false))
            val key = stream.keyframe().copy(atNanos = now)
            stream.server.publish(1, key)
            request.first.use { assertEquals("HTTP/1.1 200 OK", request.second.head().start); assertContentEquals(key.bytes, request.second.exact(key.bytes.size)) }
            now += 11_000_000_000
            stream.server.publish(1, CastChunk(ByteArray(188), false, now))
            assertFalse(stream.server.needsKeyframe.get())
            stream.server.reset(2)
            stream.server.publish(2, key.copy(atNanos = now))
            stream.server.publish(2, CastChunk(ByteArray(4 * 1024 * 1024), false, now))
            assertFalse(stream.server.needsKeyframe.get())
        }
    }
    @Test fun cacheExpiryDoesNotRequestKeyframeUntilAnotherGetArrives() {
        var now = System.nanoTime()
        RunningStream(ServerOptions(keyframeWaitMs = 1000), now = { now }).use { stream ->
            assertFalse(stream.server.needsKeyframe.get())
            val key = stream.keyframe().copy(atNanos = now); stream.server.publish(1, key)
            val first = stream.request()
            first.first.use {
                assertEquals("HTTP/1.1 200 OK", first.second.head().start)
                assertContentEquals(key.bytes, first.second.exact(key.bytes.size))
                now += 11_000_000_000
                val live = key.bytes.copyOfRange(376, key.bytes.size)
                stream.server.publish(1, CastChunk(live, false, now))
                assertContentEquals(live, first.second.exact(live.size))
                assertEquals(1, stream.server.clients); assertFalse(stream.server.needsKeyframe.get())
                val second = stream.request()
                second.first.use {
                    await("GET did not wait for expired cache") { stream.server.reservedClients == 2 }
                    assertTrue(stream.server.needsKeyframe.getAndSet(false))
                    stream.server.publish(1, key.copy(atNanos = now))
                    assertEquals("HTTP/1.1 200 OK", second.second.head().start)
                    assertContentEquals(key.bytes, second.second.exact(key.bytes.size))
                }
            }
        }
    }
    @Test fun slowClientIsClosedOnBoundedQueueOverflow() {
        RunningStream(ServerOptions(clientBytes = 64 * 1024)).use { stream ->
            stream.server.publish(1, stream.keyframe())
            val client = stream.request()
            client.first.use {
                assertEquals("HTTP/1.1 200 OK", client.second.head().start)
                // Stop reading while the producer outruns the socket writer.
                val bytes = ByteArray(188 * 128) { 0x47 }
                repeat(1000) { if (stream.server.overflows.get() == 0L) stream.server.publish(1, CastChunk(bytes, false, System.nanoTime())) }
                await("Slow client not dropped") { stream.server.overflows.get() == 1L && stream.server.clients == 0 }
            }
        }
        val queue = StreamClient(188)
        assertTrue(queue.offer(ByteArray(188))); assertFalse(queue.offer(ByteArray(188))); assertNull(queue.take())
    }
}
