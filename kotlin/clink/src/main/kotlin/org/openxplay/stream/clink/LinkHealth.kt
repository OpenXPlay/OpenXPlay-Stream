// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

package org.openxplay.stream.clink

/** Monotonic receive clock, independent of heartbeat transmission and media writes. */
internal class LinkHealth(private var lastReceived: Long) {
    companion object {
        const val RECEIVE_TIMEOUT_NS: Long = 15_000_000_000
        const val STALL_THRESHOLD_NS: Long = 5_000_000_000
        const val HEARTBEAT_INTERVAL_NS: Long = 2_000_000_000
    }
    private var stalled = false
    fun poll(now: Long): String? {
        val elapsed = now - lastReceived
        check(elapsed <= RECEIVE_TIMEOUT_NS) { "heartbeat_timeout" }
        if (elapsed >= STALL_THRESHOLD_NS && !stalled) {
            stalled = true
            return "link stalled ms=${elapsed / 1_000_000}"
        }
        return null
    }
    fun received(now: Long): String? {
        val elapsed = now - lastReceived
        lastReceived = now
        if (stalled) {
            stalled = false
            return "link recovered after ms=${elapsed / 1_000_000}"
        }
        return null
    }
}
