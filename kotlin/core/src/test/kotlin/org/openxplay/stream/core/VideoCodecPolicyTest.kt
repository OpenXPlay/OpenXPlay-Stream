// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

package org.openxplay.stream.core

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue
import kotlin.test.assertFalse

class VideoCodecPolicyTest {
    private fun output(codec: String = "hvc1.1.6.L120.B0", hardware: Boolean = true) = OutputCapabilities(
        LatencyClass.INTERACTIVE, listOf(DecoderSupport(codec, hardware)),
        mapOf(ScreenId.MAIN to DisplayInfo(1280, 720, 160, 60.0)), 0, emptySet(), emptySet(), false)

    @Test fun requiresEveryHardwareReceiver() {
        val hevc = output()
        assertEquals(VideoCodec.H264, VideoCodecPolicy.resolve(emptyList()).codec)
        assertEquals(VideoCodec.H265, VideoCodecPolicy.resolve(listOf(hevc, output("hev1.1.6.L120.B0"))).codec)
        for (other in listOf(output(hardware = false), output("avc1.42E01F"), hevc.copy(decoders = emptyList()))) {
            assertEquals(VideoCodec.H264, VideoCodecPolicy.resolve(listOf(hevc, other)).codec)
        }
        assertEquals(VideoCodec.H264, VideoCodecPolicy.resolve(listOf(hevc), requiresAvc = true).codec)
        assertEquals(VideoCodec.H264, VideoCodecPolicy.resolve(listOf(hevc.copy(displays = emptyMap()))).codec)
    }
    @Test fun limitsFlappingAndRearmsAfterStability() {
        val limiter = VideoCodecRestartLimiter()
        limiter.changed(0)
        assertTrue(limiter.allow(0))
        limiter.changed(1_000)
        assertFalse(limiter.allow(29_999))
        assertTrue(limiter.allow(30_000))
        limiter.changed(31_000)
        assertFalse(limiter.allow(60_000))
        limiter.changed(151_000)
        assertTrue(limiter.allow(151_000))
    }
}
