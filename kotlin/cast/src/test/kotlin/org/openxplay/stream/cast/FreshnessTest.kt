// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-11-02

package org.openxplay.stream.cast

import org.junit.jupiter.api.Test
import org.openxplay.stream.core.*
import kotlin.test.*

class FreshnessTest {
    @Test fun idleRequiresANonKeyframeAndSilenceAndDoesNotRepeatWithoutNewFrames() {
        val idle = CastIdleKeyframe(250_000_000, 2_000_000_000)
        assertFalse(idle.request(0))
        idle.video(true, 0); assertFalse(idle.request(1_000_000_000))
        idle.video(false, 1_000_000_000)
        assertFalse(idle.request(1_249_999_999)); assertTrue(idle.request(1_250_000_000))
        assertFalse(idle.request(1_250_000_001)); idle.sent(1_250_000_000)
        assertFalse(idle.request(10_000_000_000))
        idle.video(true, 10_000_000_000); assertFalse(idle.request(11_000_000_000))
    }
    @Test fun continuousFramesPostponeIdleAndActualLimiterSendStartsTheTwoSecondInterval() {
        val idle = CastIdleKeyframe(250_000_000, 2_000_000_000)
        val limiter = KeyframeLimiter(1_000_000_000)
        val event = ControlEvent.KeyFrameRequest(ScreenId.MAIN, Origin("cast", "renderer"))
        assertEquals(listOf(event), limiter.request(event, 0))
        for (now in listOf(0L, 100_000_000, 200_000_000, 300_000_000)) {
            idle.video(false, now); assertFalse(idle.request(now + 99_999_999))
        }
        assertFalse(idle.request(549_999_999))
        assertTrue(idle.request(550_000_000)); assertTrue(limiter.request(event, 550_000_000).isEmpty())
        assertTrue(limiter.poll(999_999_999).isEmpty())
        assertEquals(listOf(event), limiter.poll(1_000_000_000)); idle.sent(1_000_000_000)
        idle.video(false, 1_100_000_000)
        assertFalse(idle.request(2_999_999_999)); assertTrue(idle.request(3_000_000_000))
    }
    @Test fun positionParsingAcceptsLongHoursAndMillisecondFractionsAndRejectsInvalidValues() {
        for ((value, expected) in mapOf("0:00:01" to 1000L, "01:02:03.4" to 3_723_400L,
            "123:59:59.045" to 446_399_045L, "00:00:00.01" to 10L, "00:00:00" to 0L))
            assertEquals(expected, rendererPositionMs(value), value)
        for (value in listOf(null, "", "NOT_IMPLEMENTED", "1:2:03", "-1:00:00", "00:60:00", "00:00:60",
            "00:00:01.", "00:00:01.0000", "00:00:01junk", "999999999999999999999:00:00", "2562047788016:00:00"))
            assertNull(rendererPositionMs(value), value)
        assertEquals(1500L, rendererLagMs(5_000_000_000, 1_000_000_000, "00:00:02.500"))
        assertEquals(0L, rendererLagMs(5_000_000_000, 1_000_000_000, "00:00:05"))
        assertNull(rendererLagMs(5_000_000_000, 1_000_000_000, "00:00:00"))
        assertNull(rendererLagMs(5_000_000_000, 1_000_000_000, "NOT_IMPLEMENTED"))
    }
}
