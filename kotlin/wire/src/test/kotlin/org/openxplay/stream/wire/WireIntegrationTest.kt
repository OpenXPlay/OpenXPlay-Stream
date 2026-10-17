// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.wire

import java.io.PipedInputStream
import java.io.PipedOutputStream
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.CopyOnWriteArrayList
import org.junit.jupiter.api.Test
import org.openxplay.stream.core.*
import org.openxplay.stream.core.wire.BinaryKind
import kotlin.test.*

class WireIntegrationTest {
    @Test fun restartPreservesTheWireConnectionAndReplacesTheFencedSink() {
        val consumerInput = PipedInputStream(65536)
        val producerOutput = PipedOutputStream(consumerInput)
        val producerInput = PipedInputStream(65536)
        val consumerOutput = PipedOutputStream(producerInput)
        val declarations = CopyOnWriteArrayList<OutputCapabilities>()
        val restarts = CopyOnWriteArrayList<Origin>()
        val errors = CopyOnWriteArrayList<Throwable>()
        val caps = FakeInput().capabilities.copy(reconfigure = ReconfigureCost.RESTART_SESSION)
        val input = WireInput(WireTransport.streams(consumerInput, consumerOutput), name = "consumer", initialCapabilities = caps, onError = { errors.add(it) })
        val output = WireOutput(WireTransport.streams(producerInput, producerOutput), id = "consumer", inputCapabilities = caps, onError = { errors.add(it) })
        fun sink(states: MutableList<SessionState>) = object : InputSink {
            override fun session(state: SessionState) { states.add(state) }
            override fun videoFormat(screen: ScreenId, format: VideoFormat?) = Unit
            override fun videoFrame(frame: VideoFrame) = Unit
            override fun audioFormat(stream: Int, format: AudioFormat?) = Unit
            override fun audioPacket(packet: AudioPacket) = Unit
            override fun microphone(stream: Int, request: MicRequest?) = Unit
        }
        val oldStates = CopyOnWriteArrayList<SessionState>()
        val newStates = CopyOnWriteArrayList<SessionState>()
        val config = SessionConfig(emptyMap())
        try {
            input.start(config, sink(oldStates))
            output.open(object : OutputHost {
                override fun capabilities(value: OutputCapabilities) { declarations.add(value) }
                override fun requestRestart(origin: Origin) { restarts.add(origin) }
                override fun control(event: ControlEvent) = Unit
                override fun stats(stats: OutputStats) = Unit
            })
            eventually { declarations.isNotEmpty() }
            output.onSession(SessionState(SessionId(1), SessionPhase.ACTIVE), config)
            eventually { oldStates.size == 1 }
            input.restart(config, sink(newStates), authorized = false)
            assertTrue(restarts.isEmpty())
            input.restart(config, sink(newStates), authorized = true)
            eventually { restarts.size == 1 }
            output.onSession(SessionState(SessionId(2), SessionPhase.CONNECTING), config)
            eventually { newStates.size == 1 }
            assertEquals(1, oldStates.size)
            assertEquals(SessionId(2), newStates.single().id)
            assertTrue(errors.isEmpty(), errors.joinToString())
        } finally { input.stop(); output.close() }
    }

