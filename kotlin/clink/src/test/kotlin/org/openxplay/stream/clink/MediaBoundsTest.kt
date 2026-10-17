// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

package org.openxplay.stream.clink

import java.io.ByteArrayOutputStream
import java.io.IOException
import java.nio.ByteBuffer
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.random.Random
import kotlin.test.*
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.Timeout
import org.openxplay.stream.clink.media.RtpPacketizer
import org.openxplay.stream.clink.media.RtpSender
import org.openxplay.stream.clink.media.RtpUnit
import org.openxplay.stream.core.*
import org.openxplay.stream.core.codec.TsMuxer

/** Every buffer on the Carlink media path stays bounded while its consumer or socket is stalled. */
@Timeout(60)
class MediaBoundsTest {
    private fun ts(n: Int): ByteArray = ByteArray(188) { (n + it).toByte() }.also { it[0] = 0x47 }
    private fun RtpUnit.framed(): List<ByteArray> = buildList {
        forEachPacket { bytes, offset, length -> add(bytes.copyOfRange(offset, offset + length)) }
    }

    /** The previous list-based packetizer, kept as an independent reference for the streamed layout. */
    private fun reference(ts: List<ByteArray>, ptsUs: Long, firstSequence: Int, ssrc: Int): List<ByteArray> {
        var sequence = firstSequence
        return ts.chunked(7).map { group ->
            ByteBuffer.allocate(14 + group.size * 188).putShort((12 + group.size * 188).toShort())
                .put(0x80.toByte()).put(33).putShort(sequence.toShort()).putInt(TsMuxer.to90kHz(ptsUs).toInt()).putInt(ssrc)
                .also { buffer -> group.forEach { buffer.put(it) } }.array().also { sequence = (sequence + 1) and 0xffff }
        }
    }

    @Test fun `streamed RTP units match the list packetizer and continue the sequence across units`() {
        val random = Random(7)
        val rtp = RtpPacketizer(sequence = 65530)
        val unit = RtpUnit()
        var sequence = 65530
        repeat(40) { call ->
            val packets = List(random.nextInt(1, 40)) { ts(call * 41 + it) }
            val pts = call * 33_333L
            rtp.start(unit, pts); packets.forEach { rtp.append(unit, it) }
            assertTrue(rtp.finish(unit))
            val expected = reference(packets, pts, sequence, rtp.ssrc)
            assertEquals(expected.map { it.toList() }, unit.framed().map { it.toList() })
            sequence = (sequence + expected.size) and 0xffff
        }
        val list = List(15) { ts(it) }
        assertEquals(reference(list, 1_000_000, sequence, rtp.ssrc).map { it.copyOfRange(2, it.size).toList() },
            rtp.packetize(list, 1_000_000).map { it.toList() })
    }

    @Test fun `a runaway muxer call stays within the unit bound and leaves no RTP gap`() {
        // 2026-10-15: an hours-long timestamp jump made one muxer call emit hundreds of thousands of PCR
        // packets; a million packets is about eleven hours of 40 ms PCR fill.
        val rtp = RtpPacketizer(sequence = 100)
        val unit = RtpUnit()
        rtp.start(unit, 0)
        val pcr = ts(0)
        repeat(1_000_000) { rtp.append(unit, pcr) }
        assertTrue(unit.overflowed)
        assertTrue(unit.size <= RtpUnit.MAX_BYTES && unit.bytes.size <= RtpUnit.MAX_BYTES)
        assertFalse(rtp.finish(unit))
        rtp.start(unit, 40_000); rtp.append(unit, ts(1)); assertTrue(rtp.finish(unit))
        assertEquals(100, ByteBuffer.wrap(unit.bytes, 4, 2).short.toInt() and 0xffff, "discarded unit consumed no sequence")
        assertTrue(unit.bytes.size < 1024 * 1024, "an oversized buffer is released at the next unit")
    }

