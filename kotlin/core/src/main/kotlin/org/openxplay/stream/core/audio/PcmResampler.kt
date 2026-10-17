// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core.audio

import java.io.ByteArrayOutputStream
import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.ceil
import kotlin.math.cos
import kotlin.math.roundToInt
import kotlin.math.sin

/** Streaming S16LE converter. Calls must be serialized; chunks may split sample frames. */
public class PcmResampler(
    public val inputRate: Int,
    public val inputChannels: Int,
    public val outputRate: Int,
    public val outputChannels: Int,
) {
    private val cutoff: Double
    private val radius: Int
    private val phases = mutableMapOf<Int, DoubleArray>()
    private var samples = DoubleArray(0)
    private var buffered = 0
    private var base = 0L
    private var total = 0L
    private var position = 0L
    private var phase = 0
    private var partial = byteArrayOf()
    private var finished = false

    init {
        require(inputRate in 1..384_000 && outputRate in 1..384_000) { "Invalid PCM sample rate" }
        require(inputChannels in 1..2 && outputChannels in 1..2) { "PCM requires mono or stereo" }
        cutoff = 0.94 * minOf(1.0, outputRate.toDouble() / inputRate)
        radius = ceil(32.0 / cutoff).toInt()
    }

    /** Holds filter lookahead until more data arrives. Identity conversion copies complete frames. */
    public fun process(pcm: ByteArray): ByteArray {
        check(!finished) { "Resampler has ended; reset before a new run" }
        val bytes = partial + pcm
        val frameBytes = inputChannels * 2
        val frames = bytes.size / frameBytes
        partial = bytes.copyOfRange(frames * frameBytes, bytes.size)
        if (inputRate == outputRate) {
            if (inputChannels == outputChannels) return bytes.copyOf(frames * frameBytes)
            val output = ByteArrayOutputStream(frames * outputChannels * 2)
            repeat(frames) { frame ->
                repeat(outputChannels) { channel -> writeS16(output, converted(bytes, frame, channel).roundToInt()) }
            }
            return output.toByteArray()
        }
        val needed = Math.multiplyExact(Math.addExact(buffered, frames), outputChannels)
        if (samples.size < needed) samples = samples.copyOf(maxOf(needed, samples.size + samples.size / 2, 256))
        repeat(frames) { frame ->
            repeat(outputChannels) { channel ->
                samples[(buffered + frame) * outputChannels + channel] = converted(bytes, frame, channel)
            }
        }
        buffered += frames
        total = Math.addExact(total, frames.toLong())
        return emit(false)
    }

    /** Zero-pads the filter edges, producing ceil(inputFrames * outputRate / inputRate) frames in total. */
    public fun finish(): ByteArray {
        require(partial.isEmpty()) { "Incomplete S16LE sample frame" }
        if (finished) return byteArrayOf()
        finished = true
        val result = if (inputRate == outputRate) byteArrayOf() else emit(true)
        samples = DoubleArray(0)
        buffered = 0
        return result
    }

    public fun reset() {
        samples = DoubleArray(0); buffered = 0; base = 0; total = 0
        position = 0; phase = 0; partial = byteArrayOf(); finished = false
    }

    private fun emit(end: Boolean): ByteArray {
        val output = ByteArrayOutputStream()
        while (position < total && (end || total - position > radius)) {
            val weights = phases[phase] ?: weights().also { if (phases.size < 1024) phases[phase] = it }
            repeat(outputChannels) { channel ->
                var value = 0.0
                for (tap in weights.indices) {
                    val index = position + tap - radius + 1
                    if (index >= base && index < total) {
                        value += samples[(index - base).toInt() * outputChannels + channel] * weights[tap]
                    }
                }
                writeS16(output, value.roundToInt())
            }
            // Integer phase accumulation gives exact rational ratios without floating-point drift.
            val next = phase.toLong() + inputRate
            position += next / outputRate
            phase = (next % outputRate).toInt()
        }
        val discard = minOf(buffered.toLong(), maxOf(0L, position - radius + 1 - base)).toInt()
        samples.copyInto(samples, 0, discard * outputChannels, buffered * outputChannels)
        buffered -= discard
        base += discard
        return output.toByteArray()
    }

    private fun weights(): DoubleArray {
        val fraction = phase.toDouble() / outputRate
        val result = DoubleArray(radius * 2) { tap ->
            val distance = tap - radius + 1 - fraction
            val x = PI * cutoff * distance
            val sinc = if (abs(x) < 1e-12) cutoff else cutoff * sin(x) / x
            val window = if (abs(distance) >= radius) 0.0 else
                0.42 + 0.5 * cos(PI * distance / radius) + 0.08 * cos(2 * PI * distance / radius)
            sinc * window
        }
        val sum = result.sum()
        for (tap in result.indices) result[tap] /= sum
        return result
    }

    private fun converted(bytes: ByteArray, frame: Int, channel: Int): Double {
        val offset = frame * inputChannels * 2
        return if (inputChannels == 2 && outputChannels == 1) {
            (readS16(bytes, offset).toDouble() + readS16(bytes, offset + 2)) / 2
        } else readS16(bytes, offset + (if (inputChannels == 1) 0 else channel * 2)).toDouble()
    }
}

internal fun readS16(bytes: ByteArray, offset: Int): Int =
    ((bytes[offset].toInt() and 0xff) or (bytes[offset + 1].toInt() shl 8)).toShort().toInt()

internal fun writeS16(output: ByteArrayOutputStream, sample: Int) {
    val value = sample.coerceIn(-32768, 32767)
    output.write(value and 0xff)
    output.write((value ushr 8) and 0xff)
}
