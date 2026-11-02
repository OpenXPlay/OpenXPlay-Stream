// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-11-02

package org.openxplay.stream.cast

import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.Timeout
import org.openxplay.stream.core.*
import kotlin.test.*

@Timeout(15)
class CastFreshnessLoopbackTest {
    @Test fun idleUsesTheMediaLimiterAndCallsTheHostWithoutTheInboxLock() {
        FakeRenderer().use { fixture ->
            val now = AtomicLong(10_000_000_000)
            val requests = CopyOnWriteArrayList<Pair<Long, ControlEvent.KeyFrameRequest>>()
            val unlocked = AtomicBoolean(true); val threads = TrackedThreads()
            val output = fixture.output(threads = threads, audio = false, options = fixture.options.copy(now = now::get))
            try {
                output.open(object : OutputHost by NO_HOST {
                    override fun control(event: ControlEvent) {
                        if (event !is ControlEvent.KeyFrameRequest) return
                        val delivered = CountDownLatch(1)
                        threads.newThread { output.onVideoFrame(frame(received = now.get())); delivered.countDown() }.apply { start() }
                        if (!delivered.await(1, TimeUnit.SECONDS)) unlocked.set(false)
                        requests += now.get() to event
                    }
                })
                output.onSession(SessionState(SessionId(1), SessionPhase.ACTIVE), SESSION_CONFIG)
                output.onVideoFormat(ScreenId.MAIN, videoFormat()); output.onVideoFrame(frame(received = now.get()))
                await("Initial keyframe request missing") { requests.size == 1 }
                now.set(10_200_000_000)
                await("Initial client missing") { fixture.packets.isNotEmpty() }
                output.onVideoFrame(frame(false, now.get()))
                now.set(10_449_999_999); Thread.sleep(50); assertEquals(1, requests.size)
                now.set(10_450_000_000); Thread.sleep(50); assertEquals(1, requests.size, "Idle request must respect the one second limiter")
                now.set(11_000_000_000)
                await("Idle request missing after limiter deadline") { requests.size == 2 }
                assertEquals(11_000_000_000L, requests[1].first)
                assertEquals(ControlEvent.KeyFrameRequest(ScreenId.MAIN, Origin("cast", "renderer")), requests[1].second)
                now.set(11_100_000_000); output.onVideoFrame(frame(false, now.get()))
                now.set(12_999_999_999); Thread.sleep(50); assertEquals(2, requests.size)
                now.set(13_000_000_000)
                await("Second idle request missing") { requests.size == 3 }
                now.set(20_000_000_000); Thread.sleep(50); assertEquals(3, requests.size)
                output.onVideoFrame(frame(false, now.get(), session = 2))
                now.set(21_000_000_000); Thread.sleep(50); assertEquals(3, requests.size)
                assertTrue(unlocked.get()); assertTrue(fixture.errors.isEmpty(), fixture.errors.joinToString())
            } finally { output.close(); threads.awaitEnded() }
        }
    }
    @Test fun monitorPollsPositionEveryTenSecondsAndClearsLagForInvalidPositionFaultsAndNoClient() {
        FakeRenderer().use { fixture ->
            val now = AtomicLong(10_000_000_000)
            val reports = CopyOnWriteArrayList<CastStatistics>(); val states = CopyOnWriteArrayList<CastState>()
            val failures = CopyOnWriteArrayList<CastFailure>(); val lines = CopyOnWriteArrayList<String>(); val threads = TrackedThreads()
            val output = fixture.output(object : CastListener {
                override fun statistics(statistics: CastStatistics) { reports += statistics }
                override fun state(state: CastState, renderer: CastRenderer?) { states += state }
                override fun failure(failure: CastFailure) { failures += failure }
                override fun diagnostic(message: String) { lines += message }
            }, threads, audio = false, options = fixture.options.copy(now = now::get))
            try {
                output.open(object : OutputHost by NO_HOST {
                    override fun control(event: ControlEvent) { if (event is ControlEvent.KeyFrameRequest) output.onVideoFrame(frame(received = now.get())) }
                })
                output.onSession(SessionState(SessionId(1), SessionPhase.ACTIVE), SESSION_CONFIG)
                output.onVideoFormat(ScreenId.MAIN, videoFormat()); output.onVideoFrame(frame(received = now.get()))
                now.set(10_300_000_000)
                await("Initial client missing") { fixture.packets.isNotEmpty() }
                now.set(10_400_000_000)
                await("Playing state missing") { states.lastOrNull() == CastState.PLAYING }
                fixture.position = "00:00:02.500"
                now.set(20_000_000_000)
                await("Statistics missing") { reports.isNotEmpty() }
                Thread.sleep(50); assertFalse("GetPositionInfo" in fixture.actions)
                assertNull(reports.last().rendererLagMs)
                now.set(21_000_000_000)
                await("Position was not polled") { fixture.positionInstances.isNotEmpty() }
                Thread.sleep(50); now.set(22_000_000_000)
                // The reply can complete on either side of the statistics clock advance.
                await("Lag missing: $reports") { reports.any { it.rendererLagMs == 8200L || it.rendererLagMs == 9200L } }
                assertTrue(fixture.positionInstances.all { it == "0" })
                for (position in listOf("NOT_IMPLEMENTED", "invalid", "00:00:00")) {
                    fixture.position = position
                    val before = fixture.positionInstances.size
                    now.addAndGet(10_000_000_000)
                    await("Position not polled for $position") { fixture.positionInstances.size > before }
                    Thread.sleep(50)
                    val reported = reports.size; now.addAndGet(1_000_000_000)
                    await("Invalid position retained lag") { reports.size > reported && reports.last().rendererLagMs == null }
                }
                fixture.position = "123:00:00"
                val beforeClamp = fixture.positionInstances.size; now.addAndGet(10_000_000_000)
                await("Clamped position not polled") { fixture.positionInstances.size > beforeClamp }
                Thread.sleep(50)
                val beforeReport = reports.size; now.addAndGet(1_000_000_000)
                await("Negative lag was not clamped") { reports.size > beforeReport && reports.last().rendererLagMs == 0L }
                fixture.rejections["GetPositionInfo"] = 10
                repeat(3) {
                    val before = fixture.positionInstances.size
                    now.addAndGet(10_000_000_000)
                    await("Faulting position not polled") { fixture.positionInstances.size > before }
                    Thread.sleep(50)
                    val reported = reports.size; now.addAndGet(1_000_000_000)
                    await("Position fault retained lag") { reports.size > reported && reports.last().rendererLagMs == null }
                }
                assertTrue(failures.isEmpty(), failures.joinToString())
                assertFalse(lines.any { "cause=soap_fault" in it })
                assertEquals(1, fixture.uris.size)
                fixture.transportState = "PAUSED_PLAYBACK"; now.addAndGet(1_000_000_000)
                await("Pause missing") { states.lastOrNull() == CastState.PAUSED }
                val beforePause = fixture.positionInstances.size
                now.addAndGet(20_000_000_000); Thread.sleep(50)
                assertEquals(beforePause, fixture.positionInstances.size)
                output.onSession(SessionState(SessionId(1), SessionPhase.ENDED), SESSION_CONFIG)
                await("Ended session not cleared") { states.lastOrNull() == CastState.WAITING_FOR_VIDEO }
                val reported = reports.size; now.addAndGet(1_000_000_000)
                await("No-client statistics retained lag") { reports.size > reported && reports.last().clients == 0 && reports.last().rendererLagMs == null }
                assertTrue(fixture.errors.isEmpty(), fixture.errors.joinToString())
            } finally { output.close(); threads.awaitEnded() }
        }
    }
}
