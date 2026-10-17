// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.replay

import java.io.ByteArrayOutputStream
import org.openxplay.stream.core.ScreenId
import org.openxplay.stream.core.VideoCodec
import org.openxplay.stream.core.VideoFormat
import org.openxplay.stream.core.codec.Nal

/** Parsed replay media. Arrays are owned by the clip and must not be mutated. */
public class AnnexBClip private constructor(
    public val format: VideoFormat,
    public val accessUnits: List<AccessUnit>,
) {
    public class AccessUnit internal constructor(public val data: ByteArray, public val keyFrame: Boolean)

    public companion object {
        /**
         * Splits on AUDs, or on first_mb_in_slice == 0 for ordinary ordered slices without AUDs.
         * Streams using arbitrary slice order, FMO, data partitions or separate field pictures
         * require AUDs. Timestamps are generated during replay, not recovered from the bitstream.
         */
        public fun parse(data: ByteArray, frameRate: Double = 30.0): AnnexBClip {
            val units = Nal.units(data, Nal.Format.ANNEX_B).toList()
            val sps = units.firstOrNull { type(it) == 7 }
                ?: throw IllegalArgumentException("Annex B file has no SPS")
            val pps = units.firstOrNull { type(it) == 8 }
                ?: throw IllegalArgumentException("Annex B file has no PPS")
            val format = VideoFormat.create(ScreenId.MAIN, VideoCodec.H264, AvcConfigBuilder.build(sps, pps), frameRate)
            val hasAud = units.any { type(it) == 9 }
            val frames = mutableListOf<AccessUnit>()
            val pending = mutableListOf<ByteArray>()
            var hasPicture = false
            fun flush() {
                if (hasPicture) {
                    val output = ByteArrayOutputStream()
                    for (nal in pending) {
                        for (shift in 24 downTo 0 step 8) output.write(nal.size ushr shift and 0xff)
                        output.write(nal)
                    }
                    frames.add(AccessUnit(output.toByteArray(), pending.any { type(it) == 5 }))
                }
                pending.clear()
                hasPicture = false
            }
            for (nal in units) {
                val type = type(nal)
                require(nal[0].toInt() and 0x80 == 0 && type in 1..23) { "Invalid AVC NAL header" }
                val picture = type in 1..5
                if (type == 9 || !hasAud && hasPicture &&
                    (type in setOf(6, 7, 8, 14, 15, 16, 17, 18) || picture && firstMacroblock(nal) == 0)) flush()
                pending.add(nal)
                hasPicture = hasPicture || picture
                if (type == 10 || type == 11) flush()
            }
            flush()
            require(frames.isNotEmpty()) { "Annex B file has no pictures" }
            require(frames.any { it.keyFrame }) { "Replay requires an IDR keyframe" }
            return AnnexBClip(format, frames.toList())
        }

        private fun type(nal: ByteArray): Int = nal[0].toInt() and 0x1f

        // first_mb_in_slice is the first unsigned Exp-Golomb value in the slice RBSP.
        private fun firstMacroblock(nal: ByteArray): Int {
            require(type(nal) in setOf(1, 2, 5)) { "Data partitions require AUDs" }
            var byteIndex = 1
            var bitIndex = 0
            var zeros = 0
            var previousZeros = 0
            fun bit(): Int {
                if (bitIndex == 0) {
                    require(byteIndex < nal.size) { "Truncated slice header" }
                    if (previousZeros >= 2 && nal[byteIndex] == 3.toByte()) {
                        byteIndex++
                        previousZeros = 0
                        require(byteIndex < nal.size && (nal[byteIndex].toInt() and 0xff) <= 3) {
                            "Invalid slice emulation prevention"
                        }
                    }
                }
                val byte = nal[byteIndex].toInt() and 0xff
                val value = byte ushr (7 - bitIndex) and 1
                bitIndex++
                if (bitIndex == 8) {
                    bitIndex = 0
                    byteIndex++
                    previousZeros = if (byte == 0) previousZeros + 1 else 0
                }
                return value
            }
            while (bit() == 0) {
                zeros++
                require(zeros <= 30) { "Invalid first macroblock" }
            }
            var suffix = 0
            repeat(zeros) { suffix = (suffix shl 1) or bit() }
            return ((1L shl zeros) - 1 + suffix).toInt()
        }
    }
}
