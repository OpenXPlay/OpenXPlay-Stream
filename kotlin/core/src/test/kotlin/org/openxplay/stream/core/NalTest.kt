// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core

import org.openxplay.stream.core.codec.Nal
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class NalTest {
    @Test
    fun mixedStartCodesAndEmulationPrevention() {
        val annexB = hex("00 00 00 01 67 00 00 03 01 80 00 00 01 65 88")
        val avcc = hex("00 00 00 06 67 00 00 03 01 80 00 00 00 02 65 88")
        assertContentEquals(avcc, Nal.annexBToAvcc(annexB))
        assertContentEquals(hex("00 00 00 01 67 00 00 03 01 80 00 00 00 01 65 88"), Nal.avccToAnnexB(avcc))
        val units = Nal.units(avcc).toList()
        assertEquals(2, units.size)
        assertContentEquals(hex("67 00 00 03 01 80"), units[0])
        assertContentEquals(hex("65 88"), units[1])
        assertContentEquals(avcc, Nal.annexBToAvcc(Nal.avccToAnnexB(avcc)))
    }

    @Test
    fun annexBZeroPaddingAndEmptyStreams() {
        assertContentEquals(hex("00 00 00 02 65 80"), Nal.annexBToAvcc(hex("00 00 00 00 01 65 80 00 00")))
        assertContentEquals(byteArrayOf(), Nal.annexBToAvcc(byteArrayOf()))
        assertContentEquals(byteArrayOf(), Nal.avccToAnnexB(byteArrayOf()))
        assertFalse(Nal.isKeyFrame(byteArrayOf(), VideoCodec.H264))
    }

    @Test
    fun h264AndAllHevcIrapTypes() {
        assertTrue(Nal.isKeyFrame(hex("00 00 00 02 65 88"), VideoCodec.H264))
        assertFalse(Nal.isKeyFrame(hex("00 00 00 02 41 88"), VideoCodec.H264))
        assertTrue(Nal.isKeyFrame(hex("00 00 01 65 88"), VideoCodec.H264, Nal.Format.ANNEX_B))
        for (type in 0..63) {
            val avcc = byteArrayOf(0, 0, 0, 2, (type shl 1).toByte(), 1)
            assertEquals(type in 16..23, Nal.isKeyFrame(avcc, VideoCodec.H265), "HEVC NAL type $type")
            assertEquals(type in 16..23, Nal.isKeyFrame(Nal.avccToAnnexB(avcc), VideoCodec.H265, Nal.Format.ANNEX_B))
        }
    }

    @Test
    fun malformedFramingIsRejected() {
        for (bytes in listOf("00", "00 00 00 00", "ff ff ff ff", "00 00 00 03 65 88")) {
            assertFailsWith<IllegalArgumentException> { Nal.avccToAnnexB(hex(bytes)) }
        }
        for (bytes in listOf("00 00", "99 00 00 01 65", "00 00 01", "00 00 01 00 00 01 65")) {
            assertFailsWith<IllegalArgumentException> { Nal.annexBToAvcc(hex(bytes)) }
        }
        assertFailsWith<IllegalArgumentException> { Nal.isKeyFrame(hex("00 00 00 01 26"), VideoCodec.H265) }
        assertFailsWith<IllegalArgumentException> { Nal.isKeyFrame(hex("00 00 00 02 65 88 ff"), VideoCodec.H264) }
    }
}
