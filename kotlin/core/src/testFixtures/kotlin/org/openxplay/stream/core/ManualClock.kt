// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core

/** Deterministic monotonic clock. Advancing executes due tasks outside the clock lock. */
public class ManualClock(initialNanos: Long = 0) : MonotonicClock {
    private data class Task(val deadline: Long, val action: () -> Unit, var cancelled: Boolean = false)
    private val lock = Any()
    private var now = initialNanos
    private val tasks = mutableListOf<Task>()

    override fun nowNanos(): Long = synchronized(lock) { now }
    override fun schedule(delayNanos: Long, task: () -> Unit): ClockTask {
        require(delayNanos >= 0)
        val scheduled = synchronized(lock) { Task(now + delayNanos, task).also { tasks.add(it) } }
        return ClockTask { synchronized(lock) { scheduled.cancelled = true; tasks.remove(scheduled) } }
    }

    /** Runs every deadline up to the target, including tasks scheduled by other tasks. */
    public fun advanceNanos(delta: Long) {
        require(delta >= 0)
        val target = synchronized(lock) { Math.addExact(now, delta) }
        while (true) {
            val task = synchronized(lock) {
                val next = tasks.filter { !it.cancelled && it.deadline <= target }.minByOrNull { it.deadline }
                if (next == null) { now = target; null }
                else { tasks.remove(next); now = next.deadline; next }
            } ?: return
            if (!task.cancelled) task.action()
        }
    }
}
