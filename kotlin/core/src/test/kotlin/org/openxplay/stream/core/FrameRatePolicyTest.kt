// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

package org.openxplay.stream.core

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class FrameRatePolicyTest {
    @Test
    fun preferenceAndMeasurementTable() {
        val measurements = listOf(
            Triple(null, null, 60.0),
            Triple(60.0, null, 60.0),
            Triple(null, 60.0, 60.0),
            Triple(49.9, null, 30.0),
            Triple(null, 49.9, 30.0),
            Triple(49.9, 60.0, 30.0),
            Triple(60.0, 49.9, 30.0),
            Triple(30.0, 30.0, 30.0),
            Triple(50.0, 50.0, 60.0),
            Triple(120.0, 60.0, 60.0),
            Triple(Double.NaN, Double.NaN, 60.0),
        )
        for (preference in FrameRatePreference.entries) {
            for ((refreshHz, sustainedFps, automatic) in measurements) {
                val expected = when (preference) {
                    FrameRatePreference.AUTO -> automatic
                    FrameRatePreference.FPS_30 -> 30.0
                    FrameRatePreference.FPS_35 -> 35.0
                    FrameRatePreference.FPS_40 -> 40.0
                    FrameRatePreference.FPS_45 -> 45.0
                    FrameRatePreference.FPS_50 -> 50.0
                    FrameRatePreference.FPS_55 -> 55.0
                    FrameRatePreference.FPS_60 -> 60.0
                }
                assertEquals(expected, FrameRatePolicy.resolve(preference, refreshHz, sustainedFps),
                    "$preference, refreshHz=$refreshHz, sustainedFps=$sustainedFps")
            }
        }
        assertEquals(60.0, FrameRatePolicy.resolve())
    }

    @Test
    fun displayDefaultsAndMissingDisplayFallBackTo60() {
        val display = DisplayInfo(800, 480, 160, 60.0)
        assertEquals(FrameRatePreference.AUTO, display.frameRate)
        assertNull(display.sustainedFps)
        val config = DefaultSessionPolicy.derive(FakeInput().capabilities, emptyList())
        assertEquals(60.0, config.screens[ScreenId.MAIN]!!.fps)
    }

    @Test
    fun sessionPolicyUsesPrimaryDisplayPreferenceAndSustainedRate() {
        fun output(latency: LatencyClass, display: DisplayInfo): OutputCapabilities = OutputCapabilities(
            latency, emptyList(), mapOf(ScreenId.MAIN to display), 0, emptySet(), emptySet(), false,
        )
        val display = DisplayInfo(800, 480, 160, 120.0)
        val buffered = output(LatencyClass.BUFFERED, display.copy(frameRate = FrameRatePreference.FPS_60))
        val secondary = output(LatencyClass.INTERACTIVE, display.copy(frameRate = FrameRatePreference.FPS_60))
        for ((primaryDisplay, expected) in listOf(
            display.copy(frameRate = FrameRatePreference.FPS_30) to 30.0,
            display.copy(refreshHz = 30.0, frameRate = FrameRatePreference.FPS_60) to 60.0,
            display.copy(sustainedFps = 45.0) to 30.0,
            display to 60.0,
        )) {
            val primary = output(LatencyClass.INTERACTIVE, primaryDisplay)
            val config = DefaultSessionPolicy.derive(FakeInput().capabilities, listOf(buffered, primary, secondary))
            assertEquals(expected, config.screens[ScreenId.MAIN]!!.fps)
        }
        val config = DefaultSessionPolicy.derive(FakeInput().capabilities,
            listOf(output(LatencyClass.BUFFERED, display.copy(frameRate = FrameRatePreference.FPS_30))))
        assertEquals(30.0, config.screens[ScreenId.MAIN]!!.fps)
    }
}
