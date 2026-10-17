// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core.audio

import kotlin.math.PI
import kotlin.math.roundToInt
import kotlin.math.sin
import kotlin.math.sqrt
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

class PcmResamplerTest {
    @Test
    fun preservesSineFrequencyAndRejectsFrequenciesAboveOutputNyquist() {
        for ((inputRate, outputRate) in listOf(48000 to 16000, 44100 to 48000, 8000 to 48000)) {
            val converter = PcmResampler(inputRate, 1, outputRate, 1)
            val bytes = converter.process(sine(inputRate, 1000, inputRate)) + converter.finish()
            assertEquals(outputRate * 2, bytes.size)
            val signal = values(bytes).drop(outputRate / 10).take(outputRate * 8 / 10)
            val crossings = signal.zipWithNext().count { (a, b) -> a <= 0 && b > 0 }
            assertTrue(crossings in 799..801, "Frequency changed: $inputRate -> $outputRate, $crossings crossings")
            assertTrue(rms(signal) in 6900.0..7200.0)
        }
        for (frequency in listOf(9000, 10000, 14000, 20000)) {
            val converter = PcmResampler(48000, 1, 16000, 1)
            val bytes = converter.process(sine(48000, frequency, 48000)) + converter.finish()
            val signal = values(bytes).drop(1600).take(12800)
            assertTrue(rms(signal) < 20.0, "Aliased $frequency Hz: RMS ${rms(signal)}")
        }
    }

    @Test
    fun arbitraryByteBoundariesMatchOneChunkAndRetainExactRationalLength() {
        val input = sine(44100, 1379, 44101)
        for ((source, target) in listOf(44100 to 16000, 44100 to 48000, 48000 to 16000)) {
            val whole = PcmResampler(source, 1, target, 1)
            val expected = whole.process(input) + whole.finish()
            val split = PcmResampler(source, 1, target, 1)
            val output = java.io.ByteArrayOutputStream()
            var offset = 0
            var chunk = 1
            while (offset < input.size) {
                val end = minOf(input.size, offset + chunk)
                output.write(split.process(input.copyOfRange(offset, end)))
                offset = end
                chunk = chunk % 257 + 1
            }
            output.write(split.finish())
            assertContentEquals(expected, output.toByteArray())
            assertEquals(((44101L * target + source - 1) / source).toInt() * 2, expected.size)
            assertTrue(split.finish().isEmpty())
            assertFailsWith<IllegalStateException> { split.process(byteArrayOf()) }
            split.reset()
            assertContentEquals(expected, split.process(input) + split.finish())
        }
    }

    @Test
    fun identityAndChannelConversionUseSignedLittleEndianSamples() {
        val input = pcm(-32768, -1234, 0, 1234, 32767)
        val identity = PcmResampler(16000, 1, 16000, 1)
        val output = identity.process(input.copyOfRange(0, 3)) + identity.process(input.copyOfRange(3, input.size))
        assertContentEquals(input, output)
        assertTrue(identity.finish().isEmpty())
        val stereo = PcmResampler(16000, 1, 16000, 2)
        assertContentEquals(pcm(-32768, -32768, -1234, -1234, 0, 0, 1234, 1234, 32767, 32767), stereo.process(input))
        val mono = PcmResampler(16000, 2, 16000, 1)
        assertContentEquals(pcm(0, 32767, -32768, 1000), mono.process(pcm(-20000, 20000, 32767, 32767, -32768, -32768, 500, 1500)))
        val down = PcmResampler(48000, 2, 16000, 1)
        val opposed = sine(48000, 1000, 4800)
        val interleaved = values(opposed).flatMap { listOf(it, -it) }.toIntArray()
        assertTrue(values(down.process(pcm(*interleaved)) + down.finish()).all { it == 0 })
        val up = PcmResampler(8000, 1, 16000, 2)
        assertTrue(values(up.process(sine(8000, 1000, 800)) + up.finish()).chunked(2).all { it[0] == it[1] })
    }

    @Test
    fun rejectsInvalidFormatsAndIncompleteFinalFrames() {
        assertFailsWith<IllegalArgumentException> { PcmResampler(0, 1, 16000, 1) }
        assertFailsWith<IllegalArgumentException> { PcmResampler(16000, 3, 16000, 1) }
        val converter = PcmResampler(48000, 2, 16000, 1)
        converter.process(byteArrayOf(1, 2, 3))
        assertFailsWith<IllegalArgumentException> { converter.finish() }
        converter.process(byteArrayOf(4))
        assertEquals(2, converter.finish().size)
    }

    private fun rms(values: List<Int>): Double = sqrt(values.sumOf { it.toDouble() * it } / values.size)
}

internal fun pcm(vararg values: Int): ByteArray = ByteArray(values.size * 2).also { bytes ->
    values.forEachIndexed { index, value ->
        bytes[index * 2] = value.toByte()
        bytes[index * 2 + 1] = (value ushr 8).toByte()
    }
}

internal fun values(bytes: ByteArray): List<Int> = (bytes.indices step 2).map { readS16(bytes, it) }

internal fun sine(rate: Int, frequency: Int, frames: Int): ByteArray =
    pcm(*IntArray(frames) { (10000 * sin(2 * PI * frequency * it / rate)).roundToInt() })
