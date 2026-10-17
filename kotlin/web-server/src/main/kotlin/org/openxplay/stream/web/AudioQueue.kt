// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.web

import kotlinx.serialization.json.JsonObject
import org.openxplay.stream.core.*

internal data class SelectedAudio(val packet: AudioPacket, val generation: Long)

/** Called under the output lock; sample duration and timestamp span bound each client's backlog. */
internal class AudioQueue(private val defaultFormat: AudioFormat? = null) {
    // Each format marker follows the last packet accepted before its callback.
    private val markers = ArrayDeque<Pair<Long, JsonObject>>()
    private var offered = 0L
    fun format(message: JsonObject): Boolean {
        if (markers.size >= 256) return false
        markers.addLast(offered to message); return true
    }
    private fun markerReady(): Boolean {
        val marker = markers.firstOrNull() ?: return false
        return packets.firstOrNull()?.third?.let { it > marker.first } ?: true
    }
    fun pollFormat(): JsonObject? = if (markerReady()) markers.removeFirst().second else null
    private val packets = ArrayDeque<Triple<AudioPacket, Long, Long>>()
    private val generations = mutableMapOf<Int, Long>()
    private var durationUs = 0L
    fun generation(stream: Int): Long = generations[stream] ?: 0
    fun remove(stream: Int) {
        generations[stream] = generation(stream) + 1
        val retained = packets.filter { it.first.stream != stream }
        packets.clear(); packets.addAll(retained)
        durationUs = packets.sumOf { it.second }
    }
    fun clear() {
        markers.clear()
        (generations.keys + packets.map { it.first.stream }).toSet().forEach(::remove)
    }
    fun offer(packet: AudioPacket, format: AudioFormat = checkNotNull(defaultFormat)) {
        val samples = if (format.codec == AudioCodec.PCM_S16LE) packet.data.size / (2 * format.channels) else 1024
        if (samples <= 0) return
        val duration = samples * 1_000_000L / format.sampleRate
        generations.putIfAbsent(packet.stream, 0)
        packets.addLast(Triple(packet, duration, ++offered)); durationUs += duration
        while (packets.isNotEmpty() && (durationUs > 500_000 ||
                packet.ptsUs - packets.first().first.ptsUs + duration > 500_000)) {
            durationUs -= packets.removeFirst().second
        }
    }
    fun poll(): AudioPacket? {
        if (markerReady()) return null
        val item = packets.removeFirstOrNull() ?: return null
        durationUs -= item.second
        return item.first
    }
}

internal fun validMicPacket(request: MicRequest, bytes: Int): Boolean {
    if (request.codec != AudioCodec.PCM_S16LE || request.frameMs != 20 ||
        request.sampleRate !in 8000..48000 || request.channels !in 1..2 || request.sampleRate % 50 != 0) return false
    val frameBytes = request.sampleRate / 50 * request.channels * 2
    return bytes > 0 && bytes % frameBytes == 0
}