    @Test fun producerCapabilityDeclarationsUpdateHubControlAcceptance() {
        val consumerInput = PipedInputStream(65536)
        val producerOutput = PipedOutputStream(consumerInput)
        val producerInput = PipedInputStream(65536)
        val consumerOutput = PipedOutputStream(producerInput)
        val errors = CopyOnWriteArrayList<Throwable>()
        val declarations = CopyOnWriteArrayList<InputCapabilities>()
        val received = CopyOnWriteArrayList<WireMessage>()
        // A v0 producer declares that it cannot reconfigure, so the hub must keep the applied configuration.
        val initial = FakeInput().capabilities.copy(touch = false, keys = emptySet(), microphone = false,
            reconfigure = ReconfigureCost.UNSUPPORTED)
        val updated = initial.copy(touch = true, keys = setOf(KeyCode.HOME), screens = setOf(ScreenId.MAIN, ScreenId.CLUSTER))
        val input = WireInput(WireTransport.streams(consumerInput, consumerOutput),
            initialCapabilities = initial, onError = { errors.add(it) })
        val clock = ManualClock()
        val hub = ProjectionHub(input, policy = SessionPolicy { capabilities, outputs ->
            declarations.add(capabilities)
            DefaultSessionPolicy.derive(capabilities, outputs)
        }, clock = clock)
        val recording = RecordingOutput()
        val producer = WireConnection(WireTransport.streams(producerInput, producerOutput),
            WireMessage.Hello(WireRole.PRODUCER, "producer", "default"),
            listOf(WireMessage.InputCapabilitiesMessage(initial)), 256, 8L * 1024 * 1024, 100_000,
            { message, _ -> received.add(message) }, { _, _ -> }, { errors.add(it) })
        try {
            hub.addOutput(recording); hub.start()
            val host = assertNotNull(recording.host)
            val origin = Origin(recording.id, "default")
            val touch = ControlEvent.Touch(ScreenId.MAIN, listOf(TouchContact(0, 0.5, 0.25, true)), origin)
            val key = ControlEvent.Key(KeyCode.HOME, true, origin)
            val keyframe = ControlEvent.KeyFrameRequest(ScreenId.MAIN, origin)
            val applied = hub.sessionConfig
            host.control(touch); host.control(key)
            producer.start()
            assertTrue(producer.enqueue(WireMessage.InputCapabilitiesMessage(updated)))
            eventually { declarations.last() == updated }
            assertEquals(updated, input.capabilities)
            assertEquals(applied, hub.sessionConfig) // Wire v0 cannot apply configuration requests.
            assertNull(hub.pendingConfiguration)
            host.control(touch); host.control(key); host.control(keyframe)
            eventually { received.any { it is WireMessage.Keyframe } }
            assertEquals(listOf(BinaryKind.TOUCH, BinaryKind.KEY),
                received.filterIsInstance<WireMessage.Binary>().map { it.value.header.kind })
            assertEquals(listOf(initial, updated), declarations.takeLast(2))

            assertTrue(producer.enqueue(WireMessage.InputCapabilitiesMessage(initial)))
            eventually { declarations.last() == initial }
            assertEquals(initial, input.capabilities)
            host.control(touch); host.control(key)
            clock.advanceNanos(250_000_000)
            host.control(keyframe)
            eventually { received.filterIsInstance<WireMessage.Keyframe>().size == 2 }
            assertEquals(2, received.filterIsInstance<WireMessage.Binary>().size)
            assertTrue(errors.isEmpty(), errors.joinToString())
        } finally { hub.stop(); producer.close() }
    }

