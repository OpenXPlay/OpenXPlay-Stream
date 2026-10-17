// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core

import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

public fun interface ClockTask {
    public fun cancel()
}

/** Scheduling must return promptly and must not invoke the task before its deadline. */
public interface MonotonicClock {
    public fun nowNanos(): Long
    public fun schedule(delayNanos: Long, task: () -> Unit): ClockTask

    public companion object {
        /** Shared daemon scheduler; monotonic timestamps have an arbitrary epoch. */
        public val system: MonotonicClock = object : MonotonicClock {
            private val timer = Executors.newSingleThreadScheduledExecutor { runnable ->
                Thread(runnable, "oxp-keyframes").apply { isDaemon = true }
            }
            override fun nowNanos(): Long = System.nanoTime()
            override fun schedule(delayNanos: Long, task: () -> Unit): ClockTask {
                val future = timer.schedule(Runnable { task() }, maxOf(0, delayNanos), TimeUnit.NANOSECONDS)
                return ClockTask { future.cancel(false) }
            }
        }
    }
}

/** Listener callbacks must return promptly. Listener failures do not affect the hub. */
public interface HubListener {
    public fun outputFailed(outputId: String, failure: Throwable): Unit = Unit
    public fun receiverDiagnostics(outputId: String, origin: Origin, type: String, json: String): Unit = Unit
    public fun stats(outputId: String, stats: OutputStats): Unit = Unit
    public fun inputFailed(failure: Throwable): Unit = Unit

    public companion object {
        public val NONE: HubListener = object : HubListener {}
    }
}
