// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14.

package org.openxplay.stream.replay

import java.nio.file.Files
import org.openxplay.stream.core.*
import org.openxplay.stream.core.audio.AudioClock

/** All deadlines are relative to the video epoch; all callbacks run on its worker. */
internal class ReplayAudioRun(
    private val options: ReplayAudioOptions,
    private val sink: InputSink,
    private val session: SessionId,
    private val loop: Boolean,
    private val summary: (MicSummary) -> Unit,
) : AutoCloseable {
    private val media = options.media?.let { AdtsClip.parse(Files.readAllBytes(it)) }
    private val voice = options.voice?.let { WavClip.parse(Files.readAllBytes(it)) }
    private val mediaClock = AudioClock(48000)
    private val voiceClock = voice?.let { AudioClock(it.sampleRate) }
    private val writer: WavWriter? = options.micOut?.let { WavWriter(it, options.micRate!!) }
    private val micStats = MicAccumulator()
    private var mediaIndex = 0
    private var mediaSamples = 0L
    private var mediaBase = 0L
    private var mediaDue = if (media == null) Long.MAX_VALUE else 0L
    private var voiceDue = if (voice == null) Long.MAX_VALUE else 0L
    private var voiceStart = 0L
    private var voiceFrame = 0
    private var voicePacket = 0L
    private var voiceActive = false
    private var mediaActive = false
    private var micActive = false
    private var summaryDue = 1_000_000_000L

    internal fun start() {
        media?.let { mediaActive = true; sink.audioFormat(0, it.format) }
        if (options.voiceEverySeconds == null) microphone(true)
    }

    private fun microphone(active: Boolean) {
        val rate = options.micRate ?: return
        if (micActive == active) return
        micActive = active
        sink.microphone(1, if (active) MicRequest(1, AudioCodec.PCM_S16LE, rate, 1, 20) else null)
    }

    internal fun accepts(packet: ControlEvent.MicPacket): Boolean {
        val rate = options.micRate ?: return false
        return micActive && packet.stream == 1 && packet.data.size % 2 == 0 &&
            packet.data.size / 2 in rate / 50..(rate + 49) / 50
    }

    internal fun receive(packet: ControlEvent.MicPacket) {
        writer?.write(packet.data)
        micStats.add(packet)
    }

    internal fun nextDeadline(): Long = minOf(mediaDue, voiceDue, if (options.micRate == null) Long.MAX_VALUE else summaryDue)

    internal fun tick(now: Long, arrivalUs: Long) {
        if (now >= mediaDue) {
            val clip = media!!
            sink.audioPacket(AudioPacket(0, session, clip.units[mediaIndex], mediaClock.timestamp(1024, arrivalUs)))
            mediaIndex++
            mediaSamples += 1024
            if (mediaIndex == clip.units.size) {
                if (loop) mediaIndex = 0 else {
                    mediaDue = Long.MAX_VALUE
                    mediaActive = false
                    sink.audioFormat(0, null)
                }
            }
            if (mediaActive) {
                mediaDue = mediaBase + mediaSamples / 48000 * 1_000_000_000L + mediaSamples % 48000 * 1_000_000_000L / 48000
                // Avoid a catch-up burst after worker stalls, while retaining sample pacing.
                if (now - mediaDue > 100_000_000L) {
                    mediaBase = now + 21_333_333L
                    mediaSamples = 0L
                    mediaDue = mediaBase
                }
            }
        }
        if (now >= voiceDue) {
            val clip = voice!!
            if (!voiceActive) {
                voiceActive = true
                voiceFrame = 0
                voicePacket = 0
                voiceStart = now
                voiceClock!!.reset()
                sink.audioFormat(1, clip.format)
                if (options.voiceEverySeconds != null) microphone(true)
            }
            if (voiceFrame == clip.frames && options.voiceEverySeconds != null) {
                voiceActive = false
                sink.audioFormat(1, null)
                microphone(false)
                voiceDue = maxOf(voiceStart + (options.voiceEverySeconds * 1_000_000_000).toLong(), now + 20_000_000L)
            } else {
                val frames = (((voicePacket + 1) * clip.sampleRate / 50) - (voicePacket * clip.sampleRate / 50)).toInt()
                val bytes = ByteArray(frames * clip.channels * 2)
                var copied = 0
                while (copied < frames) {
                    val count = minOf(frames - copied, clip.frames - voiceFrame)
                    clip.pcm.copyInto(bytes, copied * clip.channels * 2, voiceFrame * clip.channels * 2,
                        (voiceFrame + count) * clip.channels * 2)
                    copied += count
                    voiceFrame += count
                    if (voiceFrame == clip.frames) {
                        if (options.voiceEverySeconds != null || !loop) break
                        voiceFrame = 0
                    }
                }
                sink.audioPacket(AudioPacket(1, session, bytes, voiceClock!!.timestamp(frames, arrivalUs)))
                voicePacket++
                voiceDue = voiceStart + voicePacket * 20_000_000L
                if (now - voiceDue > 100_000_000L) {
                    voiceStart = now + 20_000_000L - voicePacket * 20_000_000L
                    voiceDue = now + 20_000_000L
                }
                if (voiceFrame == clip.frames && options.voiceEverySeconds == null && !loop) {
                    voiceActive = false
                    voiceDue = Long.MAX_VALUE
                    sink.audioFormat(1, null)
                }
            }
        }
        if (now >= summaryDue) {
            micStats.drain().forEach(summary)
            summaryDue = now + 1_000_000_000L
        }
    }

    internal fun end() {
        if (mediaActive) { mediaActive = false; sink.audioFormat(0, null) }
        if (voiceActive) { voiceActive = false; sink.audioFormat(1, null) }
        microphone(false)
    }

    override fun close() {
        try { micStats.drain().forEach(summary) } finally { writer?.close() }
    }
}