    @Test fun unixRoundTrip() {
        val dir = Files.createTempDirectory(Path.of("/tmp"), "oxp-wire-")
        val path = dir.resolve("wire.sock")
        try { roundTrip(WireTransport.unixListen(path)) { WireTransport.unixConnect(path) } }
        finally { eventually { !Files.exists(path) }; Files.delete(dir) }
    }
    @Test fun tcpRoundTrip() {
        val listener = WireTransport.tcpListen()
        roundTrip(listener) { WireTransport.tcpConnect(port = listener.awaitListening()) }
    }
    @Test fun nonLoopbackAddressesAreRejected() {
        assertFailsWith<IllegalArgumentException> { WireTransport.tcpListen("0.0.0.0") }
        assertFailsWith<IllegalArgumentException> { WireTransport.tcpConnect("192.168.1.2", 1234) }
    }
    private fun roundTrip(listener: WireTransport, connector: () -> WireTransport) {
        val errors = CopyOnWriteArrayList<Throwable>()
        val source = FakeInput()
        val caps = OutputCapabilities(LatencyClass.INTERACTIVE, listOf(DecoderSupport("avc1.42", true)),
            mapOf(ScreenId.MAIN to DisplayInfo(640, 480, 160, 60.0)), 10, setOf(KeyCode.HOME), emptySet(), false)
        val input = WireInput(listener, name = "consumer", initialCapabilities = source.capabilities.copy(microphone = false),
            outputCapabilities = caps, onError = { errors.add(it) })
        val recording = RecordingOutput()
        val destinationHub = ProjectionHub(input)
        val stats = CopyOnWriteArrayList<OutputStats>()
        val sourceHub = ProjectionHub(source, listener = object : HubListener {
            override fun stats(outputId: String, statsValue: OutputStats) { stats.add(statsValue) }
        })
        var output: WireOutput? = null
        try {
            destinationHub.addOutput(recording); destinationHub.start()
            listener.awaitListening()
            output = WireOutput(connector(), id = "consumer", name = "producer", inputCapabilities = source.capabilities,
                onError = { errors.add(it) })
            sourceHub.addOutput(output); sourceHub.start()
            eventually { recording.sessions.isNotEmpty() && input.capabilities == source.capabilities &&
                sourceHub.sessionConfig.screens[ScreenId.MAIN]?.width == 640 }
            source.emit {
                it.session(SessionState(SessionId(1), SessionPhase.ACTIVE))
                it.videoFormat(ScreenId.MAIN, testFormat())
                it.videoFrame(VideoFrame(ScreenId.MAIN, SessionId(1), byteArrayOf(0, 0, 0, 2, 0x65, 0),
                    12345, true, 123456000))
            }
            eventually { recording.frames.size == 1 }
            assertEquals(SessionPhase.ACTIVE, recording.sessions.last().first.phase)
            assertEquals("avc1.42C01E", recording.videoFormats.last().second!!.codecString)
            assertEquals(12345L, recording.frames.single().ptsUs)
            assertContentEquals(byteArrayOf(0, 0, 0, 2, 0x65, 0), recording.frames.single().data)
            assertTrue(input.sendClient("head-unit"))
            val origin = Origin("consumer", "head-unit")
            val localOrigin = Origin(recording.id, "head-unit")
            val host = assertNotNull(recording.host)
            host.control(ControlEvent.Touch(ScreenId.MAIN, listOf(TouchContact(0, 0.5, 0.25, true)), localOrigin))
            host.control(ControlEvent.Key(KeyCode.HOME, true, localOrigin))
            host.control(ControlEvent.KeyFrameRequest(ScreenId.MAIN, localOrigin))
            eventually { source.controls.any { it is ControlEvent.Touch } && source.controls.any { it is ControlEvent.Key } &&
                source.controls.any { it is ControlEvent.KeyFrameRequest } }
            val touch = source.controls.filterIsInstance<ControlEvent.Touch>().single()
            assertEquals(origin, touch.origin)
            assertEquals(0.5, touch.contacts.single().x, 1.0 / 65535)
            assertEquals(origin, source.controls.filterIsInstance<ControlEvent.KeyFrameRequest>().last().origin)
            assertTrue(input.sendStats(OutputStats(localOrigin, ScreenId.MAIN, 2000, 1, 2, 3, 4000)))
            eventually { stats.size == 1 }
            assertEquals(origin, stats.single().origin)
            assertEquals(2000L, stats.single().decodeTimeUs)
            source.emit { it.videoFormat(ScreenId.MAIN, null) }
            eventually { recording.videoFormats.last().second == null }
            source.emit { it.session(SessionState(SessionId(1), SessionPhase.ENDED)) }
            eventually { recording.sessions.last().first.phase == SessionPhase.ENDED }
            assertTrue(errors.isEmpty(), errors.joinToString())
        } finally { sourceHub.stop(); destinationHub.stop(); output?.close(); input.stop() }
    }
}

internal fun eventually(condition: () -> Boolean) {
    val deadline = System.nanoTime() + 5_000_000_000L
    while (!condition()) {
        check(System.nanoTime() < deadline) { "Timed out waiting for wire activity" }
        Thread.sleep(5)
    }
}
