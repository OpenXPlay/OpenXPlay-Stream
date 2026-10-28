// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.replay

import java.nio.file.Files
import java.nio.file.Path

/** Inspects an Annex B H.264/H.265 file; relay shells are responsible for wiring outputs. */
public object ReplayMain {
    @JvmStatic
    public fun main(args: Array<String>): Unit {
        require(args.isNotEmpty()) { "Usage: replay <annex-b.h264|annex-b.h265> [--audio-media aac] [--audio-voice wav] [--audio-voice-every seconds] [--mic-rate hz] [--mic-out wav]" }
        val values = mutableMapOf<String, String>()
        var index = 1
        while (index < args.size) {
            val flag = args[index++]
            require(flag in setOf("--audio-media", "--audio-voice", "--audio-voice-every", "--mic-rate", "--mic-out")) { "Unknown option: $flag" }
            require(flag !in values && index < args.size && !args[index].startsWith("--")) { "Duplicate option or missing value: $flag" }
            values[flag] = args[index++]
        }
        val audio = ReplayAudioOptions(values["--audio-media"]?.let { Path.of(it) },
            values["--audio-voice"]?.let { Path.of(it) }, values["--audio-voice-every"]?.toDouble(),
            values["--mic-rate"]?.toInt(), values["--mic-out"]?.let { Path.of(it) })
        val clip = AnnexBClip.parse(Files.readAllBytes(Path.of(args[0])))
        println("${clip.format.codecString} ${clip.format.width}x${clip.format.height} ${clip.accessUnits.size} frames")
        audio.media?.let { println("AAC-LC 48000 Hz stereo ${AdtsClip.parse(Files.readAllBytes(it)).units.size} access units") }
        audio.voice?.let { path ->
            val voice = WavClip.parse(Files.readAllBytes(path))
            println("PCM S16LE ${voice.sampleRate} Hz ${voice.channels} channels ${voice.frames} sample frames")
        }
    }
}
