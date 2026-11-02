// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-11-02

package org.openxplay.stream.carlife

public data class CarlifeConfig(
    public val bindAddress: String = "0.0.0.0",
    public val targetAddress: String? = null,
    public val tcp: Boolean = true,
    public val discovery: Boolean = true,
    public val maxWidth: Int = 1920,
    public val maxHeight: Int = 1080,
    public val fps: Int = 30,
    public val ports: List<Int> = listOf(7240, 8240, 9240, 9241, 9242, 9340, 9440),
) {
    init {
        require(maxWidth in 16..1920 && maxHeight in 16..1080 && fps in 1..60)
        require(ports.size == 7 && ports.all { it in 0..65535 })
        require(ports.filter { it != 0 }.distinct().size == ports.count { it != 0 })
    }
}

public data class CarlifeDeviceInfo(
    public val brand: String = "OpenXPlay",
    public val model: String = "Relay",
    public val sdk: String = "1",
)

public enum class CarlifeState { CLOSED, LISTENING, CONNECTED, AUTHENTICATED, STREAMING, FAILED }

/** Frames count completed socket writes, not confirmed head-unit presentation. */
public data class CarlifeStatus(
    public val state: CarlifeState,
    public val peer: String? = null,
    public val width: Int = 0,
    public val height: Int = 0,
    public val fps: Int = 0,
    public val frames: Long = 0,
    public val reason: String? = null,
)

public fun interface CarlifeListener {
    public fun onStatus(status: CarlifeStatus)
    /** Protocol metadata only; payloads and authentication material are never included. */
    public fun diagnostic(event: String): Unit = Unit
}
