// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core.audio

import org.openxplay.stream.core.hex
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

class AacTest {
    @Test
    fun ascMatchesNormalizedMediaAndRoundTripsIndexedAndExplicitRates() {
        val media = AudioSpecificConfig(2, 48000, 2)
        assertContentEquals(hex("11 90"), media.build())
        assertEquals(media, AudioSpecificConfig.parse(hex("11 90")))
        for (type in 1..4) for (channels in 1..7) for (index in 0..12) {
            val config = AudioSpecificConfig(type, AudioSpecificConfig.rateForIndex(index), channels)
            assertEquals(index, config.samplingFrequencyIndex)
            assertEquals(config, AudioSpecificConfig.parse(config.build()))
        }
        for (rate in listOf(12345, 48000, 0xffffff)) {
            val explicit = AudioSpecificConfig(2, rate, 1, 15)
            assertEquals(5, explicit.build().size)
            assertEquals(explicit, AudioSpecificConfig.parse(explicit.build()))
        }
    }

    @Test
    fun adtsRoundTripsMultipleUnitsWithConfigChangesAndMaximumLength() {
        val a = AudioSpecificConfig(2, 48000, 2)
        val b = AudioSpecificConfig(1, 16000, 1)
        val payloadA = byteArrayOf(1, 2, 3)
        val payloadB = ByteArray(8184) { it.toByte() }
        val framedA = Adts.buildHeader(a, payloadA.size) + payloadA
        val framedB = Adts.buildHeader(b, payloadB.size) + payloadB
        assertContentEquals(hex("ff f1 4c 80 01 5f fc"), framedA.copyOf(7))
        val units = Adts.parse(framedA + framedB)
        assertEquals(2, units.size)
        assertEquals(a, units[0].config)
        assertEquals(b, units[1].config)
        assertContentEquals(payloadA, units[0].data)
        assertContentEquals(payloadB, units[1].data)
        assertEquals(8191, Adts.validateFrame(framedB).frameLength)
        assertTrue(Adts.parse(byteArrayOf()).isEmpty())
        val mpeg2 = framedA.copyOf().also { it[1] = 0xf9.toByte() }
        assertEquals(a, Adts.parse(mpeg2).single().config)
        val explicitStandard = AudioSpecificConfig(2, 48000, 2, 15)
        assertContentEquals(Adts.buildHeader(a, 3), Adts.buildHeader(explicitStandard, 3))
    }

    @Test
    fun ascRejectsTruncationReservedIndicesAndUnsupportedCodecFeatures() {
        val valid = AudioSpecificConfig(2, 12345, 1).build()
        for (length in valid.indices) {
            assertFailsWith<IllegalArgumentException> { AudioSpecificConfig.parse(valid.copyOf(length)) }
        }
        for (bytes in listOf(hex("00 10"), hex("f8 10"), hex("16 90"), hex("17 10"), hex("11 80"), hex("11 f8"), hex("11 91"), hex("11 92"), hex("11 94"), hex("11 90 00"))) {
            assertFailsWith<IllegalArgumentException> { AudioSpecificConfig.parse(bytes) }
        }
        assertFailsWith<IllegalArgumentException> { AudioSpecificConfig(2, 0, 1) }
        assertFailsWith<IllegalArgumentException> { AudioSpecificConfig(5, 48000, 2) }
        assertFailsWith<IllegalArgumentException> { AudioSpecificConfig(2, 48000, 2, 4) }
    }

    @Test
    fun adtsRejectsBadSyncLayerRateChannelsBlocksLengthsAndTrailingData() {
        val config = AudioSpecificConfig(2, 48000, 2)
        val frame = Adts.buildHeader(config, 3) + byteArrayOf(1, 2, 3)
        for (length in 1 until frame.size) {
            assertFailsWith<IllegalArgumentException> { Adts.parse(frame.copyOf(length)) }
        }
        val mutations = listOf(0 to 0xfe, 1 to 0xf3, 1 to 0xf0, 2 to 0x74, 2 to 0x7c, 3 to 0, 6 to 0xff, 4 to 0xff, 5 to 0xff)
        for ((offset, value) in mutations) {
            val malformed = frame.copyOf().also { it[offset] = value.toByte() }
            assertFailsWith<IllegalArgumentException>("Offset $offset, value $value") { Adts.parse(malformed) }
        }
        assertFailsWith<IllegalArgumentException> { Adts.parse(frame + byteArrayOf(0)) }
        assertFailsWith<IllegalArgumentException> { Adts.validateFrame(frame + frame) }
        assertFailsWith<IllegalArgumentException> { Adts.parseHeader(frame, -1) }
        assertFailsWith<IllegalArgumentException> { Adts.parseHeader(frame, Int.MAX_VALUE) }
        assertFailsWith<IllegalArgumentException> { Adts.buildHeader(config, 0) }
        assertFailsWith<IllegalArgumentException> { Adts.buildHeader(config, 8185) }
        assertFailsWith<IllegalArgumentException> { Adts.buildHeader(AudioSpecificConfig(2, 12345, 1), 1) }
    }
}
