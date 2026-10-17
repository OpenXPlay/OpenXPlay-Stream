// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core.audio

import org.openxplay.stream.core.MonotonicClock

/** One sample-accurate stream run on the relay clock. Calls must be serialized. */
public class AudioClock(
    public val sampleRate: Int,
    private val clock: MonotonicClock = MonotonicClock.system,
) {
    private var nextUs: Long? = null
    private var remainder = 0L

    init { require(sampleRate > 0) { "Invalid audio sample rate" } }

    /** Samples are frames per channel. Allow buffered runs up to 1.5 s ahead; recover when more than 100 ms late. */
    public fun timestamp(samples: Int, arrivalUs: Long = clock.nowNanos() / 1000): Long {
        require(samples > 0) { "Audio packets require samples" }
        val expected = nextUs
        val drifted = expected != null &&
            (if (arrivalUs >= expected) arrivalUs.toULong() - expected.toULong() > 100_000uL
            else expected.toULong() - arrivalUs.toULong() > 1_500_000uL)
        val pts = if (expected == null || drifted) arrivalUs else expected
        val fraction = samples.toLong() * 1_000_000 + if (expected == null || drifted) 0 else remainder
        val next = Math.addExact(pts, fraction / sampleRate)
        remainder = fraction % sampleRate
        nextUs = next
        return pts
    }

    /** Call when a format ends or a new format starts a run. */
    public fun reset() { nextUs = null; remainder = 0 }
}
