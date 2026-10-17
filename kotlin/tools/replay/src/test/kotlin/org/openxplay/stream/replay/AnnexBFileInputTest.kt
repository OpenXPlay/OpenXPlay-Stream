// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.replay

import java.time.Duration
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import org.junit.jupiter.api.Assertions.assertTimeoutPreemptively
import org.junit.jupiter.api.Test
import org.openxplay.stream.core.*
import kotlin.test.*

public class AnnexBFileInputTest {
    @Test
    public fun defaultRateIsPacedAndStopFencesCallbacks(): Unit {
        val input = AnnexBFileInput(clipPath())
        val frames = CopyOnWriteArrayList<VideoFrame>()
        val ready = CountDownLatch(7)
        try {
            input.start(configuration(), object : TestSink() {
                override fun videoFrame(frame: VideoFrame) { frames.add(frame); ready.countDown() }
            })
            ready.awaitResult()
            assertTimeoutPreemptively(Duration.ofMillis(250)) { input.stop() }
            val elapsed = frames[6].receivedAtNanos - frames[0].receivedAtNanos
            assertTrue(elapsed >= 150_000_000, "Seven frames must be paced at 30 FPS")
            assertTrue(elapsed < 3_000_000_000, "Replay must make progress within bounded time")
            val stopped = frames.size
            Thread.sleep(100)
            assertEquals(stopped, frames.size)
            input.stop()
            assertFalse(input.control(ControlEvent.Key(KeyCode.HOME, true, origin)))
            frames.zipWithNext().forEach { (a, b) ->
                assertTrue(b.ptsUs > a.ptsUs)
                assertTrue(b.receivedAtNanos > a.receivedAtNanos)
            }
        } finally { input.stop() }
    }

    @Test
    public fun requestSelectsNextKeyframeAndMostRecentNearEof(): Unit {
        val parsed = AnnexBClip.parse(clipBytes())
        val input = AnnexBFileInput(clipBytes(), frameRate = 300.0, loop = false)
        val frames = CopyOnWriteArrayList<VideoFrame>()
        val ready = CountDownLatch(5)
        try {
            input.start(configuration(), object : TestSink() {
                override fun videoFrame(frame: VideoFrame) {
                    frames.add(frame)
                    if (frames.size == 2 || frames.size == 4) {
                        input.control(ControlEvent.KeyFrameRequest(ScreenId.MAIN, origin))
                    }
                    ready.countDown()
                }
            })
            ready.awaitResult()
            input.stop()
            assertFalse(frames[1].keyFrame)
            assertTrue(frames[2].keyFrame)
            assertContentEquals(parsed.accessUnits[30].data, frames[2].data)
            assertFalse(frames[3].keyFrame)
            assertTrue(frames[4].keyFrame)
            assertContentEquals(parsed.accessUnits[30].data, frames[4].data)
            assertTrue(frames[4].ptsUs > frames[3].ptsUs)
        } finally { input.stop() }
    }

    @Test
    public fun loopingKeepsTimestampsAndRestartUsesNewSession(): Unit {
        val input = AnnexBFileInput(clipBytes(), frameRate = 500.0)
        val frames = CopyOnWriteArrayList<VideoFrame>()
        val first = CountDownLatch(61)
        val restarted = CountDownLatch(1)
        try {
            input.start(configuration(), object : TestSink() {
                override fun videoFrame(frame: VideoFrame) { frames.add(frame); first.countDown() }
            })
            first.awaitResult()
            input.stop()
            assertTrue(frames[60].keyFrame)
            assertContentEquals(frames[0].data, frames[60].data)
            frames.zipWithNext().forEach { (a, b) ->
                assertTrue(b.ptsUs > a.ptsUs)
                assertTrue(b.receivedAtNanos > a.receivedAtNanos)
            }
            val previous = frames.last()
            input.start(configuration(), object : TestSink() {
                override fun videoFrame(frame: VideoFrame) { frames.add(frame); restarted.countDown() }
            })
            restarted.awaitResult()
            input.stop()
            assertTrue(frames.last().session.value > previous.session.value)
            assertTrue(frames.last().ptsUs > previous.ptsUs)
            assertTrue(frames.last().receivedAtNanos > previous.receivedAtNanos)
        } finally { input.stop() }
    }

    @Test
    public fun touchAndKeysAreDeliveredOnWorkerWithCapabilities(): Unit {
        val events = CopyOnWriteArrayList<ControlEvent>()
        val threads = CopyOnWriteArrayList<Thread>()
        val received = CountDownLatch(2)
        val input = AnnexBFileInput(clipBytes(), controlListener = ReplayControlListener {
            events.add(it); threads.add(Thread.currentThread()); received.countDown()
        })
        val touch = ControlEvent.Touch(ScreenId.MAIN, listOf(TouchContact(0, 0.25, 0.75, true)), origin)
        val key = ControlEvent.Key(KeyCode.HOME, true, origin)
        try {
            input.start(configuration(), TestSink())
            assertTimeoutPreemptively(Duration.ofMillis(250)) {
                assertTrue(input.control(touch))
                assertTrue(input.control(key))
            }
            assertFalse(input.control(ControlEvent.Touch(ScreenId.CLUSTER, emptyList(), origin)))
            assertFalse(input.control(ControlEvent.MicPacket(0, byteArrayOf(1), 0, origin)))
            received.awaitResult()
            assertEquals(listOf(touch, key), events.toList())
            assertTrue(threads.all { it.name.startsWith("oxp-replay-") })
            assertEquals(setOf(VideoCodec.H264), input.capabilities.codecs)
            assertEquals(setOf(ScreenId.MAIN), input.capabilities.screens)
            assertTrue(input.capabilities.touch && input.capabilities.keyFrameRequests)
            assertEquals(ReconfigureCost.NONE, input.capabilities.reconfigure)
            assertEquals(ReconfigureResult.APPLIED, input.reconfigure(configuration()))
        } finally { input.stop() }
    }

    @Test
    public fun singlePassEndsAndLoadFailureIsReportedAsynchronously(): Unit {
        for (input in listOf(
            AnnexBFileInput(clipPath(), frameRate = 500.0, loop = false),
            AnnexBFileInput(ByteArray(0)),
        )) {
            val ended = CountDownLatch(1)
            val states = CopyOnWriteArrayList<SessionState>()
            try {
                input.start(configuration(), object : TestSink() {
                    override fun session(state: SessionState) {
                        states.add(state)
                        if (state.phase == SessionPhase.ENDED) ended.countDown()
                    }
                })
                ended.awaitResult()
                input.stop()
                assertEquals(SessionPhase.ENDED, states.last().phase)
                assertNotNull(states.last().reason)
            } finally { input.stop() }
        }
    }
}
