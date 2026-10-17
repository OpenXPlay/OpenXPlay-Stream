// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.clink

import java.util.ArrayDeque
import org.openxplay.stream.core.*

/** Pure handoff gate. PLAY and every reset require a subsequent live keyframe. */
internal class VideoSwitch {
    var playing = false
        private set
    var live = false
        private set
    fun play(value: Boolean) { playing = value; reset() }
    fun reset() { live = false }
    fun accept(keyFrame: Boolean): Boolean {
        if (!playing) return false
        if (!live && !keyFrame) return false
        live = true
        return true
    }
    val placeholder: Boolean get() = playing && !live
}

/** Short critical sections only; retains borrowed frame arrays, never performs I/O. */
// Phones deliver video in Wi-Fi batches with gaps of up to ~1 s; a 100 ms budget reset the queue on
// every batch and kept requesting keyframes, which made Carlink head units flicker (2026-10-15).
internal class VideoQueue(private val maxBytes: Int = 4 * 1024 * 1024,
    private val maxDurationUs: Long = 600_000, private val maxFrames: Int = 120) {
    private val frames = ArrayDeque<VideoFrame>()
    private var bytes = 0L
    private var waiting = true
    private var stalled = false
    private var gap = false
    var dropped = 0L
        private set
    /** Diagnostics: resets caused by an over-full queue, and frames dropped while awaiting a keyframe. */
    var overflows = 0L
        private set
    var waitingDrops = 0L
        private set
    init { require(maxBytes > 0 && maxDurationUs > 0 && maxFrames > 0) }
    @Synchronized fun reset() { dropped += frames.size; frames.clear(); bytes = 0; waiting = true; gap = true }
    @Synchronized fun setStalled(value: Boolean) { stalled = value; reset() }
    /** Returns true when the consumer needs recovery and a keyframe request. */
    @Synchronized fun offer(frame: VideoFrame): Boolean {
        if (stalled) { dropped++; return false }
        if (waiting && !frame.keyFrame) { dropped++; waitingDrops++; return true }
        val overflow = bytes + frame.data.size > maxBytes || frames.size >= maxFrames ||
            (frames.isNotEmpty() && frame.ptsUs - frames.first.ptsUs > maxDurationUs)
        if (overflow) { overflows++; reset() }
        if (frame.data.size > maxBytes || waiting && !frame.keyFrame) { dropped++; return true }
        if (frame.keyFrame) waiting = false
        frames.addLast(frame); bytes += frame.data.size
        return overflow
    }
    @Synchronized fun takeGap(): Boolean = gap.also { gap = false }
    @Synchronized fun poll(): VideoFrame? = frames.pollFirst()?.also { bytes -= it.data.size }
    @Synchronized fun droppedCount(): Long = dropped
    @Synchronized fun size(): Int = frames.size
    @Synchronized fun bytes(): Long = bytes
}
