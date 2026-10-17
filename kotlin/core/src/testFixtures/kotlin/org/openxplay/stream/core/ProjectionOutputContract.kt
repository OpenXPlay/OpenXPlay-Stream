// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core

import java.time.Duration
import java.util.concurrent.atomic.AtomicInteger
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertTimeoutPreemptively
import org.junit.jupiter.api.Test

/** Subclasses provide an output and any producer/client activity needed to exercise its host calls. */
public abstract class ProjectionOutputContract {
    protected abstract fun createOutput(): ProjectionOutput
    protected open val callTimeBound: Duration = Duration.ofMillis(250)
    protected abstract fun stimulate(output: ProjectionOutput)
    /** Override to wait for the implementation's asynchronous client activity to become idle. */
    protected open fun awaitIdle(): Unit = Unit
    /** Supplies a valid decoder configuration for media callbacks. */
    protected abstract fun videoFormat(): VideoFormat

    @Test
    public fun callbacksReturnPromptlyAndCloseIsIdempotent() {
        val output = createOutput()
        val calls = AtomicInteger()
        val host = object : OutputHost {
            override fun capabilities(capabilities: OutputCapabilities) { calls.incrementAndGet() }
            override fun control(event: ControlEvent) { calls.incrementAndGet() }
            override fun stats(stats: OutputStats) { calls.incrementAndGet() }
        }
        try {
            output.open(host)
            val session = SessionState(SessionId(1), SessionPhase.ACTIVE)
            val config = SessionConfig(mapOf(ScreenId.MAIN to ScreenConfig(VideoCodec.H264, 1280, 720, 60.0)))
            val format = videoFormat()
            val frame = VideoFrame(ScreenId.MAIN, session.id, byteArrayOf(0, 0, 0, 2, 0x65, 0x00), 0, true, 0)
            val callbacks: List<() -> Unit> = listOf(
                { output.onSession(session, config) },
                { output.onVideoFormat(ScreenId.MAIN, format) },
                { output.onVideoFrame(frame) },
                { output.onAudioFormat(0, AudioFormat(0, AudioRole.MEDIA, AudioCodec.PCM_S16LE, 48000, 1)) },
                { output.onAudioPacket(AudioPacket(0, session.id, byteArrayOf(0, 0), 0)) },
                { output.onMicrophone(0, MicRequest(0, AudioCodec.PCM_S16LE, 48000, 1, 20)) },
                { output.onVideoFormat(ScreenId.MAIN, null) },
                { output.onAudioFormat(0, null) },
                { output.onMicrophone(0, null) },
            )
            callbacks.forEach { call -> assertTimeoutPreemptively(callTimeBound) { call() } }
            stimulate(output)
            awaitIdle()
            assertTimeoutPreemptively(callTimeBound) { output.close() }
            val count = calls.get()
            assertTimeoutPreemptively(callTimeBound) { output.close() }
            stimulate(output)
            awaitIdle()
            assertEquals(count, calls.get(), "No host calls after close")
        } finally { output.close() }
    }
}
