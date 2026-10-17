// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core

/** A borrowed frame plus the wire gap flag; media bytes are never copied. */
public data class QueuedFrame(public val frame: VideoFrame, public val gap: Boolean)

/** One screen per client. Queue operations are short; callbacks run outside the state lock. */
public class FrameQueue(
    maxDurationUs: Long = 100_000,
    public val maxBytes: Long = 8L * 1024 * 1024,
    private val needKeyframe: () -> Unit = {},
) {
    private val lock = Any()
    private val frames = ArrayDeque<QueuedFrame>()
    private var bytes = 0L
    private var waiting = true
    private var gap = true
    private var requested = false
    private var overflowed = false
    private var droppedFrames = 0L
    private var durationLimitUs = maxDurationUs
    private var session: SessionId? = null
    private var screen: ScreenId? = null

    init { require(maxDurationUs >= 0 && maxBytes > 0) }

    /** Takes effect from the next offer, so a playback change keeps the decoding history. */
    public var maxDurationUs: Long
        get() = synchronized(lock) { durationLimitUs }
        set(value) { require(value >= 0); synchronized(lock) { durationLimitUs = value } }

    /**
     * Frames discarded for back-pressure: those evicted when the queue outgrew its bounds, plus later
     * frames refused until the key frame that recovers. Waiting for a first key frame is not counted.
     */
    public val dropped: Long get() = synchronized(lock) { droppedFrames }

    public val size: Int get() = synchronized(lock) { frames.size }
    public val queuedBytes: Long get() = synchronized(lock) { bytes }
    public val durationUs: Long get() = synchronized(lock) { duration() }

    /** Returns whether the frame was retained. Recovery requests are issued once per gap. */
    public fun offer(frame: VideoFrame): Boolean {
        require(frame.data.isNotEmpty()) { "Empty video access unit" }
        var request = false
        val accepted = synchronized(lock) {
            require(screen == null || screen == frame.screen) { "Use one queue per screen" }
            screen = frame.screen
            if (session != null && frame.session.value < session!!.value) return@synchronized false
            if (session != frame.session) {
                reset()
                session = frame.session
            }
            if (frames.isNotEmpty() && frame.ptsUs < frames.last().frame.ptsUs) reset()
            if (waiting && !frame.keyFrame || frame.data.size.toLong() > maxBytes) {
                if (frame.data.size.toLong() > maxBytes) {
                    val alreadyRequested = requested
                    droppedFrames += frames.size
                    reset()
                    requested = alreadyRequested
                    overflowed = true
                }
                if (overflowed) droppedFrames++
                if (!requested) { requested = true; request = true }
                return@synchronized false
            }
            waiting = false
            frames.addLast(QueuedFrame(frame, gap))
            bytes += frame.data.size
            gap = false
            while (bytes > maxBytes || duration() > durationLimitUs) {
                overflowed = true
                removeFirst(); droppedFrames++
                while (frames.isNotEmpty() && !frames.first().frame.keyFrame) { removeFirst(); droppedFrames++ }
                if (frames.isNotEmpty()) {
                    val first = frames.removeFirst()
                    frames.addFirst(QueuedFrame(first.frame, true))
                } else {
                    waiting = true
                    gap = true
                }
                if (!requested) { requested = true; request = true }
            }
            if (frame.keyFrame && frames.isNotEmpty()) { requested = false; overflowed = false }
            frames.any { it.frame === frame }
        }
        if (request) needKeyframe()
        return accepted
    }

    public fun poll(): QueuedFrame? = synchronized(lock) {
        if (frames.isEmpty()) null else removeFirst()
    }

    public fun drain(maxFrames: Int = Int.MAX_VALUE): List<QueuedFrame> = synchronized(lock) {
        require(maxFrames >= 0)
        List(minOf(maxFrames, frames.size)) { removeFirst() }
    }

    /** Clears decoding history; the next delivery must be a keyframe and carries a gap. */
    public fun clear(): Unit = synchronized(lock) { reset(); session = null }

    private fun removeFirst(): QueuedFrame = frames.removeFirst().also { bytes -= it.frame.data.size }
    private fun duration(): Long = if (frames.size < 2) 0 else frames.last().frame.ptsUs - frames.first().frame.ptsUs
    private fun reset() {
        frames.clear(); bytes = 0; waiting = true; gap = true; requested = false; overflowed = false
    }
}
