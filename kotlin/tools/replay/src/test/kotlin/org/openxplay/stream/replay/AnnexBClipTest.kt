// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.replay

import org.junit.jupiter.api.Test
import org.openxplay.stream.core.VideoCodec
import org.openxplay.stream.core.codec.AvcConfig
import org.openxplay.stream.core.codec.Nal
import kotlin.test.*

public class AnnexBClipTest {
    @Test
    public fun builderParsesCoreClipWithFourByteLengths(): Unit {
        val units = Nal.units(clipBytes(), Nal.Format.ANNEX_B).toList()
        val sps = units.first { it[0].toInt() and 31 == 7 }
        val pps = units.first { it[0].toInt() and 31 == 8 }
        val config = AvcConfigBuilder.build(sps, pps)
        val parsed = AvcConfig.parse(config)
        assertEquals("avc1.42C01F", parsed.codecString)
        assertEquals(640 to 360, parsed.width to parsed.height)
        assertEquals(4, parsed.nalLengthSize)
        assertContentEquals(sps, parsed.sps.single())
        assertContentEquals(pps, parsed.pps.single())
    }

    @Test
    public fun accessUnitsMatchSixtyFramesAndKeyframes(): Unit {
        val clip = AnnexBClip.parse(clipBytes())
        assertEquals(60, clip.accessUnits.size)
        assertEquals(listOf(0, 30), clip.accessUnits.indices.filter { clip.accessUnits[it].keyFrame })
        clip.accessUnits.forEach {
            assertEquals(it.keyFrame, Nal.isKeyFrame(it.data, VideoCodec.H264))
            assertEquals(1, Nal.units(it.data).count { nal -> nal[0].toInt() and 31 in 1..5 })
        }
    }

    @Test
    public fun splitsWithoutAudAndKeepsAdditionalSlicesTogether(): Unit {
        val units = Nal.units(clipBytes(), Nal.Format.ANNEX_B).filter { it[0].toInt() and 31 != 9 }.toList()
        fun annexB(nals: List<ByteArray>): ByteArray = nals.fold(ByteArray(0)) { bytes, nal ->
            bytes + byteArrayOf(0, 0, 1) + nal
        }
        assertEquals(60, AnnexBClip.parse(annexB(units)).accessUnits.size)
        val parameters = units.filter { it[0].toInt() and 31 in 7..8 }.take(2)
        // first_mb_in_slice: 0 = bit 1; 1 = bits 010. Remaining slice data is opaque to replay.
        val clip = AnnexBClip.parse(annexB(parameters + listOf(
            byteArrayOf(0x65, 0x80.toByte()), byteArrayOf(0x65, 0x40), byteArrayOf(0x41, 0x80.toByte()),
        )))
        assertEquals(2, clip.accessUnits.size)
        assertEquals(2, Nal.units(clip.accessUnits.first().data).count { it[0].toInt() and 31 == 5 })
    }

    @Test
    public fun malformedAndMissingMediaAreRejected(): Unit {
        assertFailsWith<IllegalArgumentException> { AnnexBClip.parse(ByteArray(0)) }
        assertFailsWith<IllegalArgumentException> { AnnexBClip.parse(byteArrayOf(0, 0, 1, 0x65, 0x80.toByte())) }
        assertFailsWith<IllegalArgumentException> { AvcConfigBuilder.build(byteArrayOf(0x68), byteArrayOf(0x67)) }
        assertFailsWith<IllegalArgumentException> { AnnexBClip.parse(clipBytes(), Double.NaN) }
    }
}
