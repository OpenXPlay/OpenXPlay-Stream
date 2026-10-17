// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.replay

import java.nio.file.Files
import java.nio.file.Path
import java.util.ArrayDeque
import java.util.concurrent.locks.LockSupport
import org.openxplay.stream.core.*

/** Receives accepted touch/key events on the replay thread; callbacks must return promptly. */
public fun interface ReplayControlListener {
    public fun onControl(event: ControlEvent)
}

/**
 * Replays fixed H.264 media on a daemon thread, which also handles file I/O and parsing.
 * Negotiated dimensions and FPS are advisory: the clip size and constructor frame rate are used.
 * Sink/listener callbacks must honor the non-blocking contract; stop fences them under a short lock.
 * A stopped session emits no further sink/control callbacks; recording summaries may finish later. EOF and load failures report ENDED asynchronously.
 */
public class AnnexBFileInput private constructor(
    private val load: () -> ByteArray,
    public val frameRate: Double,
    public val loop: Boolean,
    private val controlListener: ReplayControlListener,
    private val audio: ReplayAudioOptions,
    private val micSummary: (MicSummary) -> Unit,
) : ProjectionInput {
    public constructor(
        path: Path,
        frameRate: Double = 30.0,
        loop: Boolean = true,
        controlListener: ReplayControlListener = ReplayControlListener {},
    ) : this({ Files.readAllBytes(path) }, frameRate, loop, controlListener, ReplayAudioOptions(), {})

    public constructor(
        path: Path,
        frameRate: Double = 30.0,
        loop: Boolean = true,
        audio: ReplayAudioOptions,
        micSummary: (MicSummary) -> Unit = {},
        controlListener: ReplayControlListener = ReplayControlListener {},
    ) : this({ Files.readAllBytes(path) }, frameRate, loop, controlListener, audio, micSummary)

    /** Takes an owned snapshot so callers may reuse their original byte array. */
    public constructor(
        data: ByteArray,
        frameRate: Double = 30.0,
        loop: Boolean = true,
        controlListener: ReplayControlListener = ReplayControlListener {},
    ) : this(snapshotLoader(data), frameRate, loop, controlListener, ReplayAudioOptions(), {})

    public constructor(
        data: ByteArray,
        frameRate: Double = 30.0,
        loop: Boolean = true,
        audio: ReplayAudioOptions,
        micSummary: (MicSummary) -> Unit = {},
        controlListener: ReplayControlListener = ReplayControlListener {},
    ) : this(snapshotLoader(data), frameRate, loop, controlListener, audio, micSummary)

    init {
        require(frameRate.isFinite() && frameRate > 0.0 && frameRate <= 1_000_000.0) {
            "Frame rate must be finite and in (0, 1000000]"
        }
        require(1_000_000_000.0 / frameRate < Long.MAX_VALUE.toDouble()) { "Frame interval is too large" }
    }

    override val capabilities: InputCapabilities = InputCapabilities(
        codecs = setOf(VideoCodec.H264), screens = setOf(ScreenId.MAIN), touch = true,
        keys = KeyCode.entries.toSet(), microphone = audio.micRate != null, keyFrameRequests = true,
        reconfigure = ReconfigureCost.NONE,
    )

    private class Run(val id: SessionId, val sink: InputSink, val predecessor: Thread?) {
        lateinit var thread: Thread
        val controls = ArrayDeque<ControlEvent>()
        var keyframeRequested = false
        var audio: ReplayAudioRun? = null
    }

    private val lock = Any()
    private var current: Run? = null
    private var lastWorker: Thread? = null
    private var nextSession = 0L
    private var nextPtsUs = 0.0
    private var lastPtsUs = -1L
    private var lastReceivedNanos = Long.MIN_VALUE

    override fun start(config: SessionConfig, sink: InputSink) {
        require(supported(config)) { "Replay supports only H.264 on the main screen" }
        synchronized(lock) {
            if (current != null) return
            val run = Run(SessionId(++nextSession), sink, lastWorker)
            run.thread = Thread({ play(run) }, "oxp-replay-${run.id.value}").apply { isDaemon = true }
            current = run
            lastWorker = run.thread
            run.thread.start()
        }
    }

    override fun reconfigure(config: SessionConfig): ReconfigureResult =
        if (supported(config)) ReconfigureResult.APPLIED else ReconfigureResult.UNSUPPORTED

    override fun control(event: ControlEvent): Boolean = synchronized(lock) {
        val run = current ?: return false
        when (event) {
            is ControlEvent.KeyFrameRequest -> {
                if (event.screen != ScreenId.MAIN) return false
                run.keyframeRequested = true
            }
            is ControlEvent.Touch -> {
                if (event.screen != ScreenId.MAIN || run.controls.size >= MAX_CONTROLS) return false
                run.controls.add(event.copy(contacts = event.contacts.toList()))
            }
            is ControlEvent.Key -> {
                if (run.controls.size >= MAX_CONTROLS) return false
                run.controls.add(event)
            }
            is ControlEvent.MicPacket -> {
                if (run.audio?.accepts(event) != true || run.controls.size >= MAX_CONTROLS) return false
                run.controls.add(ControlEvent.MicPacket(event.stream, event.data.copyOf(), event.ptsUs, event.origin))
            }
        }
        LockSupport.unpark(run.thread)
        true
    }

    override fun stop() {
        synchronized(lock) {
            val run = current ?: return
            run.audio?.end()
            current = null
            run.controls.removeIf { it !is ControlEvent.MicPacket }
            run.thread.interrupt()
            LockSupport.unpark(run.thread)
        }
    }

    /** Shells may wait off the callback thread for the recording header and final summary. */
    public fun awaitStopped(): Unit {
        val worker = synchronized(lock) { lastWorker }
        if (worker != null && worker !== Thread.currentThread()) worker.join()
    }

    private fun supported(config: SessionConfig): Boolean = config.screens.all { (screen, value) ->
        screen == ScreenId.MAIN && value.codec == VideoCodec.H264
    }

    private fun dispatch(run: Run, call: () -> Unit): Boolean = synchronized(lock) {
        if (current !== run) return false
        call()
        current === run
    }

    private fun play(run: Run) {
        try {
            run.predecessor?.join()
            val clip = AnnexBClip.parse(load(), frameRate)
            if (!dispatch(run) { run.sink.session(SessionState(run.id, SessionPhase.ACTIVE)) }) return
            if (!dispatch(run) { run.sink.videoFormat(ScreenId.MAIN, clip.format) }) return
            val fencedSink = object : InputSink by run.sink {
                override fun audioFormat(stream: Int, format: AudioFormat?) { dispatch(run) { run.sink.audioFormat(stream, format) } }
                override fun audioPacket(packet: AudioPacket) { dispatch(run) { run.sink.audioPacket(packet) } }
                override fun microphone(stream: Int, request: MicRequest?) { dispatch(run) { run.sink.microphone(stream, request) } }
            }
            val audioRun = ReplayAudioRun(audio, fencedSink, run.id, loop, micSummary)
            if (!dispatch(run) { run.audio = audioRun; audioRun.start() }) { audioRun.close(); return }
            var index = clip.accessUnits.indexOfFirst { it.keyFrame }
            val period = 1_000_000_000.0 / frameRate
            val epoch = System.nanoTime()
            if (audio.media != null || audio.voice != null || audio.micRate != null) {
                nextPtsUs = maxOf(nextPtsUs, epoch / 1000.0)
            }
            var deadlineOffset = 0.0
            while (true) {
                // Controls wake the wait but do not advance the media deadline.
                while (true) {
                    val controls = synchronized(lock) {
                        if (current !== run) return
                        run.controls.toList().also { run.controls.clear() }
                    }
                    for (event in controls) {
                        if (event is ControlEvent.MicPacket) audioRun.receive(event)
                        else if (!dispatch(run) {
                            try { controlListener.onControl(event) } catch (_: Exception) { /* Listener isolation. */ }
                        }) return
                    }
                    val now = System.nanoTime()
                    if (!dispatch(run) { audioRun.tick(now - epoch, now / 1000) }) return
                    val remaining = minOf(deadlineOffset.toLong(), audioRun.nextDeadline()) - (System.nanoTime() - epoch)
                    if (remaining <= 0) {
                        if (System.nanoTime() - epoch >= deadlineOffset.toLong()) break
                        continue
                    }
                    LockSupport.parkNanos(this, remaining)
                    if (Thread.currentThread().isInterrupted) return
                }
                if (!dispatch(run) {
                        if (run.keyframeRequested) {
                            val next = (index until clip.accessUnits.size).firstOrNull { clip.accessUnits[it].keyFrame }
                            index = next ?: if (loop) clip.accessUnits.indexOfFirst { it.keyFrame }
                                else (clip.accessUnits.indices).last { clip.accessUnits[it].keyFrame }
                            run.keyframeRequested = false
                        }
                        val unit = clip.accessUnits[index]
                        val pts = maxOf(nextPtsUs.toLong(), lastPtsUs + 1)
                        nextPtsUs += 1_000_000.0 / frameRate
                        lastPtsUs = pts
                        val received = maxOf(System.nanoTime(), lastReceivedNanos + 1)
                        lastReceivedNanos = received
                        run.sink.videoFrame(VideoFrame(ScreenId.MAIN, run.id, unit.data, pts, unit.keyFrame, received))
                    }) return
                index++
                if (index == clip.accessUnits.size) {
                    if (!loop) { finish(run, "End of file"); return }
                    index = clip.accessUnits.indexOfFirst { it.keyFrame }
                }
                // Keep absolute deadlines; when late by a whole frame, resume without a catch-up burst.
                deadlineOffset += period
                val nowOffset = (System.nanoTime() - epoch).toDouble()
                if (nowOffset - deadlineOffset >= period) deadlineOffset = nowOffset + period
            }
        } catch (failure: Exception) {
            finish(run, failure.message ?: failure.javaClass.simpleName)
        } finally {
            val pending = synchronized(lock) {
                run.controls.filterIsInstance<ControlEvent.MicPacket>().also { run.controls.clear() }
            }
            try { pending.forEach { run.audio?.receive(it) } } finally { run.audio?.close() }
        }
    }

    private fun finish(run: Run, reason: String) {
        try {
            if (!dispatch(run) { run.audio?.end(); run.sink.videoFormat(ScreenId.MAIN, null) }) return
            dispatch(run) { run.sink.session(SessionState(run.id, SessionPhase.ENDED, reason)) }
        } finally {
            synchronized(lock) { if (current === run) { current = null; run.controls.removeIf { it !is ControlEvent.MicPacket } } }
        }
    }

    private companion object {
        const val MAX_CONTROLS: Int = 1024

        fun snapshotLoader(data: ByteArray): () -> ByteArray {
            val snapshot = data.copyOf()
            return { snapshot }
        }
    }
}
