// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.wire

import java.io.PipedInputStream
import java.io.PipedOutputStream
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference
import org.junit.jupiter.api.AfterEach
import org.openxplay.stream.core.*

class WireOutputContractTest : ProjectionOutputContract() {
    private lateinit var input: WireInput
    private val reports = AtomicInteger()
    private var expected = 0
    @Volatile private var closed = false
    override fun createOutput(): ProjectionOutput {
        val pair = streamPair()
        input = WireInput(WireTransport.streams(pair.second.input, pair.second.output))
        input.start(SessionConfig(emptyMap()), CountingSink {})
        val wire = WireOutput(WireTransport.streams(pair.first.input, pair.first.output))
        return object : ProjectionOutput by wire {
            override val id: String = wire.id
            override fun open(host: OutputHost) {
                wire.open(object : OutputHost {
                    override fun capabilities(capabilities: OutputCapabilities) {
                        host.capabilities(capabilities); reports.incrementAndGet()
                    }
                    override fun control(event: ControlEvent) { host.control(event) }
                    override fun stats(stats: OutputStats) { host.stats(stats) }
                })
            }
            override fun close() { wire.close(); closed = true }
        }
    }
    override fun videoFormat(): VideoFormat = testFormat()
    override fun stimulate(output: ProjectionOutput) {
        if (!closed) {
            expected = reports.get() + 1
            input.sendCapabilities(emptyOutputCapabilities)
        } else input.sendCapabilities(emptyOutputCapabilities)
    }
    override fun awaitIdle() { eventually { reports.get() >= expected } }
    @AfterEach fun cleanup() { if (::input.isInitialized) input.stop() }
}

class WireInputContractTest : ProjectionInputContract() {
    private val producer = AtomicReference<WireOutput?>()
    private var previous: WireOutput? = null
    private val callbacks = AtomicInteger()
    private var expected = 0
    override fun createInput(): ProjectionInput {
        val wire = WireInput(WireTransport.streams {
            val pair = streamPair()
            val output = WireOutput(WireTransport.streams(pair.first.input, pair.first.output))
            producer.get()?.close()
            output.open(object : OutputHost {
                override fun capabilities(capabilities: OutputCapabilities): Unit = Unit
                override fun control(event: ControlEvent): Unit = Unit
                override fun stats(stats: OutputStats): Unit = Unit
            })
            producer.set(output)
            pair.second
        })
        return object : ProjectionInput by wire {
            override fun start(config: SessionConfig, sink: InputSink) {
                wire.start(config, object : InputSink by sink {
                    override fun session(state: SessionState) { sink.session(state); callbacks.incrementAndGet() }
                })
            }
        }
    }
    override fun stimulate(input: ProjectionInput) {
        val running = input.control(ControlEvent.KeyFrameRequest(ScreenId.MAIN, Origin("contract", "client")))
        if (running) {
            eventually { producer.get() != null && producer.get() !== previous }
            previous = producer.get()
            expected = callbacks.get() + 1
        }
        producer.get()?.onSession(SessionState(SessionId(1), SessionPhase.ACTIVE), SessionConfig(emptyMap()))
    }
    override fun awaitIdle() { eventually { callbacks.get() >= expected } }
    @AfterEach fun cleanup() { producer.get()?.close() }
}

private class CountingSink(private val count: () -> Unit) : InputSink {
    override fun session(state: SessionState) { count() }
    override fun videoFormat(screen: ScreenId, format: VideoFormat?) { count() }
    override fun videoFrame(frame: VideoFrame) { count() }
    override fun audioFormat(stream: Int, format: AudioFormat?) { count() }
    override fun audioPacket(packet: AudioPacket) { count() }
    override fun microphone(stream: Int, request: MicRequest?) { count() }
}

private fun streamPair(): Pair<WireStreams, WireStreams> {
    val leftInput = PipedInputStream(65536)
    val rightOutput = PipedOutputStream(leftInput)
    val rightInput = PipedInputStream(65536)
    val leftOutput = PipedOutputStream(rightInput)
    return WireStreams(leftInput, leftOutput) to WireStreams(rightInput, rightOutput)
}
