// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-22

package org.openxplay.stream.devrelay

import java.nio.file.Files
import java.nio.file.Path
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.io.TempDir
import org.openxplay.stream.core.*
import kotlin.test.*

class CastOptionsTest {
    @TempDir lateinit var directory: Path
    @Test fun searchDoesNotRequireAClipAndUsageDocumentsBothOptions() {
        val options = RelayOptions.parse(arrayOf("--cast-search"), directory)
        assertTrue(options.castSearch); assertNull(options.clip); assertNull(options.castRenderer)
        assertContains(RelayOptions.USAGE, "--cast-search"); assertContains(RelayOptions.USAGE, "--cast-renderer <udn>")
        assertContains(RelayOptions.USAGE, "--clip <annex-b.h264|annex-b.h265>")
    }
    @Test fun rendererKeepsClipAndAudioOptionsAndDefaultsRemainOff() {
        Files.write(directory.resolve("clip.h264"), byteArrayOf(1)); Files.write(directory.resolve("media.aac"), byteArrayOf(1))
        val options = RelayOptions.parse(arrayOf("--clip", "clip.h264", "--cast-renderer", "uuid:test", "--audio-media", "media.aac"), directory)
        assertEquals("uuid:test", options.castRenderer); assertFalse(options.castSearch)
        assertEquals(directory.resolve("media.aac"), options.audio.media)
        val defaults = RelayOptions.parse(arrayOf("--clip", "clip.h264"), directory)
        assertFalse(defaults.castSearch); assertNull(defaults.castRenderer)
        for (args in listOf(arrayOf("--cast-renderer", "uuid:test"), arrayOf("--cast-search", "--cast-search"),
            arrayOf("--cast-search", "--cast-renderer"), arrayOf("--clip", "clip.h264", "--cast-renderer", " ")))
            assertFailsWith<IllegalArgumentException> { RelayOptions.parse(args, directory) }
    }
    @Test fun replayKeepsTheClipCodecBeforeAndAfterCastAttaches() {
        val cast = OutputCapabilities(LatencyClass.BUFFERED,
            listOf(DecoderSupport("avc1.64002a", true), DecoderSupport("hvc1.1.6.L150", true)),
            mapOf(ScreenId.MAIN to DisplayInfo(1920, 1080, 160, 60.0, frameRate = FrameRatePreference.FPS_60)),
            0, emptySet(), setOf(AudioCodec.AAC_LC), false, prefersH264 = true)
        for (codec in VideoCodec.entries) {
            val input = InputCapabilities(setOf(codec), setOf(ScreenId.MAIN), true, emptySet(), false, true, ReconfigureCost.NONE)
            for (outputs in listOf(emptyList(), listOf(cast))) {
                val expected = DefaultSessionPolicy.derive(input.copy(codecs = setOf(VideoCodec.H264)), outputs)
                    .screens.getValue(ScreenId.MAIN).copy(codec = codec)
                assertEquals(expected, ReplaySessionPolicy.derive(input, outputs).screens.getValue(ScreenId.MAIN))
            }
        }
        assertEquals(VideoCodec.H264, VideoCodecPolicy.resolve(listOf(cast)).codec)
    }
}
