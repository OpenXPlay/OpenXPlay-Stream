// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core

import java.time.Duration
import java.util.concurrent.atomic.AtomicInteger
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Assertions.assertTimeoutPreemptively
import org.junit.jupiter.api.Test

/** Provide deterministic source activity so the stop fence is checked against real callbacks. */
public abstract class ProjectionInputContract {
    protected abstract fun createInput(): ProjectionInput
    protected abstract fun stimulate(input: ProjectionInput)
    protected open fun awaitIdle(): Unit = Unit
    protected open val callTimeBound: Duration = Duration.ofMillis(250)
    protected open fun configuration(input: ProjectionInput): SessionConfig = DefaultSessionPolicy.derive(input.capabilities, emptyList())

    @Test
    public fun repeatableStartStopFencesCallbacksAndControlReturnsPromptly() {
        val input = createInput()
        val callbacks = AtomicInteger()
        val sink = object : InputSink {
            override fun inputCapabilities(capabilities: InputCapabilities) { callbacks.incrementAndGet() }
            override fun session(state: SessionState) { callbacks.incrementAndGet() }
            override fun videoFormat(screen: ScreenId, format: VideoFormat?) { callbacks.incrementAndGet() }
            override fun videoFrame(frame: VideoFrame) { callbacks.incrementAndGet() }
            override fun audioFormat(stream: Int, format: AudioFormat?) { callbacks.incrementAndGet() }
            override fun audioPacket(packet: AudioPacket) { callbacks.incrementAndGet() }
            override fun microphone(stream: Int, request: MicRequest?) { callbacks.incrementAndGet() }
        }
        try {
            repeat(2) {
                val before = callbacks.get()
                assertTimeoutPreemptively(callTimeBound) { input.start(configuration(input), sink) }
                stimulate(input)
                awaitIdle()
                assertTrue(callbacks.get() > before, "Stimulus must produce a sink callback")
                assertTimeoutPreemptively(callTimeBound) {
                    input.control(ControlEvent.KeyFrameRequest(ScreenId.MAIN, Origin("contract", "client")))
                }
                assertTimeoutPreemptively(callTimeBound) { input.stop() }
                val stopped = callbacks.get()
                input.stop()
                stimulate(input)
                awaitIdle()
                assertTimeoutPreemptively(callTimeBound) {
                    input.control(ControlEvent.KeyFrameRequest(ScreenId.MAIN, Origin("contract", "client")))
                }
                awaitIdle()
                assertEquals(stopped, callbacks.get(), "No sink callbacks after stop")
            }
        } finally { input.stop() }
    }
}
