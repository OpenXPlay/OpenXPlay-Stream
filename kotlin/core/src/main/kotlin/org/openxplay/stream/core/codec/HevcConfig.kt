// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core.codec

/** Parsed hvcC. Parameter-set arrays must not be mutated after hand-off. Uses identity equality. */
public class HevcConfig private constructor(
    public val profileSpace: Int,
    public val profile: Int,
    public val compatibility: Long,
    public val highTier: Boolean,
    public val level: Int,
    public val constraints: ByteArray,
    public val nalLengthSize: Int,
    public val vps: List<ByteArray>,
    public val sps: List<ByteArray>,
    public val pps: List<ByteArray>,
    public val width: Int,
    public val height: Int,
) {
    public val codecString: String = buildString {
        val space = when (profileSpace) {
            1 -> "A"
            2 -> "B"
            3 -> "C"
            else -> ""
        }
        val reversed = Integer.reverse(compatibility.toInt()).toLong() and 0xffffffffL
        append("hvc1.$space$profile.${reversed.toString(16).uppercase()}.${if (highTier) 'H' else 'L'}$level")
        val last = constraints.indexOfLast { it != 0.toByte() }
        for (index in 0..last) append(".${hexByte(constraints[index].toInt() and 0xff)}")
    }

    public companion object {
        /** Parses a complete hvcC record and dimensions from its first SPS; other array types are skipped. */
        public fun parse(config: ByteArray): HevcConfig {
            val cursor = ByteCursor(config)
            require(cursor.u8() == 1) { "Unsupported hvcC version" }
            val profileByte = cursor.u8()
            val compatibility = cursor.u32()
            val constraints = cursor.bytes(6)
            val level = cursor.u8()
            require(cursor.u16() and 0xf000 == 0xf000) { "Invalid hvcC reserved bits" }
            require(cursor.u8() and 0xfc == 0xfc) { "Invalid hvcC reserved bits" }
            require(cursor.u8() and 0xfc == 0xfc) { "Invalid hvcC reserved bits" }
            val lumaDepth = cursor.u8()
            val chromaDepth = cursor.u8()
            require(lumaDepth and 0xf8 == 0xf8 && chromaDepth and 0xf8 == 0xf8) { "Invalid hvcC reserved bits" }
            cursor.u16() // avgFrameRate.
            val temporal = cursor.u8()
            val lengthSize = (temporal and 3) + 1
            require(lengthSize != 3) { "Reserved hvcC NAL length size" }
            val vps = mutableListOf<ByteArray>()
            val sps = mutableListOf<ByteArray>()
            val pps = mutableListOf<ByteArray>()
            repeat(cursor.u8()) {
                val arrayHeader = cursor.u8()
                require(arrayHeader and 0x40 == 0) { "Invalid hvcC array reserved bit" }
                val type = arrayHeader and 0x3f
                repeat(cursor.u16()) {
                    val nal = cursor.bytes(cursor.u16())
                    require(nal.size >= 2 && (nal[0].toInt() and 0x80) == 0 &&
                        ((nal[0].toInt() ushr 1) and 0x3f) == type && (nal[1].toInt() and 7) != 0
                    ) { "Invalid HEVC array NAL header" }
                    when (type) {
                        32 -> vps.add(nal)
                        33 -> sps.add(nal)
                        34 -> pps.add(nal)
                    }
                }
            }
            cursor.end()
            require(vps.isNotEmpty() && sps.isNotEmpty() && pps.isNotEmpty()) { "hvcC requires VPS, SPS and PPS" }
            val (width, height) = parseSps(sps.first())
            return HevcConfig(
                profileByte ushr 6, profileByte and 0x1f, compatibility, profileByte and 0x20 != 0,
                level, constraints, lengthSize, vps.toList(), sps.toList(), pps.toList(), width, height,
            )
        }

        /** Reads profile-tier sublayers and size/conformance-window fields; later SPS fields are not interpreted. */
        public fun parseSps(sps: ByteArray): Pair<Int, Int> {
            require(sps.size >= 2 && (sps[0].toInt() and 0x80) == 0 &&
                ((sps[0].toInt() ushr 1) and 0x3f) == 33 && (sps[1].toInt() and 7) != 0
            ) { "Expected HEVC SPS" }
            val bits = BitReader(sps, 2)
            bits.skip(4) // sps_video_parameter_set_id.
            val subLayers = bits.bits(3).toInt()
            require(subLayers <= 6) { "Invalid sps_max_sub_layers_minus1" }
            bits.skip(1) // sps_temporal_id_nesting_flag.
            bits.skip(96) // general profile_tier_level.
            val profiles = BooleanArray(subLayers)
            val levels = BooleanArray(subLayers)
            repeat(subLayers) { index ->
                profiles[index] = bits.flag()
                levels[index] = bits.flag()
            }
            if (subLayers > 0) repeat(8 - subLayers) {
                require(bits.bits(2) == 0L) { "Invalid profile-tier reserved bits" }
            }
            repeat(subLayers) { index ->
                if (profiles[index]) bits.skip(88)
                if (levels[index]) bits.skip(8)
            }
            require(bits.ue() <= 15) { "Invalid sps_seq_parameter_set_id" }
            val chroma = bits.ue()
            require(chroma in 0..3) { "Invalid chroma_format_idc" }
            val separateColourPlane = chroma == 3 && bits.flag()
            val width = bits.ue().toLong()
            val height = bits.ue().toLong()
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
            val cropY = if (chromaArrayType == 1) 2 else 1
            return dimensions(width - (left + right) * cropX, height - (top + bottom) * cropY)
        }
    }
}
