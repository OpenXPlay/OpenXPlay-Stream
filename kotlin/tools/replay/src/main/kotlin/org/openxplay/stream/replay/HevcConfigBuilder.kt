// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

package org.openxplay.stream.replay

import java.io.ByteArrayOutputStream
import org.openxplay.stream.core.codec.HevcConfig

/** Builds hvcC from parameter sets, preserving the SPS profile, temporal layers and bit depths. */
internal object HevcConfigBuilder {
    fun build(vps: ByteArray, sps: ByteArray, pps: ByteArray): ByteArray {
        val parameters = listOf(vps, sps, pps)
        parameters.forEachIndexed { index, nal ->
            require(nal.size in 3..65535 && nal[0].toInt() and 0x80 == 0 &&
                (nal[0].toInt() ushr 1) and 63 == 32 + index && nal[1].toInt() and 7 != 0) { "Invalid HEVC parameter set" }
        }
        val bits = Bits(sps)
        bits.read(4)
        val subLayers = bits.read(3)
        require(subLayers <= 6) { "Invalid HEVC sublayer count" }
        val nested = bits.read(1)
        val profile = ByteArray(12) { bits.read(8).toByte() }
        val profiles = BooleanArray(subLayers)
        val levels = BooleanArray(subLayers)
        repeat(subLayers) { profiles[it] = bits.read(1) != 0; levels[it] = bits.read(1) != 0 }
        if (subLayers > 0) repeat(8 - subLayers) { require(bits.read(2) == 0) { "Invalid profile-tier reserved bits" } }
        repeat(subLayers) {
            if (profiles[it]) repeat(11) { bits.read(8) }
            if (levels[it]) bits.read(8)
        }
        require(bits.ue() <= 15) { "Invalid HEVC SPS id" }
        val chroma = bits.ue()
        require(chroma in 0..3) { "Invalid HEVC chroma format" }
        if (chroma == 3) bits.read(1)
        bits.ue(); bits.ue() // Dimensions are validated by core's SPS parser.
        if (bits.read(1) != 0) repeat(4) { bits.ue() }
        val lumaDepth = bits.ue()
        val chromaDepth = bits.ue()
        require(lumaDepth in 0..7 && chromaDepth in 0..7) { "Invalid HEVC bit depth" }
        val output = ByteArrayOutputStream()
        output.write(1); output.write(profile)
        output.write(byteArrayOf(0xf0.toByte(), 0, 0xfc.toByte(), (0xfc or chroma).toByte(),
            (0xf8 or lumaDepth).toByte(), (0xf8 or chromaDepth).toByte(), 0, 0,
            (((subLayers + 1) shl 3) or (nested shl 2) or 3).toByte(), 3))
        parameters.forEachIndexed { index, nal ->
            output.write(0xa0 + index); output.write(0); output.write(1)
            output.write(nal.size ushr 8); output.write(nal.size and 0xff); output.write(nal)
        }
        return output.toByteArray().also { HevcConfig.parse(it) }
    }

    private class Bits(nal: ByteArray) {
        private val rbsp = ByteArrayOutputStream().apply {
            var zeros = 0
            for (index in 2 until nal.size) {
                val byte = nal[index].toInt() and 0xff
                if (zeros >= 2 && byte == 3) {
                    require(index + 1 < nal.size && nal[index + 1].toInt() and 0xff <= 3) { "Invalid HEVC emulation prevention" }
                    zeros = 0
                } else {
                    write(byte); zeros = if (byte == 0) zeros + 1 else 0
                }
            }
        }.toByteArray()
        private var position = 0L
        fun read(count: Int): Int {
            require(count in 0..30 && position + count <= rbsp.size.toLong() * 8) { "Truncated HEVC SPS" }
            var value = 0
            repeat(count) {
                value = (value shl 1) or ((rbsp[(position / 8).toInt()].toInt() ushr (7 - (position % 8).toInt())) and 1)
                position++
            }
            return value
        }
        fun ue(): Int {
            var zeros = 0
            while (read(1) == 0) { zeros++; require(zeros <= 30) { "Invalid HEVC Exp-Golomb value" } }
            return ((1L shl zeros) - 1 + read(zeros)).toInt()
        }
    }
}
