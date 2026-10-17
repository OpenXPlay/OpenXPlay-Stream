// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.web

import java.util.concurrent.TimeUnit

/** Rolling failures and a fixed cooldown, keyed by address without the ephemeral port. */
internal class HelloRateLimit {
    private data class Attempts(val failures: ArrayDeque<Long> = ArrayDeque(), var blockedUntil: Long = 0)
    private val addresses = mutableMapOf<String, Attempts>()
    private val window = TimeUnit.SECONDS.toNanos(60)

    @Synchronized fun allow(address: String, now: Long = System.nanoTime(), verify: () -> Boolean): Boolean {
        addresses.entries.removeIf { (_, attempts) ->
            while (attempts.failures.isNotEmpty() && now - attempts.failures.first() >= window) attempts.failures.removeFirst()
            attempts.blockedUntil <= now && attempts.failures.isEmpty()
        }
        val attempts = addresses[address]
        if (attempts != null && now < attempts.blockedUntil) return false
        if (verify()) return true
        val failed = addresses.getOrPut(address) { Attempts() }
        failed.failures.addLast(now)
        if (failed.failures.size >= 5) failed.blockedUntil = now + window
        return false
    }
}
