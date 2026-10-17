// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core

import org.openxplay.stream.core.codec.AvcConfig
import org.openxplay.stream.core.codec.HevcConfig
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertSame
import kotlin.test.assertTrue

class ConfigTest {
    @Test
    fun baseline640By480() {
        // avcC: version 1, profile 0x42, compatibility 0xc0, level 0x1e, four-byte lengths.
        // SPS: 40x30 macroblocks, frame_mbs_only, 4:2:0, no crop. PPS id and SPS id are zero.
        val config = vector("avc-baseline-640x480")
        val parsed = AvcConfig.parse(config)
        assertEquals(66, parsed.profile)
        assertEquals(0xc0, parsed.compatibility)
        assertEquals(30, parsed.level)
        assertEquals("avc1.42C01E", parsed.codecString)
        assertEquals(640 to 480, parsed.width to parsed.height)
        assertEquals(4, parsed.nalLengthSize)
        assertEquals(1, parsed.sps.size)
        assertEquals(1, parsed.pps.size)
        val format = VideoFormat.create(ScreenId.MAIN, VideoCodec.H264, config, 60.0)
        assertSame(config, format.config)
        assertEquals(parsed.codecString, format.codecString)
        assertEquals(640 to 480, format.width to format.height)
        assertEquals(60.0, format.fpsHint)
    }

    @Test
    fun high1920By1080WithCroppingAndScalingLists() {
        // Profile 0x64, compatibility 0, level 0x28. SPS has 120x68 macroblocks (1920x1088).
        // Bottom crop 4 in 4:2:0 removes 8 pixels. Scaling list 0 has 16 zero deltas;
        // list 1 uses delta -8 to select the default; others are absent. Extension fd f8 f8 00
        // specifies 4:2:0, 8-bit luma/chroma and zero SPS extensions.
        val parsed = AvcConfig.parse(vector("avc-high-1920x1080"))
        assertEquals("avc1.640028", parsed.codecString)
        assertEquals(1920 to 1080, parsed.width to parsed.height)
        assertTrue(parsed.spsExtensions.isEmpty())
    }

    @Test
    fun avcReservedBitsAreIgnoredLikeDecodersDo() {
        // iOS CarPlay sends High-profile avcC records whose reserved bits are zero.
        val strict = vector("avc-high-1920x1080")
        val zeroed = strict.copyOf().also { it[4] = (it[4].toInt() and 0x03).toByte(); it[5] = (it[5].toInt() and 0x1f).toByte() }
        // The vector ends with the extension fd f8 f8 00: zero its reserved bits, or drop it.
        val zeroedExtension = zeroed.copyOf().also { it.fill(0, it.size - 4, it.size - 1); it[it.size - 4] = 0x01 }
        val withoutExtension = zeroed.copyOf(zeroed.size - 4)
        for (config in listOf(zeroed, zeroedExtension, withoutExtension)) {
            val parsed = AvcConfig.parse(config)
            assertEquals("avc1.640028", parsed.codecString)
            assertEquals(1920 to 1080, parsed.width to parsed.height)
            assertTrue(parsed.spsExtensions.isEmpty())
        }
    }

    @Test
    fun mainHevc1920By1080() {
        // hvcC general profile byte 01: space 0, Main profile 1, low tier. Compatibility
        // 60 00 00 00 is 0x60000000: reverse its 32 bits to obtain RFC 6381 value 6.
        // Constraints 90 00 00 00 00 00 trim to 90; level 78 is decimal 120.
        // f0 00 / fc / fd / f8 / f8 are reserved bits + spatial/parallelism/chroma/depth;
        // 00 00 is average frame rate; 0f specifies one nested temporal layer and 4-byte lengths.
        // Three complete arrays a0/a1/a2 each contain one VPS/SPS/PPS. SPS is 1920x1088,
        // 4:2:0, bottom conformance offset 4: final height 1080. 00 00 03 sequences are EBSP escapes.
        val config = vector("hevc-main-1920x1080")
        val parsed = HevcConfig.parse(config)
        assertEquals("hvc1.1.6.L120.90", parsed.codecString)
        assertEquals(1920 to 1080, parsed.width to parsed.height)
        assertEquals(4, parsed.nalLengthSize)
        assertEquals(1, parsed.vps.size)
        assertEquals(1, parsed.sps.size)
        assertEquals(1, parsed.pps.size)
        assertContentEquals(hex("90 00 00 00 00 00"), parsed.constraints)
        val format = VideoFormat.create(ScreenId.CLUSTER, VideoCodec.H265, config)
        assertSame(config, format.config)
        assertEquals("hvc1.1.6.L120.90", format.codecString)
        assertEquals(1920 to 1080, format.width to format.height)
    }

