// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-22

package org.openxplay.stream.cast

import java.io.ByteArrayOutputStream
import java.util.ArrayDeque
import org.openxplay.stream.core.VideoCodec
import org.openxplay.stream.core.VideoFormat
import org.openxplay.stream.core.VideoFrame
import org.openxplay.stream.core.audio.Adts
import org.openxplay.stream.core.audio.AudioSpecificConfig
import org.openxplay.stream.core.codec.AvcConfig
import org.openxplay.stream.core.codec.HevcConfig
import org.openxplay.stream.core.codec.TsMuxer

internal val SILENT_AAC = byteArrayOf(0x21, 0x10, 0x04, 0x60, 0x8c.toByte(), 0x1c)
internal data class CastChunk(val bytes: ByteArray, val keyframeStart: Boolean, val atNanos: Long)
internal sealed interface MediaEvent {
    val ptsUs: Long
    data class Video(val frame: VideoFrame, override val ptsUs: Long) : MediaEvent
    data class Audio(val data: ByteArray, val silent: Boolean, override val ptsUs: Long) : MediaEvent
}

/** Pure scheduler, serialized by the output's short inbox lock. No muxing or callbacks here. */
internal class CastTimeline(val startNanos: Long, private val audioEnabled: Boolean) {
    private data class Pending(val frame: VideoFrame, var label: Long? = null)
    private val video = ArrayDeque<Pending>()
    private val audio = ArrayDeque<ByteArray>()
    private var bytes = 0
    private var waitingForKeyframe = true
    private var previousVideo = -1L
    private var emitted = 1_000_000L
    private var emittedVideo = -1L
    private var sampleCount = 0L
    var dropped = 0L; private set
    var requestKeyframe = true
    val queuedVideo: Int get() = video.size
    val queuedVideoBytes: Int get() = bytes
    val queuedAudio: Int get() = audio.size

    fun video(frame: VideoFrame) {
        if (frame.data.size > 8 * 1024 * 1024 || frame.data.isEmpty()) {
            dropped++; while (video.isNotEmpty()) dropVideo()
            waitingForKeyframe = true; requestKeyframe = true; return
        }
        if (waitingForKeyframe && !frame.keyFrame) { dropped++; requestKeyframe = true; return }
        if (frame.keyFrame) waitingForKeyframe = false
        video.addLast(Pending(frame)); bytes += frame.data.size
        if (video.size > 60 || bytes > 8 * 1024 * 1024) {
            do { dropVideo() } while (video.isNotEmpty() && (video.size > 60 || bytes > 8 * 1024 * 1024 || !video.first.frame.keyFrame))
            waitingForKeyframe = video.isEmpty(); requestKeyframe = true
        }
    }
    private fun dropVideo() { bytes -= video.removeFirst().frame.data.size; dropped++ }
    fun audio(data: ByteArray) {
        if (!audioEnabled || data.size !in 1..8184) return
        // Fourteen complete 1024-sample slots fit in 300 ms at 48 kHz.
        while (audio.size >= 14) { audio.removeFirst(); dropped++ }
        audio.addLast(data)
    }
    fun clearAudio() { audio.clear() }
    fun take(nowNanos: Long): List<MediaEvent> {
        val horizon = 1_000_000 + (nowNanos - startNanos) / 1000 - 200_000
        video.forEach { pending ->
            if (pending.label == null) {
                val arrival = pending.frame.receivedAtNanos.takeIf { it in startNanos..nowNanos } ?: nowNanos
                val label = maxOf(1_000_000 + (arrival - startNanos) / 1000, previousVideo + 1000)
                pending.label = label; previousVideo = label
            }
        }
        val events = mutableListOf<MediaEvent>()
        repeat(256) {
            val head = video.peekFirst()
            // A frame delivered beyond the reorder horizon cannot move an already emitted AAC clock back.
            val videoPts = head?.label?.let { maxOf(it, emitted, emittedVideo + 1000) } ?: Long.MAX_VALUE
            val audioPts = if (audioEnabled) 1_000_000 + sampleCount * 1_000_000 / 48_000 else Long.MAX_VALUE
            val next = minOf(videoPts, audioPts)
            if (next > horizon) return events
            if (videoPts <= audioPts) {
                val frame = video.removeFirst().frame; bytes -= frame.data.size
                events += MediaEvent.Video(frame, next); emittedVideo = next
            } else {
                val live = audio.pollFirst()
                events += MediaEvent.Audio(live ?: SILENT_AAC, live == null, next)
                sampleCount += 1024
            }
            emitted = next
        }
        return events
    }
}

/** One muxer per generation; its borrowed packet buffer is copied into owned chunks. */
internal class CastMediaMuxer(format: VideoFormat, audio: Boolean, private val sink: (CastChunk) -> Unit) {
    private val avc = if (format.codec == VideoCodec.H264) AvcConfig.parse(format.config) else null
    private val hevc = if (format.codec == VideoCodec.H265) HevcConfig.parse(format.config) else null
    private val aac = AudioSpecificConfig(2, 48_000, 2)
    private val bytes = ByteArrayOutputStream()
    private var keyframe = false
    private var now = 0L
    private val muxer = TsMuxer({ bytes.write(it) }, audioEnabled = audio, insertAud = true, videoCodec = format.codec)
    fun write(event: MediaEvent, nowNanos: Long) {
        now = nowNanos
        when (event) {
            is MediaEvent.Video -> {
                if (event.frame.keyFrame) { flush(); keyframe = true }
                if (avc != null) muxer.writeVideo(event.frame.data, avc, event.ptsUs, event.frame.keyFrame)
                else muxer.writeVideo(event.frame.data, checkNotNull(hevc), event.ptsUs, event.frame.keyFrame)
            }
            is MediaEvent.Audio -> muxer.writeAudio(Adts.buildHeader(aac, event.data.size) + event.data, event.ptsUs)
        }
        // Bound chunk allocation even for an unusually large access unit.
        flush()
    }
    fun flush() {
        if (bytes.size() == 0) return
        sink(CastChunk(bytes.toByteArray(), keyframe, now)); bytes.reset(); keyframe = false
    }
}
