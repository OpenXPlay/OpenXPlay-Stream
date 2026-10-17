// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core.audio

import java.io.ByteArrayOutputStream
import java.util.ArrayDeque

/** Caller-paced saturating PCM mixer. Calls must be serialized; each read advances one fixed frame. */
public class PcmMixer(
    public val sampleRate: Int = 16_000,
    public val channels: Int = 1,
    public val frameMs: Int = 20,
) {
    public val frameSamples: Int
    private val streams = linkedMapOf<Int, Stream>()

    init {
        require(sampleRate in 1..384_000 && channels in 1..2 && frameMs > 0)
        val size = sampleRate.toLong() * frameMs
        require(size % 1000 == 0L && size / 1000 <= Int.MAX_VALUE / (channels * 2)) {
            "Frame duration must contain a whole, representable number of samples"
        }
        frameSamples = (size / 1000).toInt()
    }

    /** Active streams remain live during underruns; only ended streams can be drained. */
    public val isDrained: Boolean get() = streams.values.all { it.ended && it.chunks.isEmpty() }

    public fun streamDrained(stream: Int): Boolean =
        streams[stream]?.let { it.ended && it.chunks.isEmpty() } ?: true

    public fun addStream(stream: Int, sampleRate: Int, channels: Int) {
        val previous = streams[stream]
        require(previous == null || previous.ended && previous.chunks.isEmpty()) { "Duplicate PCM stream" }
        streams[stream] = Stream(PcmResampler(sampleRate, channels, this.sampleRate, this.channels))
    }

    public fun feed(stream: Int, pcm: ByteArray) {
        val source = requireNotNull(streams[stream]) { "Unknown PCM stream" }
        check(!source.ended) { "PCM stream has ended" }
        source.enqueue(source.resampler.process(pcm))
    }

    /** Ends input and retains queued audio, including resampler lookahead, until drained. */
    public fun endStream(stream: Int) {
        val source = requireNotNull(streams[stream]) { "Unknown PCM stream" }
        if (!source.ended) {
            source.enqueue(source.resampler.finish())
            source.ended = true
        }
    }

    /** Immediately discards a stream and its queued audio. */
    public fun removeStream(stream: Int) { streams.remove(stream) }

    /** Returns null when all streams have ended and drained; partial final frames are silence-padded. */
    public fun readFrame(): ByteArray? {
        if (isDrained) return null
        val sums = LongArray(frameSamples * channels)
        for (source in streams.values) {
            var sample = 0
            while (sample < sums.size && source.chunks.isNotEmpty()) {
                val bytes = source.chunks.first
                sums[sample++] += readS16(bytes, source.offset).toLong()
                source.offset += 2
                if (source.offset == bytes.size) { source.chunks.removeFirst(); source.offset = 0 }
            }
        }
        streams.entries.removeAll { it.value.ended && it.value.chunks.isEmpty() }
        val output = ByteArrayOutputStream(sums.size * 2)
        for (sum in sums) writeS16(output, sum.coerceIn(-32768L, 32767L).toInt())
        return output.toByteArray()
    }

    private class Stream(val resampler: PcmResampler) {
        val chunks = ArrayDeque<ByteArray>()
        var offset = 0
        var ended = false
        fun enqueue(bytes: ByteArray) { if (bytes.isNotEmpty()) chunks.addLast(bytes) }
    }
}
