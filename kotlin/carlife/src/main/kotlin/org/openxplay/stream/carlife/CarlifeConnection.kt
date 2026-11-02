// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-11-02

package org.openxplay.stream.carlife

import java.io.Closeable
import java.io.InputStream
import java.io.OutputStream
import java.util.concurrent.atomic.AtomicBoolean

/** Each stream has one writer lock; USB multiplexes every channel under the same lock. */
internal class CarlifeConnection(
    val input: InputStream,
    val output: OutputStream,
    val resource: Closeable,
    val usb: Boolean,
    val channel: Int,
) : Closeable {
    val active = AtomicBoolean(true)
    private val writer = Any()
    @Volatile private var writingSince = 0L
    @Volatile private var writingChannel = 0

    fun stalled(now: Long): Boolean = writingSince.let { it != 0L && now - it >= 4_000_000_000L }
    fun stalledChannel(now: Long): Int? = if (stalled(now)) writingChannel else null

    fun write(message: CarlifeMessage): Boolean = synchronized(writer) {
        if (!active.get()) return false
        writingChannel = message.channel
        writingSince = System.nanoTime()
        try { CarlifeFraming.write(output, message, usb) } finally { writingSince = 0 }
        true
    }

    override fun close() {
        if (active.compareAndSet(true, false)) runCatching { resource.close() }
    }
}