    @Test
    fun hevcProfileSpaceTierAndConstraintTrimming() {
        val bytes = vector("hevc-main-1920x1080")
        bytes[1] = 0xe2.toByte() // Space C, high tier, profile 2.
        bytes[7] = 0x0a
        bytes[8] = 0
        bytes[9] = 0x01
        assertEquals("hvc1.C2.6.H120.90.0A.00.01", HevcConfig.parse(bytes).codecString)
        for (index in 6..11) bytes[index] = 0
        assertEquals("hvc1.C2.6.H120", HevcConfig.parse(bytes).codecString)
    }

    @Test
    fun avcInterlacedMonochromePictureOrderTypeOne() {
        val sps = TestBits().bits(100, 8).bits(0, 8).bits(40, 8).ue(0)
            .ue(0).ue(0).ue(0).bits(0, 1).bits(0, 1) // Monochrome, 8-bit, no scaling lists.
            .ue(0).ue(1).bits(0, 1).se(-1).se(1).ue(2).se(-2).se(2)
            .ue(1).bits(0, 1).ue(39).ue(14).bits(0, 1).bits(1, 1).bits(1, 1)
            .bits(1, 1).ue(1).ue(2).ue(3).ue(4).bits(0, 1).nal(hex("67"))
        // 640x480; monochrome crop units are 1x2 for a field-coded picture.
        assertEquals(637 to 466, AvcConfig.parseSps(sps))
    }

    @Test
    fun hevcSublayerProfileAndSeparateColourPlane() {
        val bits = TestBits().bits(0, 4).bits(1, 3).bits(1, 1)
        repeat(3) { bits.bits(0, 32) } // General profile-tier-level: 96 bits.
        bits.bits(1, 1).bits(1, 1) // Sublayer profile and level present.
        repeat(7) { bits.bits(0, 2) }
        repeat(2) { bits.bits(0, 32) }
        bits.bits(0, 24).bits(30, 8) // Sublayer profile (88 bits) and level (8 bits).
        bits.ue(0).ue(3).bits(1, 1).ue(640).ue(480).bits(1, 1).ue(1).ue(2).ue(3).ue(4)
        // Separate colour planes use 1x1 crop units, even though chroma_format_idc is 3.
        assertEquals(637 to 473, HevcConfig.parseSps(bits.nal(hex("42 01"))))
    }

    @Test
    fun invalidDimensionsAndCropping() {
        fun avcSps(widthMinusOne: Int, cropLeft: Int): ByteArray = TestBits()
            .bits(66, 8).bits(0, 8).bits(30, 8).ue(0).ue(0).ue(0).ue(0).ue(0).bits(0, 1)
            .ue(widthMinusOne).ue(0).bits(1, 1).bits(1, 1).bits(1, 1)
            .ue(cropLeft).ue(0).ue(0).ue(0).bits(0, 1).nal(hex("67"))
        assertFailsWith<IllegalArgumentException> { AvcConfig.parseSps(avcSps(0, 8)) }
        assertFailsWith<IllegalArgumentException> { AvcConfig.parseSps(avcSps(Int.MAX_VALUE - 1, 0)) }

        fun hevcSps(width: Int, cropLeft: Int): ByteArray {
            val bits = TestBits().bits(0, 4).bits(0, 3).bits(1, 1)
            repeat(3) { bits.bits(0, 32) }
            return bits.ue(0).ue(1).ue(width).ue(480).bits(1, 1)
                .ue(cropLeft).ue(0).ue(0).ue(0).nal(hex("42 01"))
        }
        assertFailsWith<IllegalArgumentException> { HevcConfig.parseSps(hevcSps(0, 0)) }
        assertFailsWith<IllegalArgumentException> { HevcConfig.parseSps(hevcSps(640, 321)) }
        assertFailsWith<IllegalArgumentException> { HevcConfig.parseSps(hevcSps(Int.MAX_VALUE, 0)) }
    }

