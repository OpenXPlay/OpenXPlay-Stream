// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-11-02

package org.openxplay.stream.cast

/** Serialized with the media inbox; the limiter acknowledges a request only when it is sent. */
internal class CastIdleKeyframe(private val idleNanos: Long, private val intervalNanos: Long) {
    private var lastVideo: Long? = null
    private var lastRequest: Long? = null
    private var changed = false
    private var pending = false
    fun video(keyframe: Boolean, now: Long) { lastVideo = now; if (!keyframe) changed = true }
    fun request(now: Long): Boolean {
        val last = lastVideo ?: return false
        if (pending || !changed || now - last < idleNanos || lastRequest?.let { now - it < intervalNanos } == true) return false
        pending = true; return true
    }
    fun sent(now: Long) { if (pending) { lastRequest = now; changed = false; pending = false } }
}

internal fun rendererPositionMs(value: String?): Long? {
    val match = value?.let { Regex("([0-9]+):([0-5][0-9]):([0-5][0-9])(?:\\.([0-9]{1,3}))?").matchEntire(it) } ?: return null
    val hours = match.groupValues[1].toLongOrNull() ?: return null
    val remainder = match.groupValues[2].toLong() * 60_000 + match.groupValues[3].toLong() * 1000 +
        match.groupValues[4].padEnd(3, '0').toLong()
    if (hours > (Long.MAX_VALUE - remainder) / 3_600_000) return null
    return hours * 3_600_000 + remainder
}

internal fun rendererLagMs(now: Long, start: Long, position: String?): Long? {
    val elapsed = rendererPositionMs(position)?.takeIf { it > 0 } ?: return null
    return ((now - start) / 1_000_000 - elapsed).coerceAtLeast(0)
}
