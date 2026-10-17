// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core.codec

/** Parsed avcC. Parameter-set arrays must not be mutated after hand-off. Uses identity equality. */
public class AvcConfig private constructor(
    public val profile: Int,
    public val compatibility: Int,
    public val level: Int,
    public val nalLengthSize: Int,
    public val sps: List<ByteArray>,
    public val pps: List<ByteArray>,
    public val spsExtensions: List<ByteArray>,
    public val width: Int,
    public val height: Int,
) {
    public val codecString: String = "avc1.${hexByte(profile)}${hexByte(compatibility)}${hexByte(level)}"

    public companion object {
        /** Parses a complete avcC record and dimensions from its first SPS. */
        public fun parse(config: ByteArray): AvcConfig {
            val cursor = ByteCursor(config)
            require(cursor.u8() == 1) { "Unsupported avcC version" }
            val profile = cursor.u8()
            val compatibility = cursor.u8()
            val level = cursor.u8()
            // Reserved bits are ignored like FFmpeg and Chromium do; some encoders (iOS) leave them zero.
            val lengths = cursor.u8()
            val lengthSize = (lengths and 3) + 1
            require(lengthSize != 3) { "Reserved avcC NAL length size" }
            val count = cursor.u8()
            fun parameterSet(type: Int): ByteArray {
                val bytes = cursor.bytes(cursor.u16())
                require(bytes.isNotEmpty() && (bytes[0].toInt() and 0x9f) == type) {
                    "Invalid AVC parameter-set NAL type"
                }
                return bytes
            }
            val sps = List(count and 0x1f) { parameterSet(7) }
            val pps = List(cursor.u8()) { parameterSet(8) }
            require(sps.isNotEmpty() && pps.isNotEmpty()) { "avcC requires SPS and PPS" }
            // The High-profile extension (chroma format, bit depths, SPS extensions) is optional.
            // Like FFmpeg and Chromium, ignore its reserved bits and any incomplete or trailing bytes.
            val extensions = if (cursor.remaining >= 4 && profile in extensionProfiles) {
                cursor.bytes(3)
                List(cursor.u8()) { parameterSet(13) }
            } else {
                emptyList()
            }
            cursor.bytes(cursor.remaining)
            val (width, height) = parseSps(sps.first())
            return AvcConfig(profile, compatibility, level, lengthSize, sps, pps, extensions, width, height)
        }

        /** Parses coded size, interlacing and cropping; VUI and later RBSP fields are not interpreted. */
        public fun parseSps(sps: ByteArray): Pair<Int, Int> {
            require(sps.isNotEmpty() && (sps[0].toInt() and 0x9f) == 7) { "Expected AVC SPS" }
            val bits = BitReader(sps, 1)
            val profile = bits.bits(8).toInt()
            bits.skip(16) // Constraint flags and level_idc.
            require(bits.ue() <= 31) { "Invalid seq_parameter_set_id" }
            var chroma = 1
            var separateColourPlane = false
            if (profile in highProfiles) {
                chroma = bits.ue()
                require(chroma in 0..3) { "Invalid chroma_format_idc" }
                if (chroma == 3) separateColourPlane = bits.flag()
                require(bits.ue() <= 6 && bits.ue() <= 6) { "Unsupported AVC bit depth" }
                bits.skip(1) // qpprime_y_zero_transform_bypass_flag.
                if (bits.flag()) {
                    repeat(if (chroma == 3) 12 else 8) { index ->
                        if (bits.flag()) scalingList(bits, if (index < 6) 16 else 64)
                    }
                }
            }
            require(bits.ue() <= 12) { "Invalid log2_max_frame_num_minus4" }
            when (bits.ue()) {
                0 -> require(bits.ue() <= 12) { "Invalid log2_max_pic_order_cnt_lsb_minus4" }
                1 -> {
                    bits.skip(1)
                    bits.se()
                    bits.se()
                    val cycle = bits.ue()
                    require(cycle <= 255) { "Invalid picture-order cycle" }
                    repeat(cycle) { bits.se() }
                }
                2 -> Unit
                else -> throw IllegalArgumentException("Invalid pic_order_cnt_type")
            }
            bits.ue() // max_num_ref_frames.
            bits.skip(1) // gaps_in_frame_num_value_allowed_flag.
            val widthMbs = bits.ue().toLong() + 1
            val heightMapUnits = bits.ue().toLong() + 1
            val frameMbsOnly = bits.flag()
            if (!frameMbsOnly) bits.skip(1) // mb_adaptive_frame_field_flag.
            bits.skip(1) // direct_8x8_inference_flag.
            var left = 0L
            var right = 0L
            var top = 0L
            var bottom = 0L
            if (bits.flag()) {
                left = bits.ue().toLong()
                right = bits.ue().toLong()
                top = bits.ue().toLong()
                bottom = bits.ue().toLong()
            }
            val chromaArrayType = if (separateColourPlane) 0 else chroma
            val cropX = if (chromaArrayType == 1 || chromaArrayType == 2) 2 else 1
            val frameFactor = if (frameMbsOnly) 1 else 2
            val cropY = (if (chromaArrayType == 1) 2 else 1) * frameFactor
            return dimensions(
                widthMbs * 16 - (left + right) * cropX,
                heightMapUnits * 16 * frameFactor - (top + bottom) * cropY,
            )
        }

        private val highProfiles: Set<Int> = setOf(44, 83, 86, 100, 110, 118, 122, 128, 134, 135, 138, 139, 144, 244)
        private val extensionProfiles: Set<Int> = highProfiles

        private fun scalingList(bits: BitReader, size: Int) {
            var lastScale = 8
            var nextScale = 8
            repeat(size) {
                if (nextScale != 0) {
                    val delta = bits.se()
                    nextScale = (((lastScale.toLong() + delta) % 256 + 256) % 256).toInt()
                }
                if (nextScale != 0) lastScale = nextScale
            }
        }
    }
}
