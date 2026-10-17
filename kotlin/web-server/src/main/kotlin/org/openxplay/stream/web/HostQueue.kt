// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

package org.openxplay.stream.web

import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.TimeUnit

/**
 * Runs host callbacks in order on one thread, so socket threads never call the host directly.
 *
 * Essential callbacks (capabilities, control events, restarts) are never dropped. Reports (stats and
 * diagnostics) are coalesced per key: a newer report replaces one still waiting and keeps its place.
 * While [capacity] callbacks wait, a report with a new key is dropped, so a stalled host accumulates
 * at most [capacity] reports however fast clients send them.
 */
internal class HostQueue(name: String, private val capacity: Int) {
    private val lock = Any()
    private val executor: ExecutorService = Executors.newSingleThreadExecutor { r -> Thread(r, name).apply { isDaemon = true } }
    private var waiting = 0
    private val reports = HashMap<Any, () -> Unit>()
    private var dropped = 0L

    init { require(capacity > 0) }

    /** Callbacks submitted and not yet finished. */
    val size: Int get() = synchronized(lock) { waiting }

    /** Reports discarded because the queue was full. */
    val droppedReports: Long get() = synchronized(lock) { dropped }

    fun essential(callback: () -> Unit) { submit(callback) }

    fun report(key: Any, callback: () -> Unit) {
        synchronized(lock) {
            if (key in reports) { reports[key] = callback; return }
            if (waiting >= capacity) { dropped++; return }
            reports[key] = callback
        }
        if (!submit { synchronized(lock) { reports.remove(key) }?.invoke() }) synchronized(lock) { reports.remove(key) }
    }

    /** Waits until every callback submitted so far has run. */
    fun awaitIdle(timeoutMillis: Long) {
        try { executor.submit {}.get(timeoutMillis, TimeUnit.MILLISECONDS) } catch (_: RejectedExecutionException) { }
    }

    /** Runs what is already queued, then stops the thread. */
    fun shutdown() { executor.shutdown() }

    private fun submit(callback: () -> Unit): Boolean {
        synchronized(lock) { waiting++ }
        return try {
            executor.execute {
                try { callback() } catch (_: Exception) { /* Host isolation: one failing callback must not stop the rest. */ }
                finally { synchronized(lock) { waiting-- } }
            }
            true
        } catch (_: RejectedExecutionException) {
            synchronized(lock) { waiting-- }
            false
        }
    }
}