    @Test fun `stalled socket write releases the media worker after the bounded wait and recovers`() {
        val sender = RtpSender(waitNs = 50_000_000)
        val active = AtomicBoolean(true)
        val written = ByteArrayOutputStream()
        val release = CountDownLatch(1)
        val writer = Thread { sender.run { active.get() } }.apply { isDaemon = true; start() }
        try {
            val rtp = RtpPacketizer()
            rtp.start(sender.unit, 0); repeat(20) { rtp.append(sender.unit, ts(it)) }; assertTrue(rtp.finish(sender.unit))
            val expected = sender.unit.bytes.copyOf(sender.unit.size)
            val started = System.nanoTime()
            val stalled: (ByteArray, Int, Int) -> Boolean = { bytes, offset, length ->
                check(release.await(10, TimeUnit.SECONDS)); written.write(bytes, offset, length); true
            }
            assertFalse(sender.submit(stalled), "the write is still in flight")
            assertTrue(System.nanoTime() - started < 1_000_000_000, "the media worker waited only boundedly")
            // Minutes of media-loop iterations during the stall retain nothing beyond the one unit.
            repeat(100_000) { assertFalse(sender.idle()) }
            assertEquals(sender.unit.bytes.size to true, sender.retained())
            assertNull(sender.takeRecovered())
            release.countDown()
            val deadline = System.nanoTime() + 5_000_000_000
            while (!sender.idle()) { check(System.nanoTime() < deadline); Thread.sleep(1) }
            assertTrue(checkNotNull(sender.takeRecovered()) >= 50_000_000)
            assertContentEquals(expected, written.toByteArray())
            rtp.start(sender.unit, 33_333); rtp.append(sender.unit, ts(99)); assertTrue(rtp.finish(sender.unit))
            assertTrue(sender.submit(stalled))
            assertEquals(expected.size + 14 + 188, written.size())
            // A declined packet ends the unit, as the session does while not playing.
            rtp.start(sender.unit, 66_666); repeat(15) { rtp.append(sender.unit, ts(it)) }; assertTrue(rtp.finish(sender.unit))
            var calls = 0
            assertTrue(sender.submit { _, _, _ -> ++calls < 2 })
            assertEquals(2, calls)
            // A socket failure on the writer surfaces on the media worker.
            rtp.start(sender.unit, 99_999); rtp.append(sender.unit, ts(5)); assertTrue(rtp.finish(sender.unit))
            assertFailsWith<IOException> { sender.submit { _, _, _ -> throw IOException("closed") } }
            assertTrue(sender.idle())
        } finally {
            active.set(false); release.countDown(); writer.join(2000)
        }
        assertFalse(writer.isAlive)
    }

    @Test fun `ten minutes of video into a stalled consumer stay within frame and byte bounds`() {
        val queue = VideoQueue()
        fun frame(index: Int, size: Int, pts: Long = index * 33_333L) =
            VideoFrame(ScreenId.MAIN, SessionId(1), ByteArray(size), pts, index % 60 == 0, 0)
        var maxFrames = 0; var maxBytes = 0L
        fun check() {
            maxFrames = maxOf(maxFrames, queue.size()); maxBytes = maxOf(maxBytes, queue.bytes())
            assertTrue(queue.size() <= 120 && queue.bytes() <= 4 * 1024 * 1024, "${queue.size()} ${queue.bytes()}")
        }
        // Ordinary sizes hit the 600 ms span, large ones the byte limit, equal timestamps the frame count.
        for (index in 0 until 18_000) { queue.offer(frame(index, if (index % 60 == 0) 150_000 else 40_000)); check() }
        for (index in 18_000 until 18_600) { queue.offer(frame(index, 900_000)); check() }
        for (index in 18_600 until 21_000) { queue.offer(frame(index, 1_000, 18_600 * 33_333L)); check() }
        assertTrue(maxFrames == 120 && maxBytes > 3 * 1024 * 1024)
        assertTrue(queue.overflows > 100)
        // Recovery: whatever the consumer finds first is a keyframe.
        assertTrue(checkNotNull(queue.poll()).keyFrame)
    }

