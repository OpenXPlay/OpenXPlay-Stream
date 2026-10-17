// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core

import org.junit.jupiter.api.Assumptions.assumeTrue
import org.junit.jupiter.api.Test
import org.openxplay.stream.core.codec.AvcConfig
import org.openxplay.stream.core.codec.Nal
import org.openxplay.stream.core.codec.TsMuxer
import java.io.ByteArrayOutputStream
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.TimeUnit
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class TsMuxerIntegrationTest {
    @Test
    fun baselineClipIsRecognizedAndAllFramesDecodeWithFfprobe() {
        val ffprobe = Path.of("/opt/homebrew/bin/ffprobe")
        assumeTrue(Files.isExecutable(ffprobe), "ffprobe is absent")
        val clip = checkNotNull(javaClass.getResourceAsStream("/media/baseline-640x360.h264"))
            .use { it.readBytes() }
        assertTrue(clip.size < 200_000)
        val units = Nal.units(clip, Nal.Format.ANNEX_B).toList()
        val sps = units.first { type(it) == 7 }
        val pps = units.first { type(it) == 8 }
        val config = avcConfig(sps, pps)
        // The offline fixture has AUDs, so access-unit boundaries are unambiguous.
        val frames = mutableListOf<MutableList<ByteArray>>()
        for (nal in units) {
            if (type(nal) == 9) frames.add(mutableListOf())
            else if (type(nal) !in listOf(7, 8)) frames.last().add(nal)
        }
        assertEquals(60, frames.size)
        val directory = Files.createTempDirectory("openxplay-ts-")
        val transport = directory.resolve("baseline.ts")
        val report = directory.resolve("ffprobe.txt")
        try {
            Files.newOutputStream(transport).use { output ->
                val muxer = TsMuxer({ output.write(it) })
                for ((index, frame) in frames.withIndex()) {
                    val annexB = ByteArrayOutputStream()
                    for (nal in frame) {
                        annexB.write(byteArrayOf(0, 0, 0, 1))
                        annexB.write(nal)
                    }
                    muxer.writeVideo(Nal.annexBToAvcc(annexB.toByteArray()), config,
                        index * 1_000_000L / 30, frame.any { type(it) == 5 })
                }
            }
            val process = ProcessBuilder(ffprobe.toString(), "-v", "error", "-select_streams", "v:0",
                "-count_frames", "-show_entries", "stream=codec_name,width,height,nb_read_frames",
                "-of", "default=noprint_wrappers=1", transport.toString())
                .redirectErrorStream(true).redirectOutput(report.toFile()).start()
            try {
                assertTrue(process.waitFor(30, TimeUnit.SECONDS), "ffprobe timed out")
                val text = Files.readString(report)
                assertEquals(0, process.exitValue(), text)
                val lines = text.lineSequence().filter { it.isNotBlank() }.toList()
                val expected = setOf("codec_name=h264", "width=640", "height=360", "nb_read_frames=60")
                assertEquals(expected, lines.toSet(), text)
            } finally {
                if (process.isAlive) {
                    process.destroyForcibly()
                    process.waitFor(5, TimeUnit.SECONDS)
                }
            }
        } finally {
            Files.deleteIfExists(report)
            Files.deleteIfExists(transport)
            Files.deleteIfExists(directory)
        }
    }

    private fun type(nal: ByteArray): Int = u8(nal[0]) and 31

    private fun avcConfig(sps: ByteArray, pps: ByteArray): AvcConfig {
        val record = ByteArrayOutputStream()
        record.write(byteArrayOf(1, sps[1], sps[2], sps[3], 0xff.toByte(), 0xe1.toByte()))
        fun parameterSet(nal: ByteArray) {
            record.write(nal.size ushr 8)
            record.write(nal.size and 0xff)
            record.write(nal)
        }
        parameterSet(sps)
        record.write(1)
        parameterSet(pps)
        return AvcConfig.parse(record.toByteArray())
    }
}
