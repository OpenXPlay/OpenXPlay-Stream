// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core

/** Pure time-driven coalescer; callers serialize access and arrange wakeups at [nextDeadlineNanos]. */
public class KeyframeLimiter(public val intervalNanos: Long = 250_000_000L) {
    private data class Slot(var sent: Long, var pending: ControlEvent.KeyFrameRequest? = null)
    private val slots = mutableMapOf<ScreenId, Slot>()

    init { require(intervalNanos > 0) }

    /** Returns requests due now, including any older pending request for other screens. */
    public fun request(event: ControlEvent.KeyFrameRequest, nowNanos: Long): List<ControlEvent.KeyFrameRequest> {
        val due = poll(nowNanos).toMutableList()
        val slot = slots[event.screen]
        if (slot == null || nowNanos - slot.sent >= intervalNanos) {
            slots[event.screen] = Slot(nowNanos)
            due.add(event)
        } else {
            slot.pending = event
        }
        return due
    }

    public fun poll(nowNanos: Long): List<ControlEvent.KeyFrameRequest> {
        val due = mutableListOf<ControlEvent.KeyFrameRequest>()
        for (slot in slots.values) {
            val pending = slot.pending
            if (pending != null && nowNanos - slot.sent >= intervalNanos) {
                slot.sent = nowNanos
                slot.pending = null
                due.add(pending)
            }
        }
        return due
    }

    public val nextDeadlineNanos: Long?
        get() = slots.values.filter { it.pending != null }.minOfOrNull { it.sent + intervalNanos }

    public fun clear(): Unit = slots.clear()
}
