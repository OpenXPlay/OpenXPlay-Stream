// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.clink

/** TCP services; use distinct loopback ports in tests. */
public data class ClinkPorts(
    public val auth: Int = 57209,
    public val control: Int = 57219,
    public val media: Int = 57229,
    public val cert: Int = 57249,
    public val rtsp: Int = 7236,
    public val uibc: Int = 4321,
    public val rtp: Int = 15550,
    /** Phone-side reverse connection for logical channel 8, category 2 (SENSOR). */
    public val sensor: Int = 57239,
) {
    init {
        val services = listOf(auth, control, media, cert, rtsp, uibc, rtp, sensor)
        require(services.all { it in 1..65535 } && services.distinct().size == services.size)
    }
}

public enum class ClinkState { CONNECTING, AUTHENTICATING, NEGOTIATING, PLAYING, ENDED }

/** Callbacks run on workers. Implementations must return promptly and must not throw. */
public interface ClinkListener {
    public fun state(state: ClinkState): Unit = Unit
    public fun error(error: Throwable): Unit = Unit
    public fun diagnostic(message: String): Unit = Unit
    /** SENSOR connectivity and cumulative malformed-frame count for this session. */
    public fun vehicleState(sensorActive: Boolean, malformedFrames: Long): Unit = Unit
    public fun statistics(stats: ClinkStatistics): Unit = Unit
    public fun profileMismatch(inputCodec: String, decoders: List<String>): Unit = Unit
}
