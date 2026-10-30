// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

package org.openxplay.stream.web

import java.io.OutputStream
import java.util.concurrent.atomic.AtomicLong

/**
 * Java socket writes have no timeout. This stream splits writes into chunks and records when the
 * pending chunk started, so a watchdog can tell a stalled peer from a large frame that is still moving:
 * every chunk that completes is progress.
 */
internal class ProgressOutputStream(private val out: OutputStream, private val chunkBytes: Int = 16 * 1024) : OutputStream() {
    val bytesSent = AtomicLong()
    private val blockedSince = AtomicLong(IDLE)

    init { require(chunkBytes > 0) }

    /** How long the pending write has made no progress at [now] (System.nanoTime), or zero when idle. */
    fun stalledNanos(now: Long): Long {
        val since = blockedSince.get()
        return if (since == IDLE) 0 else (now - since).coerceAtLeast(0)
    }

    override fun write(b: Int) { track { out.write(b); bytesSent.incrementAndGet() } }

    override fun write(b: ByteArray, off: Int, len: Int) {
        java.util.Objects.checkFromIndexSize(off, len, b.size)
        var offset = off
        val end = off + len
        while (offset < end) {
            val n = minOf(chunkBytes, end - offset)
            track { out.write(b, offset, n); bytesSent.addAndGet(n.toLong()) }
            offset += n
        }
    }

    override fun flush() { track { out.flush() } }

    override fun close() { out.close() }

    private inline fun track(write: () -> Unit) {
        blockedSince.set(System.nanoTime())
        try { write() } finally { blockedSince.set(IDLE) }
    }

    private companion object { const val IDLE = Long.MIN_VALUE }
}