    @Test fun `stalled head unit socket keeps the media path bounded through five minutes of input and recovers`() {
        val ports = freePorts()
        val vehicle = SimulatedHeadUnit(ports)
        val errors = CopyOnWriteArrayList<Throwable>()
        val diagnostics = CopyOnWriteArrayList<String>()
        val controls = LinkedBlockingQueue<ControlEvent>()
        val ended = CountDownLatch(1)
        val output = ClinkOutput(StaticClinkLink(ClinkAddresses("127.0.0.1", "127.0.0.1", ports.auth, "bounds")),
            PinProvider { "012345" }, MemoryTrustStore(), ports = ports, listener = object : ClinkListener {
                override fun error(error: Throwable) { errors += error }
                override fun diagnostic(message: String) { diagnostics += message }
                override fun state(state: ClinkState) { if (state == ClinkState.ENDED) ended.countDown() }
            })
        fun awaitThat(description: String, timeoutMillis: Long = 5000, condition: () -> Boolean) {
            val deadline = System.nanoTime() + timeoutMillis * 1_000_000
            while (!condition()) {
                vehicle.assertHealthy(); assertTrue(errors.isEmpty(), errors.joinToString())
                check(System.nanoTime() < deadline) { "Timed out: $description" }
                Thread.sleep(2)
            }
        }
        var largest = MediaBacklog(0, 0, 0, 0, 0, 0, 0, false)
        fun assertBounded() {
            val b = checkNotNull(output.backlog())
            assertTrue(b.videoFrames <= 120 && b.videoBytes <= 4 * 1024 * 1024, "video $b")
            assertTrue(b.audioPackets <= 96 && b.audioBytes <= 524_288, "audio $b")
            assertTrue(b.voiceBytes <= 192_000, "voice $b")
            assertTrue(b.rtpBufferBytes <= RtpUnit.MAX_BYTES, "rtp $b")
            largest = MediaBacklog(maxOf(largest.videoFrames, b.videoFrames), maxOf(largest.videoBytes, b.videoBytes),
                maxOf(largest.audioPackets, b.audioPackets), maxOf(largest.audioBytes, b.audioBytes),
                maxOf(largest.voiceCommands, b.voiceCommands), maxOf(largest.voiceBytes, b.voiceBytes),
                maxOf(largest.rtpBufferBytes, b.rtpBufferBytes), largest.rtpInFlight || b.rtpInFlight)
        }
        try {
            output.open(object : OutputHost {
                override fun capabilities(capabilities: OutputCapabilities) = Unit
                override fun control(event: ControlEvent) { controls.add(event) }
                override fun stats(stats: OutputStats) = Unit
            })
            val session = SessionId(9)
            output.onSession(SessionState(session, SessionPhase.ACTIVE),
                SessionConfig(mapOf(ScreenId.MAIN to ScreenConfig(VideoCodec.H264, 1280, 720, 30.0))))
            vehicle.await("placeholder") { vehicle.playing && vehicle.accessUnits.isNotEmpty() }
            output.onVideoFormat(ScreenId.MAIN, BaselineClip().format)
            output.onAudioFormat(0, AudioFormat(0, AudioRole.MEDIA, AudioCodec.AAC_LC, 48000, 2, byteArrayOf(0x11, 0x90.toByte())))
            // Valid AVCC framing; 48 KB access units fill the socket buffers quickly. Audio shares the clock.
            fun avcc(header: Int, fill: Int) = ByteArray(48_000) { fill.toByte() }.also {
                ByteBuffer.wrap(it).putInt(it.size - 4); it[4] = header.toByte()
            }
            val key = avcc(0x65, 0x55); val delta = avcc(0x41, 0x55)
            val base = 5_000_000_000L
            var videoIndex = 0; var audioIndex = 0
            fun feed(keyData: ByteArray = key, deltaData: ByteArray = delta) {
                val i = videoIndex++
                output.onVideoFrame(VideoFrame(ScreenId.MAIN, session, if (i % 60 == 0) keyData else deltaData,
                    base + i * 33_333L, i % 60 == 0, System.nanoTime()))
                while (audioIndex * 21_333L <= i * 33_333L) {
                    output.onAudioPacket(AudioPacket(0, session, ByteArray(300), base + audioIndex++ * 21_333L))
                }
            }
            repeat(3) { feed() }
            vehicle.await("live video") { vehicle.accessUnits.any { it.size == 640 to 360 } }

            vehicle.rtpPaused = true
            awaitThat("send stall", 15_000) {
                feed(); assertBounded(); Thread.sleep(2)
                diagnostics.any { it.startsWith("rtp send stalled") }
            }
            val stalledAt = diagnostics.indexOfFirst { it.startsWith("rtp send stalled") }
            // Five minutes of 30 fps video and AAC while the head unit reads nothing.
            while (videoIndex < 5 * 60 * 30) { feed(); if (videoIndex % 15 == 0) assertBounded() }
            // The media worker keeps running and reports the queues dropping this input; it used to sit
            // inside the blocked write until the watchdog closed the socket.
            val overflows = Regex("video queue overflows=(\\d+) ")
            awaitThat("reports during the stall") {
                diagnostics.drop(stalledAt + 1).any { (overflows.find(it)?.groupValues?.get(1)?.toLong() ?: 0) >= 100 }
            }
            assertBounded()
            assertTrue(largest.rtpInFlight && checkNotNull(output.backlog()).rtpInFlight)
            assertTrue(largest.videoFrames > 0 && largest.audioPackets > 0, "input was retained up to the queue bounds: $largest")
            assertEquals(64 * 1024, largest.rtpBufferBytes, "one reused RTP buffer, never grown for 48 KB frames")

            controls.clear()
            val before = vehicle.accessUnits.size
            vehicle.rtpPaused = false
            awaitThat("send recovery") { diagnostics.any { it.startsWith("rtp send recovered after ms=") } }
            val stalledMs = diagnostics.first { it.startsWith("rtp send recovered after ms=") }.substringAfter("ms=").toLong()
            assertTrue(stalledMs in 1_500..14_000, "the write stayed stuck while the input arrived: $stalledMs ms")
            // Video dropped while stalled resumes at a keyframe requested only once the socket drains.
            awaitThat("keyframe request after recovery") { controls.any { it is ControlEvent.KeyFrameRequest } }
            // Fresh input, recognisable by its payload, starts at the next GOP.
            videoIndex += 60 - videoIndex % 60
            val freshKey = avcc(0x65, 0x66); val freshDelta = avcc(0x41, 0x66)
            repeat(10) { feed(freshKey, freshDelta); assertBounded(); Thread.sleep(10) }
            fun fresh() = vehicle.accessUnits.drop(before).filter { it.data.last() == 0x66.toByte() }
            vehicle.await("live video after recovery") { fresh().size >= 5 }
            assertTrue(fresh().first().keyFrame)
            // The capture enforces RTP sequence and TS continuity: nothing was cut out of the stream mid-way.
            vehicle.assertHealthy()
            assertTrue(errors.isEmpty(), errors.joinToString())
            assertTrue(diagnostics.none { it.startsWith("rtp unit_dropped") })
        } finally {
            vehicle.rtpPaused = false
            vehicle.expectRelayClose(); output.close()
            assertTrue(ended.await(5, TimeUnit.SECONDS))
            vehicle.close()
        }
    }
}
