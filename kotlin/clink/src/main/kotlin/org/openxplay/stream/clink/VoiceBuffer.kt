// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

package org.openxplay.stream.clink

import java.util.ArrayDeque
import org.openxplay.stream.core.audio.PcmResampler

/** Raw PCM stays here until a stream is ready, rather than advancing through mixer silence. */
internal class VoiceBuffer(private val rate: Int, private val channels: Int) {
    private val chunks = ArrayDeque<ByteArray>()
    private var offset = 0
    var size = 0; private set
    var targetUs = 80_000L; private set
    var ended = false; private set
    private var waiting = true
    private var arrival: Long? = null
    private var stableSince: Long? = null
    private val gaps = ArrayDeque<Pair<Long, Long>>()
    val ready: Boolean get() = if (ended) size > 0 else !waiting || size >= bytes(targetUs)
    val drained: Boolean get() = ended && size == 0
    private fun bytes(us: Long): Int = (us * rate / 1_000_000).toInt() * channels * 2
    private fun remove(count: Int): ByteArray {
        val out = ByteArray(count)
        var written = 0
        while (written < count) {
            val chunk = chunks.first
            val n = minOf(count - written, chunk.size - offset)
            chunk.copyInto(out, written, offset, offset + n)
            written += n; offset += n; size -= n
            if (offset == chunk.size) { chunks.removeFirst(); offset = 0 }
        }
        return out
    }
    fun push(data: ByteArray, nowUs: Long) {
        if (ended || data.isEmpty()) return
        arrival?.let { gaps.addLast(nowUs to maxOf(0L, nowUs - it)) }
        arrival = nowUs
        if (stableSince == null) stableSince = nowUs
        chunks.addLast(data); size += data.size
        decay(nowUs)
        val excess = size - bytes(targetUs + 200_000)
        if (excess > 0) remove(excess / (channels * 2) * (channels * 2))
    }
    fun end() { ended = true }
    private fun decay(nowUs: Long) {
        while (gaps.isNotEmpty() && nowUs - gaps.first.first > 10_000_000) gaps.removeFirst()
        val since = stableSince ?: nowUs
        if (nowUs - since >= 10_000_000) {
            targetUs = maxOf(80_000, targetUs - (nowUs - since) / 10_000_000 * 10_000)
            stableSince = nowUs
        }
    }
    fun read(nowUs: Long): ByteArray? {
        decay(nowUs)
        if (ended) return if (size == 0) null else remove(minOf(size, bytes(20_000)))
        if (waiting) {
            if (size < bytes(targetUs)) return null
            waiting = false
        }
        if (size < bytes(20_000)) {
            waiting = true
            targetUs = minOf(300_000, targetUs + (gaps.maxOfOrNull { it.second } ?: 0))
            stableSince = nowUs
            return null
        }
        return remove(bytes(20_000))
    }
}

/** Resample ahead of buffering so filter lookahead cannot pad each paced mixer read. */
internal class VoiceStream(rate: Int, channels: Int) {
    private val resampler = PcmResampler(rate, channels, 16000, 1)
    private val buffer = VoiceBuffer(16000, 1)
    private var ended = false
    private var arrivalUs = 0L
    val ready: Boolean get() = buffer.ready
    val drained: Boolean get() = buffer.drained
    fun push(data: ByteArray, nowUs: Long) {
        if (ended) return
        arrivalUs = nowUs
        buffer.push(resampler.process(data), nowUs)
    }
    fun end() {
        if (ended) return
        buffer.push(resampler.finish(), arrivalUs)
        buffer.end(); ended = true
    }
    fun read(nowUs: Long): ByteArray? = buffer.read(nowUs)
}

internal class VoicePacer {
    private var startUs = 0L
    private var frames = 0L
    fun reset(nowUs: Long) { startUs = nowUs; frames = 0 }
    fun next(nowUs: Long): Long {
        if (nowUs - (startUs + frames * 20_000) > 200_000) reset(nowUs)
        frames++
        return startUs + frames * 20_000
    }
}
