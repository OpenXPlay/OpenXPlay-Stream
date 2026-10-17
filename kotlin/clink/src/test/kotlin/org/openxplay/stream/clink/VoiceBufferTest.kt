// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

package org.openxplay.stream.clink

import kotlin.test.*

class VoiceBufferTest {
    @Test fun startsAtEightyMillisecondsAndPreservesPcm() {
        val buffer = VoiceBuffer(16000, 1)
        buffer.push(ByteArray(1920) { 1 }, 0)
        assertNull(buffer.read(0))
        buffer.push(ByteArray(640) { 2 }, 20_000)
        repeat(3) { assertContentEquals(ByteArray(640) { 1 }, buffer.read(20_000)) }
        assertContentEquals(ByteArray(640) { 2 }, buffer.read(20_000))
        assertNull(buffer.read(40_000))
        assertEquals(100_000L, buffer.targetUs)
        buffer.push(ByteArray(2560), 60_000)
        assertNull(buffer.read(60_000))
        buffer.push(ByteArray(640), 80_000)
        assertNotNull(buffer.read(80_000))
    }

    @Test fun largestRecentGapCapsTargetAndQuietTimeDecaysIt() {
        val buffer = VoiceBuffer(16000, 1)
        buffer.push(ByteArray(1280), 0)
        buffer.push(ByteArray(1280), 250_000)
        repeat(4) { assertNotNull(buffer.read(250_000)) }
        assertNull(buffer.read(270_000))
        assertEquals(300_000L, buffer.targetUs)
        buffer.read(10_270_000)
        assertEquals(290_000L, buffer.targetUs)
        buffer.read(220_270_000)
        assertEquals(80_000L, buffer.targetUs)
    }

    @Test fun trimsOldestAlignedSamplesAndDrainsShortEndedStreams() {
        val buffer = VoiceBuffer(16000, 1)
        buffer.push(ByteArray(9600) { if (it < 640) 1 else 2 }, 0)
        assertEquals(8960, buffer.size)
        assertContentEquals(ByteArray(640) { 2 }, buffer.read(0))
        val short = VoiceBuffer(8000, 2)
        short.push(ByteArray(12) { 7 }, 0)
        assertNull(short.read(0))
        short.end()
        assertContentEquals(ByteArray(12) { 7 }, short.read(0))
        assertTrue(short.drained)
        assertNull(short.read(0))
    }

    @Test fun resampledStreamsDrainWithoutPaddingBetweenPacedFrames() {
        for (rate in listOf(8000, 16000, 48000)) for (channels in 1..2) {
            val stream = VoiceStream(rate, channels)
            val input = ByteArray(rate * channels * 2 * 80 / 1000) { if (it % 2 == 0) 0xe8.toByte() else 3 }
            input.asList().chunked(37).forEach { stream.push(it.toByteArray(), 0) }
            stream.end(); stream.end()
            val output = java.io.ByteArrayOutputStream()
            while (!stream.drained) output.write(assertNotNull(stream.read(0)))
            assertEquals(2560, output.size(), "rate=$rate channels=$channels")
            assertNull(stream.read(0))
        }
    }

    @Test fun streamsRefillIndependently() {
        val a = VoiceBuffer(16000, 1)
        val b = VoiceBuffer(16000, 1)
        a.push(ByteArray(2560), 0)
        b.push(ByteArray(640), 0)
        assertNotNull(a.read(0))
        assertNull(b.read(0))
        assertEquals(80_000L, b.targetUs)
    }

    @Test fun deadlinesDoNotAccumulatePollingDelayAndStallsReanchor() {
        val pacer = VoicePacer()
        pacer.reset(1_000_000)
        repeat(500) { n ->
            assertEquals(1_000_000 + (n + 1) * 20_000L, pacer.next(1_000_000 + n * 20_000L + 2_000))
        }
        assertEquals(11_020_000L, pacer.next(11_200_000)) // Exactly 200 ms late.
        assertEquals(11_240_001L, pacer.next(11_220_001))
        pacer.reset(30_000_000)
        assertEquals(30_020_000L, pacer.next(30_000_000))
    }

    @Test fun mediaQueueLimitsRetainAOneSecondBurstAndDropOldest() {
        data class Packet(val bytes: Int, val pts: Long)
        val queue = java.util.ArrayDeque<Packet>()
        var bytes = 0
        fun offer(packet: Packet) {
            while (queue.isNotEmpty() && mediaQueueFull(queue.size, bytes + packet.bytes, mediaQueueSpanUs(packet.pts, queue.first.pts))) {
                bytes -= queue.removeFirst().bytes
            }
            queue.addLast(packet); bytes += packet.bytes
        }
        repeat(50) { offer(Packet(400, it * 20_000L)) }
        assertEquals(50, queue.size)
        repeat(100) { offer(Packet(400, (it + 50) * 20_000L)) }
        assertEquals(76, queue.size)
        assertEquals(74 * 20_000L, queue.first.pts)
        queue.clear(); bytes = 0
        repeat(97) { offer(Packet(1, 0)) }
        assertEquals(96, queue.size)
        queue.clear(); bytes = 0
        repeat(65) { offer(Packet(8184, 0)) }
        assertEquals(64, queue.size)
        assertEquals(Long.MAX_VALUE, mediaQueueSpanUs(Long.MAX_VALUE, Long.MIN_VALUE))
        assertEquals(0L, mediaQueueSpanUs(Long.MIN_VALUE, Long.MAX_VALUE))
        assertFalse(mediaQueueFull(95, 524_288, 1_500_000))
        assertTrue(mediaQueueFull(96, 0, 0))
        assertTrue(mediaQueueFull(0, 524_289, 0))
        assertTrue(mediaQueueFull(0, 0, 1_500_001))
    }
}