    @Test
    fun malformedAndTruncatedConfigurations() {
        for (name in listOf("avc-baseline-640x480", "avc-high-1920x1080", "hevc-main-1920x1080")) {
            val bytes = vector(name)
            // The optional four-byte AVC extension may be absent or incomplete; decoders ignore it.
            val acceptedFrom = if (name == "avc-high-1920x1080") bytes.size - 4 else bytes.size
            for (size in bytes.indices) {
                if (size >= acceptedFrom) continue
                assertFailsWith<IllegalArgumentException>("$name truncated at $size") {
                    if (name.startsWith("avc")) {
                        AvcConfig.parse(bytes.copyOf(size))
                    } else {
                        HevcConfig.parse(bytes.copyOf(size))
                    }
                }
            }
            // Trailing bytes are ignored for avcC (as decoders do) but rejected for hvcC.
            if (name.startsWith("avc")) {
                VideoFormat.create(ScreenId.MAIN, VideoCodec.H264, bytes + 0)
            } else {
                assertFailsWith<IllegalArgumentException> { VideoFormat.create(ScreenId.MAIN, VideoCodec.H265, bytes + 0) }
            }
        }
        val baseline = vector("avc-baseline-640x480")
        assertFailsWith<IllegalArgumentException> { AvcConfig.parse(baseline.copyOf().also { it[0] = 2 }) }
        assertFailsWith<IllegalArgumentException> { AvcConfig.parse(baseline.copyOf().also { it[4] = 0xfe.toByte() }) }
        // A zero SPS count fails; reserved bits around it do not matter.
        assertFailsWith<IllegalArgumentException> { AvcConfig.parse(baseline.copyOf().also { it[5] = 0xe0.toByte() }) }
        assertFailsWith<IllegalArgumentException> { AvcConfig.parse(baseline.copyOf().also { it[8] = 0x68 }) }
        assertFailsWith<IllegalArgumentException> { AvcConfig.parseSps(hex("67 00 00 03 04")) }
        assertFailsWith<IllegalArgumentException> { AvcConfig.parseSps(hex("67 00 00 03")) }
        assertFailsWith<IllegalArgumentException> { AvcConfig.parseSps(hex("67 42 00 1e 00 00 00 00 00")) }
        assertFailsWith<IllegalArgumentException> {
            VideoFormat.create(ScreenId.MAIN, VideoCodec.H264, baseline, Double.NaN)
        }
        val shortLengths = baseline.copyOf().also { it[4] = 0xfc.toByte() }
        assertEquals(1, AvcConfig.parse(shortLengths).nalLengthSize)
        assertFailsWith<IllegalArgumentException> { VideoFormat.create(ScreenId.MAIN, VideoCodec.H264, shortLengths) }
        val hevc = vector("hevc-main-1920x1080")
        assertFailsWith<IllegalArgumentException> { HevcConfig.parse(hevc.copyOf().also { it[13] = 0 }) }
        assertFailsWith<IllegalArgumentException> { HevcConfig.parse(hevc.copyOf().also { it[21] = 0x0e }) }
        assertFailsWith<IllegalArgumentException> { HevcConfig.parse(hevc.copyOf().also { it[23] = 0xe0.toByte() }) }
        assertFailsWith<IllegalArgumentException> { HevcConfig.parse(hevc.copyOf().also { it[29] = 0 }) }
        assertFailsWith<IllegalArgumentException> { HevcConfig.parseSps(hex("42 01 ff")) }
    }
}
