// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertSame
import kotlin.test.assertTrue
import kotlin.test.assertFailsWith

class SharedUtilitiesTest {
    private fun frame(pts: Long, key: Boolean = false, bytes: Int = 4, session: Long = 1): VideoFrame =
        VideoFrame(ScreenId.MAIN, SessionId(session), ByteArray(bytes), pts, key, 0)

    @Test
    fun newQueueGatesAtKeyframeAndPreservesSharedBytes() {
        var requests = 0
        val queue = FrameQueue(needKeyframe = { requests++ })
        assertFalse(queue.offer(frame(0)))
        assertFalse(queue.offer(frame(1)))
        assertEquals(1, requests)
        val key = frame(2, true)
        assertTrue(queue.offer(key))
        val delivered = queue.poll()!!
        assertSame(key, delivered.frame)
        assertSame(key.data, delivered.frame.data)
        assertTrue(delivered.gap)
        queue.offer(frame(3))
        assertFalse(queue.poll()!!.gap)
        assertNull(queue.poll())
    }

    @Test
    fun durationOverflowRetainsNextKeyframeAndFlagsItsGap() {
        var requests = 0
        val queue = FrameQueue(maxDurationUs = 100, needKeyframe = { requests++ })
        queue.offer(frame(0, true)); queue.offer(frame(50)); queue.offer(frame(90, true)); queue.offer(frame(150))
        assertEquals(2, queue.size)
        assertEquals(60L, queue.durationUs)
        val frames = queue.drain()
        assertEquals(listOf(90L, 150L), frames.map { it.frame.ptsUs })
        assertTrue(frames.first().gap)
        assertFalse(frames.last().gap)
        assertEquals(1, requests)
    }

    @Test
    fun byteOverflowDropsDependentFramesAndWaitsForRecoveryKeyframe() {
        var requests = 0
        val queue = FrameQueue(maxBytes = 8, needKeyframe = { requests++ })
        queue.offer(frame(0, true)); queue.offer(frame(1))
        assertFalse(queue.offer(frame(2)))
        assertEquals(0, queue.size)
        assertEquals(0L, queue.queuedBytes)
        assertFalse(queue.offer(frame(3)))
        assertEquals(1, requests)
        assertTrue(queue.offer(frame(4, true)))
        assertTrue(queue.poll()!!.gap)
        assertEquals(0L, queue.queuedBytes)
    }

    @Test
    fun oversizedKeyframeCannotBreakByteBoundAndCallbackRunsOutsideLock() {
        lateinit var queue: FrameQueue
        var requests = 0
        queue = FrameQueue(maxBytes = 3, needKeyframe = { requests++; assertEquals(0, queue.size) })
        assertFalse(queue.offer(frame(0, true, bytes = 4)))
        assertEquals(1, requests)
        assertFalse(queue.offer(frame(1, true, bytes = 4)))
        assertEquals(1, requests)
        assertEquals(0L, queue.queuedBytes)
        assertTrue(queue.offer(frame(2, true, bytes = 3)))
        assertTrue(queue.poll()!!.gap)
    }

    @Test
    fun sessionChangeAndClearResetDecodingHistoryAndFenceOldFrames() {
        val queue = FrameQueue()
        queue.offer(frame(0, true))
        assertFalse(queue.offer(frame(1, session = 2)))
        assertEquals(0, queue.size)
        assertFalse(queue.offer(frame(2, true, session = 1)))
        assertTrue(queue.offer(frame(3, true, session = 2)))
        assertTrue(queue.poll()!!.gap)
        queue.clear()
        assertFalse(queue.offer(frame(4)))
        assertTrue(queue.offer(frame(5, true)))
        assertEquals(1, queue.drain(1).size)
        assertTrue(queue.drain().isEmpty())
    }

    @Test
    fun backpressureDropsAreCountedAndANewDurationKeepsHistory() {
        var requests = 0
        val queue = FrameQueue(maxDurationUs = 100, maxBytes = 1024, needKeyframe = { requests++ })
        // Waiting for the first key frame is not back-pressure.
        assertFalse(queue.offer(frame(0)))
        assertEquals(0L, queue.dropped)
        queue.offer(frame(10, true)); queue.offer(frame(60))
        assertFalse(queue.offer(frame(120)))
        assertEquals(3L, queue.dropped)
        assertFalse(queue.offer(frame(130)))
        assertEquals(4L, queue.dropped)
        assertTrue(queue.offer(frame(140, true))); assertTrue(queue.offer(frame(150)))
        assertEquals(4L, queue.dropped)
        assertEquals(2, requests)
        queue.maxDurationUs = 300
        assertEquals(300L, queue.maxDurationUs)
        assertTrue(queue.offer(frame(400)))
        assertEquals(3, queue.size)
        queue.maxDurationUs = 50
        assertFalse(queue.offer(frame(410)))
        assertEquals(8L, queue.dropped)
        assertFailsWith<IllegalArgumentException> { queue.maxDurationUs = -1 }
        // An oversized frame counts itself and the frames it flushes.
        val small = FrameQueue(maxBytes = 8)
        small.offer(frame(0, true))
        assertFalse(small.offer(frame(1, bytes = 9)))
        assertEquals(2L, small.dropped)
        assertFalse(small.offer(frame(2)))
        assertTrue(small.offer(frame(3, true))); assertTrue(small.offer(frame(4)))
        assertEquals(3L, small.dropped)
    }

    @Test
    fun zeroDurationQueueAndInvalidBounds() {
        val queue = FrameQueue(maxDurationUs = 0)
        queue.offer(frame(0, true))
        queue.offer(frame(1, true))
        assertEquals(0L, queue.durationUs)
        assertTrue(queue.poll()!!.gap)
        assertFailsWith<IllegalArgumentException> { FrameQueue(maxBytes = 0) }
        assertFailsWith<IllegalArgumentException> { FrameQueue(maxDurationUs = -1) }
    }

    @Test
    fun limiterIsPureAndIndependentPerScreen() {
        val limiter = KeyframeLimiter(250)
        val a = ControlEvent.KeyFrameRequest(ScreenId.MAIN, Origin("a", "one"))
        val b = a.copy(origin = Origin("b", "two"))
        assertEquals(listOf(a), limiter.request(a, 0))
        assertTrue(limiter.request(b, 100).isEmpty())
        assertEquals(250L, limiter.nextDeadlineNanos)
        assertTrue(limiter.poll(249).isEmpty())
        assertEquals(listOf(b), limiter.poll(250))
        assertNull(limiter.nextDeadlineNanos)
        assertTrue(limiter.poll(1000).isEmpty())
        limiter.clear()
        assertEquals(listOf(a), limiter.request(a, 1001))
    }

    @Test
    fun clockSyncSelectsMinimumRttAndEvictsItInSlidingWindow() {
        val sync = ClockSync(2)
        assertNull(sync.estimate)
        // Remote clock is ahead by 500; symmetric transit is 10, processing is 5.
        val first = sync.pong(0, 510, 515, 25)
        assertEquals(500.0, first.offsetNanos)
        assertEquals(20L, first.rttNanos)
        assertEquals(first, sync.pong(100, 650, 655, 205))
        val third = sync.pong(300, 830, 835, 365)
        assertEquals(500.0, third.offsetNanos)
        assertEquals(60L, third.rttNanos)
        assertEquals(third, sync.estimate)
        sync.clear()
        assertNull(sync.estimate)
        assertFailsWith<IllegalArgumentException> { sync.pong(5, 0, 0, 4) }
        assertFailsWith<IllegalArgumentException> { sync.pong(0, 0, 20, 10) }
    }
}
