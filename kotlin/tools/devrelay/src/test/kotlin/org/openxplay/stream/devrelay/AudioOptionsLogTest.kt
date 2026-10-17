// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14.

package org.openxplay.stream.devrelay

import java.io.ByteArrayOutputStream
import java.io.PrintStream
import java.nio.file.Files
import java.nio.file.Path
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.io.TempDir
import org.openxplay.stream.core.*
import org.openxplay.stream.replay.*
import kotlin.test.*

class AudioOptionsLogTest {
    @TempDir lateinit var directory: Path

    @Test
    fun parsesAllAudioOptionsAndKeepsExistingDefaults() {
        listOf("clip.h264", "media.aac", "voice.wav").forEach { Files.write(directory.resolve(it), byteArrayOf(1)) }
        val options = RelayOptions.parse(arrayOf("--clip", "clip.h264", "--audio-media", "media.aac",
            "--audio-voice", "voice.wav", "--audio-voice-every", "3.5", "--mic-rate", "16000",
            "--mic-out", "capture.wav", "--clink-pin", "0012", "--clink-head-unit", "192.168.1.2",
            "--clink-local", "192.168.1.3"), directory)
        assertEquals(directory.resolve("media.aac"), options.audio.media)
        assertEquals(directory.resolve("voice.wav"), options.audio.voice)
        assertEquals(3.5, options.audio.voiceEverySeconds)
        assertEquals(16000, options.audio.micRate)
        assertEquals(directory.resolve("capture.wav"), options.audio.micOut)
        assertEquals("0012", options.clinkPin)
        assertEquals(30.0, options.fps)
        val defaults = RelayOptions.parse(arrayOf("--clip", "clip.h264"), directory)
        assertEquals(ReplayAudioOptions(), defaults.audio)
        for (extra in listOf(arrayOf("--mic-out", "out.wav"), arrayOf("--mic-rate", "7999"),
            arrayOf("--audio-voice-every", "2"), arrayOf("--audio-media", "missing.aac"),
            arrayOf("--audio-voice", "voice.wav", "--audio-voice-every", "NaN"),
            arrayOf("--mic-rate", "16000", "--mic-rate", "16000"))) {
            assertFailsWith<IllegalArgumentException> {
                RelayOptions.parse(arrayOf("--clip", "clip.h264") + extra, directory)
            }
        }
    }

    @Test
    fun logsAudioFormatsStatsRequestsAndFiniteMicSummaries() {
        val bytes = ByteArrayOutputStream()
        val origin = Origin("web", "client")
        EventLog(PrintStream(bytes, true, Charsets.UTF_8)).use { log ->
            log.audioFormat(0, AudioFormat(0, AudioRole.MEDIA, AudioCodec.AAC_LC, 48000, 2,
                byteArrayOf(0x11, 0x90.toByte())))
            log.audioStats(0, 47, 1234)
            log.microphone(1, MicRequest(1, AudioCodec.PCM_S16LE, 16000, 1, 20))
            log.microphoneStats(MicSummary(50, origin, -6.0206))
            log.microphoneStats(MicSummary(1, origin, null))
            log.control(ControlEvent.MicPacket(1, ByteArray(640), 0, origin))
            log.audioFormat(0, null)
            log.microphone(1, null)
            log.stats("web", OutputStats(origin, ScreenId.MAIN, null, 0, 0, 0, null,
                audioBufferedUs = 60000, audioUnderruns = 2))
        }
        val text = bytes.toString(Charsets.UTF_8)
        assertContains(text, "\"event\":\"audio-format\"")
        assertContains(text, "\"config\":[17,144]")
        assertContains(text, "\"event\":\"audio-stats\",\"stream\":0,\"packets\":47,\"bytes\":1234")
        assertContains(text, "\"rmsDbfs\":-6.0206")
        assertContains(text, "\"rmsDbfs\":null")
        assertContains(text, "\"origin\":{\"outputId\":\"web\",\"clientId\":\"client\"}")
        assertContains(text, "\"format\":null")
        assertContains(text, "\"request\":null")
        assertContains(text, "\"audioBufferedUs\":60000,\"audioUnderruns\":2")
        assertFalse(text.contains("\"event\":\"control\""))
        assertEquals("{\"event\":\"shutdown\"}", text.trim().lines().last())
    }
}
