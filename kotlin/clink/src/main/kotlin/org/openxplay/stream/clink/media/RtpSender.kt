// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

package org.openxplay.stream.clink.media

import java.util.concurrent.locks.ReentrantLock
import kotlin.concurrent.withLock

/**
 * Hands one [RtpUnit] at a time from the media worker to a dedicated socket writer, so the media worker
 * waits at most [waitNs] for a stalled TCP send instead of blocking until the session watchdog closes
 * the socket. The media worker refills [unit] only while [idle]; while a late write is still in flight
 * it leaves media in its bounded queues, which drop to the next keyframe (video) or drop oldest (audio).
 * Exactly one unit is ever retained here. The writer's own wait stays bounded by the session watchdog.
 */
internal class RtpSender(val waitNs: Long = DEFAULT_WAIT_NS, val unit: RtpUnit = RtpUnit()) {
    private val lock = ReentrantLock()
    private val changed = lock.newCondition()
    private var job: ((ByteArray, Int, Int) -> Boolean)? = null
    private var submittedAt = 0L
    private var late = false
    private var recoveredAfterNs = -1L
    private var failure: Throwable? = null
    init { require(waitNs > 0) }

    /** Media worker: true when no write is in flight, so [unit] may be refilled. Rethrows a writer failure. */
    fun idle(): Boolean = lock.withLock { rethrow(); job == null }

    /**
     * Media worker: hands the filled [unit] to the writer and waits up to [waitNs]. True when it was written
     * (or [send] declined part of it, as before); false when the write is still in flight.
     */
    fun submit(send: (ByteArray, Int, Int) -> Boolean): Boolean = lock.withLock {
        check(job == null) { "rtp_write_in_flight" }
        if (unit.size == 0) return true
        job = send; submittedAt = System.nanoTime(); changed.signalAll()
        var remaining = waitNs
        while (job != null && remaining > 0) remaining = changed.awaitNanos(remaining)
        if (job != null) { late = true; return false }
        rethrow()
        true
    }

    /** Media worker: once a late write has finished, how long it took in total; otherwise null. */
    fun takeRecovered(): Long? = lock.withLock {
        if (recoveredAfterNs < 0) null else recoveredAfterNs.also { recoveredAfterNs = -1 }
    }

    /** Snapshot for diagnostics and tests: bytes held by the reusable unit, and whether a write is in flight. */
    fun retained(): Pair<Int, Boolean> = lock.withLock { unit.bytes.size to (job != null) }

    /** Writer thread: writes each submitted unit, one framed packet per socket write, until [active] is false. */
    fun run(active: () -> Boolean) {
        while (true) {
            val send = lock.withLock {
                while (job == null) {
                    if (!active()) return
                    changed.awaitNanos(IDLE_POLL_NS)
                }
                checkNotNull(job)
            }
            var error: Throwable? = null
            try {
                unit.forEachPacket { bytes, offset, length -> send(bytes, offset, length) }
            } catch (e: Throwable) { error = e }
            lock.withLock {
                job = null
                if (error != null && failure == null) failure = error
                if (late) { late = false; recoveredAfterNs = System.nanoTime() - submittedAt }
                changed.signalAll()
            }
        }
    }

    private fun rethrow() { failure?.let { failure = null; throw it } }

    companion object {
        /**
         * Longer than one large IDR takes on a slow link, and between VideoQueue's 600 ms and the audio
         * queue's 1.5 s, so short Wi-Fi dips behave exactly as a blocking send did.
         */
        const val DEFAULT_WAIT_NS: Long = 1_000_000_000
        private const val IDLE_POLL_NS = 100_000_000L
    }
}
