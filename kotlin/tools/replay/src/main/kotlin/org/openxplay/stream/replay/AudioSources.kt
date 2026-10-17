// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14.

package org.openxplay.stream.replay

import java.nio.file.Path
import java.io.RandomAccessFile
import kotlin.math.log10
import org.openxplay.stream.core.*
import org.openxplay.stream.core.audio.Adts

public data class ReplayAudioOptions(
    public val media: Path? = null,
    public val voice: Path? = null,
    public val voiceEverySeconds: Double? = null,
    public val micRate: Int? = null,
    public val micOut: Path? = null,
) {
    init {
        require(voiceEverySeconds == null || voice != null && voiceEverySeconds.isFinite() &&
            voiceEverySeconds >= 0.000000001 && voiceEverySeconds < Long.MAX_VALUE / 1_000_000_000.0) {
            "--audio-voice-every requires voice audio and a finite positive interval"
        }
        require(micRate == null || micRate in 8000..48000) {
            "Microphone rate must be 8–48 kHz"
        }
        require(micOut == null || micRate != null) { "--mic-out requires --mic-rate" }
    }
}

/** Strict normalized AAC input; headers are removed before hand-off. */
public class AdtsClip private constructor(public val format: AudioFormat, public val units: List<ByteArray>) {
    public companion object {
        public fun parse(bytes: ByteArray): AdtsClip {
            val units = Adts.parse(bytes)
            require(units.isNotEmpty()) { "Empty ADTS file" }
            require(units.all { it.config.objectType == 2 && it.config.sampleRate == 48000 &&
                it.config.channelConfiguration == 2 }) { "MEDIA requires AAC-LC 48 kHz stereo" }
            return AdtsClip(AudioFormat(0, AudioRole.MEDIA, AudioCodec.AAC_LC, 48000, 2,
                units.first().config.build()), units.map { it.data })
        }
    }
}

/** RIFF PCM only. Unknown chunks are skipped, including their alignment padding. */
public class WavClip private constructor(
    public val sampleRate: Int, public val channels: Int, public val pcm: ByteArray,
) {
    public val frames: Int get() = pcm.size / (channels * 2)
    public val format: AudioFormat get() = AudioFormat(1, AudioRole.VOICE, AudioCodec.PCM_S16LE, sampleRate, channels)
    public companion object {
        public fun parse(bytes: ByteArray): WavClip {
            fun u16(p: Int): Int = (bytes[p].toInt() and 255) or ((bytes[p + 1].toInt() and 255) shl 8)
            fun u32(p: Int): Long = (0..3).fold(0L) { v, i -> v or ((bytes[p + i].toLong() and 255) shl (i * 8)) }
            fun tag(p: Int): String = String(bytes, p, 4, Charsets.US_ASCII)
            require(bytes.size >= 12 && tag(0) == "RIFF" && tag(8) == "WAVE") { "Invalid WAV header" }
            require(u32(4) + 8 == bytes.size.toLong()) { "Invalid RIFF length" }
            var offset = 12
            var rate = 0
            var channels = 0
            var pcm: ByteArray? = null
            while (offset < bytes.size) {
                require(bytes.size - offset >= 8) { "Truncated WAV chunk" }
                val size = u32(offset + 4)
                val start = offset + 8
                val end = start.toLong() + size
                require(end + (size and 1) <= bytes.size) { "Truncated WAV chunk payload" }
                when (tag(offset)) {
                    "fmt " -> {
                        require(rate == 0 && size >= 16 && u16(start) == 1 && u16(start + 14) == 16) {
                            "WAV requires one PCM S16LE format"
                        }
                        channels = u16(start + 2)
                        val hz = u32(start + 4)
                        require(channels in 1..2 && hz in 8000..48000) { "WAV requires mono/stereo at 8–48 kHz" }
                        rate = hz.toInt()
                        require(u16(start + 12) == channels * 2 && u32(start + 8) == rate.toLong() * channels * 2) {
                            "Invalid WAV block alignment or byte rate"
                        }
                    }
                    "data" -> {
                        require(pcm == null) { "Multiple WAV data chunks" }
                        pcm = bytes.copyOfRange(start, end.toInt())
                    }
                }
                offset = (end + (size and 1)).toInt()
            }
            val data = requireNotNull(pcm) { "Missing WAV data" }
            require(rate != 0 && data.isNotEmpty() && data.size % (channels * 2) == 0) { "Missing format or incomplete PCM frames" }
            return WavClip(rate, channels, data)
        }
    }
}

/** Finalizes the RIFF lengths on close; invoked only by the replay worker. */
public class WavWriter(path: Path, private val rate: Int, private val channels: Int = 1) : AutoCloseable {
    private val file: RandomAccessFile
    private var size = 0L
    private var closed = false
    init {
        require(rate in 8000..48000 && channels in 1..2)
        file = RandomAccessFile(path.toFile(), "rw")
        file.setLength(0)
        file.write(ByteArray(44))
        header()
    }
    public fun write(pcm: ByteArray): Unit {
        check(!closed)
        require(pcm.size % (channels * 2) == 0 && size + pcm.size <= 0xffffffffL - 36)
        file.write(pcm)
        size += pcm.size
    }
    private fun header() {
        fun number(value: Long, count: Int) { repeat(count) { file.write((value ushr (8 * it)).toInt() and 255) } }
        file.seek(0)
        file.writeBytes("RIFF"); number(size + 36, 4); file.writeBytes("WAVEfmt ")
        number(16, 4); number(1, 2); number(channels.toLong(), 2); number(rate.toLong(), 4)
        number(rate.toLong() * channels * 2, 4); number(channels.toLong() * 2, 2); number(16, 2)
        file.writeBytes("data"); number(size, 4)
    }
    override fun close() {
        if (closed) return
        closed = true
        try { header() } finally { file.close() }
    }
}

public data class MicSummary(public val packets: Long, public val origin: Origin, public val rmsDbfs: Double?)

/** Summaries are per origin; null dBFS represents digital silence without invalid JSON numbers. */
public class MicAccumulator {
    private class Totals { var packets = 0L; var samples = 0L; var squares = 0.0 }
    private val totals = linkedMapOf<Origin, Totals>()
    public fun add(packet: ControlEvent.MicPacket): Unit {
        require(packet.data.size % 2 == 0)
        val value = totals.getOrPut(packet.origin) { Totals() }
        value.packets++
        for (i in packet.data.indices step 2) {
            val sample = ((packet.data[i].toInt() and 255) or (packet.data[i + 1].toInt() shl 8)).toShort().toDouble() / 32768
            value.squares += sample * sample
            value.samples++
        }
    }
    public fun drain(): List<MicSummary> = totals.map { (origin, value) ->
        MicSummary(value.packets, origin, if (value.squares == 0.0) null else 10 * log10(value.squares / value.samples))
    }.also { totals.clear() }
}
