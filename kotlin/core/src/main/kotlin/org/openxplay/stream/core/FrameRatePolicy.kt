// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

package org.openxplay.stream.core

/** Pure frame-rate selection; unknown measurements fall back to 60 FPS. */
public object FrameRatePolicy {
    public fun resolve(
        preference: FrameRatePreference = FrameRatePreference.AUTO,
        refreshHz: Double? = null,
        sustainedFps: Double? = null,
    ): Double = when (preference) {
        FrameRatePreference.FPS_30 -> 30.0
        FrameRatePreference.FPS_35 -> 35.0
        FrameRatePreference.FPS_40 -> 40.0
        FrameRatePreference.FPS_45 -> 45.0
        FrameRatePreference.FPS_50 -> 50.0
        FrameRatePreference.FPS_55 -> 55.0
        FrameRatePreference.FPS_60 -> 60.0
        FrameRatePreference.AUTO -> if ((refreshHz != null && refreshHz < 50.0) ||
            (sustainedFps != null && sustainedFps < 50.0)) 30.0 else 60.0
    }
}
