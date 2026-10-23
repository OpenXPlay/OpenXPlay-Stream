// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-22

package org.openxplay.stream.cast

import org.junit.jupiter.api.Test
import kotlin.test.*

class CastTimelineTest {
    @Test fun fakeClockMergesVideoAndExactAudioSlotsInTimestampOrder() {
        val start = 5_000_000_000L
        var now = start
        val timeline = CastTimeline(start, true)
        timeline.video(frame(received = start))
        assertTrue(timeline.take(now).isEmpty())
        timeline.video(frame(false, start + 30_000_000))
        now += 300_000_000
        val events = timeline.take(now)
        assertTrue(events.zipWithNext().all { (a, b) -> b.ptsUs >= a.ptsUs })
        assertEquals(listOf(1_000_000L, 1_030_000L), events.filterIsInstance<MediaEvent.Video>().map { it.ptsUs })
        assertEquals((0L..4L).map { 1_000_000 + it * 1024 * 1_000_000 / 48_000 }, events.filterIsInstance<MediaEvent.Audio>().map { it.ptsUs })
        val chunks = mutableListOf<CastChunk>(); val muxer = CastMediaMuxer(videoFormat(), true) { chunks += it }
        events.forEach { muxer.write(it, now) }
        val pesPts = chunks.flatMap { chunk -> chunk.bytes.toList().chunked(188).map { it.toByteArray() } }.mapNotNull(::packetPts)
        assertTrue(pesPts.zipWithNext().all { (a, b) -> b >= a }, pesPts.toString())
    }
    @Test fun silenceLiveReplacementQueueCapAndLongVideoGaps() {
        val timeline = CastTimeline(0, true)
        val live = byteArrayOf(9, 8, 7)
        timeline.audio(live)
        var events = timeline.take(300_000_000).filterIsInstance<MediaEvent.Audio>()
        assertContentEquals(live, events.first().data); assertFalse(events.first().silent)
        assertTrue(events.drop(1).all { it.silent && it.data.contentEquals(SILENT_AAC) })
        repeat(100) { timeline.audio(byteArrayOf(it.toByte())) }
        assertEquals(14, timeline.queuedAudio); assertEquals(86L, timeline.dropped)
        events = timeline.take(1_000_000_000).filterIsInstance<MediaEvent.Audio>()
        assertEquals(86.toByte(), events.first().data.single()); assertTrue(events.count { !it.silent } == 14)
        val longGap = timeline.take(5_000_000_000).filterIsInstance<MediaEvent.Audio>()
        assertTrue(longGap.size > 150); assertTrue(longGap.all { it.silent })
        assertEquals(1_000_000 + (5L + events.size + longGap.size - 1) * 1024 * 1_000_000 / 48_000, longGap.last().ptsUs)
    }
    @Test fun staleArrivalUsesDequeueClockAndLateFramesNeverRewindTheMuxer() {
        val timeline = CastTimeline(1_000_000_000, true)
        timeline.take(1_400_000_000)
        timeline.video(frame(received = 1_000_000_000))
        timeline.video(frame(false, 1_000_000_000))
        timeline.video(frame(false, 0))
        val first = timeline.take(1_500_000_000)
        val second = timeline.take(1_800_000_000)
        val all = first + second
        assertTrue(all.zipWithNext().all { (a, b) -> b.ptsUs >= a.ptsUs })
        val video = all.filterIsInstance<MediaEvent.Video>()
        assertTrue(video.zipWithNext().all { (a, b) -> b.ptsUs >= a.ptsUs + 1000 })
        assertEquals(1_500_000L, video.last().ptsUs)
    }
    @Test fun boundedVideoQueueDropsToNextKeyframeAndRequestsRecovery() {
        val timeline = CastTimeline(0, false)
        timeline.video(frame(false, 0)); assertEquals(0, timeline.queuedVideo)
        repeat(61) { timeline.video(frame(key = it == 0 || it == 40, received = 0)) }
        assertEquals(21, timeline.queuedVideo); assertTrue(timeline.requestKeyframe)
        val events = timeline.take(500_000_000).filterIsInstance<MediaEvent.Video>()
        assertTrue(events.first().frame.keyFrame); assertTrue(timeline.queuedVideoBytes <= 8 * 1024 * 1024)
        val giant = VideoFrameCopy.bigFrame()
        timeline.video(giant); assertEquals(0, timeline.queuedVideo)
        timeline.video(frame(false, 0)); assertEquals(0, timeline.queuedVideo)
        timeline.video(frame(true, 0)); assertEquals(1, timeline.queuedVideo)
    }
}

private object VideoFrameCopy {
    fun bigFrame() = org.openxplay.stream.core.VideoFrame(org.openxplay.stream.core.ScreenId.MAIN,
        org.openxplay.stream.core.SessionId(1), ByteArray(8 * 1024 * 1024 + 1), 0, true, 0)
}
internal fun packetPts(packet: ByteArray): Long? {
    if (packet[1].toInt() and 0x40 == 0 || (packet[3].toInt() ushr 4) and 1 == 0) return null
    val pid = ((packet[1].toInt() and 31) shl 8) or (packet[2].toInt() and 255)
    if (pid !in setOf(0x101, 0x102)) return null
    val offset = if ((packet[3].toInt() ushr 4) and 2 != 0) 5 + (packet[4].toInt() and 255) else 4
    fun b(i: Int): Long = packet[offset + i].toLong() and 255
    return (((b(9) ushr 1) and 7) shl 30) or (b(10) shl 22) or ((b(11) ushr 1) shl 15) or (b(12) shl 7) or (b(13) ushr 1)
}
