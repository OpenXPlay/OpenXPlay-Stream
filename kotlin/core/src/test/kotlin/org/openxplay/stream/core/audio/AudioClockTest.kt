// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core.audio

import org.openxplay.stream.core.ManualClock
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith

class AudioClockTest {
    @Test
    fun carriesFractionalMicrosecondsAcrossPacketsWithoutAccumulatedRounding() {
        val clock = AudioClock(44100)
        val anchor = 987654L
        repeat(10000) { packet ->
            val expected = anchor + packet * 1024L * 1_000_000 / 44100
            assertEquals(expected, clock.timestamp(1024, expected + if (packet == 0) 0 else 3000))
        }
    }

    @Test
    fun preservesOneSecondBurstsAcrossStartsResumesAndTrackChanges() {
        val clock = AudioClock(16000)
        repeat(3) { run ->
            val start = run * 3_000_000L
            repeat(50) { assertEquals(start + it * 20_000, clock.timestamp(320, start)) }
            assertEquals(start + 1_000_000, clock.timestamp(320, start + 1_000_000))
        }
    }

    @Test
    fun reanchorsOnlyBeyondAsymmetricBoundsAndResetsFraction() {
        val clock = AudioClock(16000)
        assertEquals(0L, clock.timestamp(320, 0))
        assertEquals(20_000L, clock.timestamp(320, 120_000))
        assertEquals(40_000L, clock.timestamp(320, -1_460_000))
        assertEquals(-1_440_001L, clock.timestamp(320, -1_440_001))
        assertEquals(0L, clock.timestamp(320, 0))
        val fractional = AudioClock(44100)
        assertEquals(0L, fractional.timestamp(1, 0))
        assertEquals(200000L, fractional.timestamp(1, 200000))
        assertEquals(200022L, fractional.timestamp(1, 200022))
        assertEquals(200045L, fractional.timestamp(1, 200045))
        fractional.reset()
        assertEquals(42L, fractional.timestamp(1, 42))
    }

    @Test
    fun usesRelayMonotonicClockAndChecksInvalidCountsAndOverflow() {
        val relay = ManualClock(1_000_000)
        val clock = AudioClock(16000, relay)
        assertEquals(1000L, clock.timestamp(320))
        relay.advanceNanos(20_000_000)
        assertEquals(21000L, clock.timestamp(320))
        clock.reset()
        assertEquals(-50000L, clock.timestamp(320, -50000))
        assertFailsWith<IllegalArgumentException> { clock.timestamp(0, 0) }
        assertFailsWith<IllegalArgumentException> { AudioClock(0) }
        assertFailsWith<ArithmeticException> { clock.timestamp(320, Long.MAX_VALUE) }
    }
}
