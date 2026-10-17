// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core

/** Minimum-RTT NTP-style estimate over the most recent [windowSize] pong samples. All units are nanoseconds. */
public class ClockSync(public val windowSize: Int = 16) {
    public data class Estimate(public val offsetNanos: Double, public val rttNanos: Long)
    private val lock = Any()
    private val samples = ArrayDeque<Estimate>()

    init { require(windowSize > 0) }

    /** Offset is remote minus local; subtract it from remote timestamps to obtain local time. */
    public fun pong(localSent: Long, remoteReceived: Long, remoteSent: Long, localReceived: Long): Estimate {
        require(localReceived >= localSent && remoteSent >= remoteReceived)
        val rtt = Math.subtractExact(Math.subtractExact(localReceived, localSent), Math.subtractExact(remoteSent, remoteReceived))
        require(rtt >= 0) { "Remote processing exceeds round-trip time" }
        val sample = Estimate((Math.subtractExact(remoteReceived, localSent).toDouble() +
            Math.subtractExact(remoteSent, localReceived).toDouble()) / 2.0, rtt)
        return synchronized(lock) {
            samples.addLast(sample)
            if (samples.size > windowSize) samples.removeFirst()
            samples.minBy { it.rttNanos }
        }
    }

    public val estimate: Estimate? get() = synchronized(lock) { samples.minByOrNull { it.rttNanos } }
    public fun clear(): Unit = synchronized(lock) { samples.clear() }
}
