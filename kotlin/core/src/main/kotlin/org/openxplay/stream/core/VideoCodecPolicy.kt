// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

package org.openxplay.stream.core

public data class VideoCodecDecision(val codec: VideoCodec, val reason: String)

public object VideoCodecPolicy {
    public fun isHevc(codec: String): Boolean = codec.startsWith("hvc1.") || codec.startsWith("hev1.")

    public fun resolve(outputs: List<OutputCapabilities>, requiresAvc: Boolean = false): VideoCodecDecision {
        val viewers = outputs.filter { ScreenId.MAIN in it.displays }
        val reason = when {
            requiresAvc -> "output-requires-avc"
            viewers.isEmpty() -> "no-main-screen-output"
            viewers.any { it.prefersH264 } -> "main-screen-output-prefers-h264"
            viewers.any { output -> output.decoders.none { it.hardware && isHevc(it.codecString) } } -> "hardware-hevc-unavailable"
            else -> "all-main-screen-outputs-hardware-hevc"
        }
        return VideoCodecDecision(if (reason == "all-main-screen-outputs-hardware-hevc") VideoCodec.H265 else VideoCodec.H264, reason)
    }
}

/** Two automatic restarts per burst; a full two minutes without a decision change re-arms it. */
public class VideoCodecRestartLimiter {
    private var lastRestart: Long? = null
    private var lastChange: Long? = null
    private var count = 0
    public fun changed(nowMs: Long): Unit {
        if (lastChange?.let { nowMs - it >= 120_000 } == true) count = 0
        lastChange = nowMs
    }
    public fun allow(nowMs: Long): Boolean {
        if (count >= 2 || lastRestart?.let { nowMs - it < 30_000 } == true) return false
        count++
        lastRestart = nowMs
        return true
    }
}
