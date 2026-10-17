// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core

/** Pure, non-blocking negotiation. Outputs are listed in insertion order. */
public fun interface SessionPolicy {
    public fun derive(input: InputCapabilities, outputs: List<OutputCapabilities>): SessionConfig
}

/** The first interactive display owns size and FPS. Buffered displays are a fallback. */
public object DefaultSessionPolicy : SessionPolicy {
    override fun derive(input: InputCapabilities, outputs: List<OutputCapabilities>): SessionConfig {
        val primary = outputs.firstOrNull { it.latency == LatencyClass.INTERACTIVE && it.displays.isNotEmpty() }
        val screens = input.screens.associateWith { screen ->
            val viewers = outputs.filter { screen in it.displays }
            val display = primary?.displays?.get(screen) ?: viewers.firstOrNull()?.displays?.get(screen)
            val hevc = if (screen == ScreenId.MAIN) VideoCodecPolicy.resolve(outputs).codec == VideoCodec.H265
                else viewers.isNotEmpty() && viewers.all { output -> output.decoders.any { it.hardware && VideoCodecPolicy.isHevc(it.codecString) } }
            val codec = when {
                hevc && VideoCodec.H265 in input.codecs -> VideoCodec.H265
                VideoCodec.H264 in input.codecs -> VideoCodec.H264
                else -> error("Input must support H.264 fallback unless every viewer supports hardware H.265")
            }
            ScreenConfig(codec, display?.widthPx ?: 1280, display?.heightPx ?: 720,
                FrameRatePolicy.resolve(display?.frameRate ?: FrameRatePreference.AUTO, display?.refreshHz, display?.sustainedFps))
        }
        return SessionConfig(screens)
    }
}
