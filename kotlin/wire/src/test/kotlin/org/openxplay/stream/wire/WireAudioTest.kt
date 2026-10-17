// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.wire

import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.PipedInputStream
import java.io.PipedOutputStream
import java.util.concurrent.CountDownLatch
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicReference
import org.junit.jupiter.api.Test
import org.openxplay.stream.core.*
import org.openxplay.stream.core.wire.*
import kotlin.test.*

class WireAudioTest {
    @Test fun lateJoinAndReconnectReplayAudioAndMicAndReturnCaptureWithOrigin() {
        fun pair(): Pair<WireStreams, WireStreams> {
            val consumerInput = PipedInputStream(65536)
            val producerInput = PipedInputStream(65536)
            val closed = CountDownLatch(2)
            val owner = AutoCloseable { closed.countDown(); closed.await() }
            return WireStreams(consumerInput, PipedOutputStream(producerInput), owner) to
                WireStreams(producerInput, PipedOutputStream(consumerInput), owner)
        }
        val streams = AtomicReference(pair())
        val errors = CopyOnWriteArrayList<Throwable>()
        val sessions = CopyOnWriteArrayList<SessionState>()
        val formats = CopyOnWriteArrayList<Pair<Int, AudioFormat?>>()
        val requests = CopyOnWriteArrayList<Pair<Int, MicRequest?>>()
        val packets = CopyOnWriteArrayList<AudioPacket>()
        val sink = object : InputSink {
            override fun session(state: SessionState) { sessions.add(state) }
            override fun videoFormat(screen: ScreenId, format: VideoFormat?) = Unit
            override fun videoFrame(frame: VideoFrame) = Unit
            override fun audioFormat(stream: Int, format: AudioFormat?) { formats.add(stream to format) }
            override fun audioPacket(packet: AudioPacket) { packets.add(packet) }
            override fun microphone(stream: Int, request: MicRequest?) { requests.add(stream to request) }
        }
        val caps = emptyOutputCapabilities.copy(audio = setOf(AudioCodec.AAC_LC), microphone = true)
        val input = WireInput(WireTransport.streams { streams.get().first }, name = "consumer",
            outputCapabilities = caps, onError = { errors.add(it) })
        val source = FakeInput()
        val hub = ProjectionHub(source)
        val output = WireOutput(WireTransport.streams { streams.get().second }, id = "consumer",
            inputCapabilities = source.capabilities, onError = { errors.add(it) })
        val config = SessionConfig(emptyMap())
        val format = AudioFormat(0, AudioRole.MEDIA, AudioCodec.AAC_LC, 48000, 2, byteArrayOf(0x11, 0x90.toByte()))
        val request = MicRequest(1, AudioCodec.PCM_S16LE, 16000, 1, 20)
        try {
            hub.start()
            source.emit {
                it.session(SessionState(SessionId(1), SessionPhase.ACTIVE))
                it.audioFormat(0, format); it.microphone(1, request)
            }
            repeat(2) { run ->
                if (run > 0) streams.set(pair())
                input.start(config, sink)
                hub.addOutput(output)
                eventually { formats.size == run + 1 && requests.size == run + 1 }
                assertEquals(SessionId(1), sessions.last().id)
                val decoded = assertNotNull(formats.last().second)
                assertEquals(format.role, decoded.role); assertEquals(format.codec, decoded.codec)
                assertEquals(48000, decoded.sampleRate); assertEquals(2, decoded.channels)
                assertContentEquals(format.config, decoded.config)
                assertEquals(1 to request, requests.last())
                source.emit { it.audioPacket(AudioPacket(0, SessionId(1), byteArrayOf(0x21, 0x10), 123456)) }
                eventually { packets.size == run + 1 }
                assertEquals(SessionId(1), packets.last().session)
                assertEquals(123456L, packets.last().ptsUs)
                assertContentEquals(byteArrayOf(0x21, 0x10), packets.last().data)
                assertTrue(input.control(ControlEvent.MicPacket(1, byteArrayOf(0, 0, -1, 127), -123,
                    Origin("local", "head-unit-$run"))))
                eventually { source.controls.filterIsInstance<ControlEvent.MicPacket>().size == run + 1 }
                val mic = source.controls.filterIsInstance<ControlEvent.MicPacket>().last()
                assertEquals(Origin("consumer", "head-unit-$run"), mic.origin)
                assertEquals(1, mic.stream); assertEquals(-123L, mic.ptsUs)
                assertContentEquals(byteArrayOf(0, 0, -1, 127), mic.data)
                if (run == 1) {
                    source.emit { it.audioFormat(0, null); it.microphone(1, null) }
                    eventually { formats.last().second == null && requests.last().second == null }
                }
                // Fence both readers before closing either transport to avoid a peer-EOF error.
                input.stop(); hub.removeOutput(output.id)
            }
            assertFalse(input.control(ControlEvent.MicPacket(1, byteArrayOf(), 0, Origin("local", "client"))))
            assertTrue(errors.isEmpty(), errors.joinToString())
        } finally { input.stop(); hub.stop(); output.close() }
    }
    @Test fun audioRequiresSessionAndCurrentFormat() {
        val state = WireMessage.Session(SessionState(SessionId(1), SessionPhase.ACTIVE))
        val format = WireMessage.AudioFormatMessage(0, AudioFormat(0, AudioRole.VOICE, AudioCodec.PCM_S16LE, 16000, 1))
        listOf(
            emptyList(),
            listOf(state),
            listOf(state, format, WireMessage.AudioFormatMessage(0, null)),
            listOf(state, format, WireMessage.Session(SessionState(SessionId(2), SessionPhase.ACTIVE))),
            listOf(state, format, WireMessage.Session(SessionState(SessionId(1), SessionPhase.ENDED))),
        ).forEach { declarations ->
            val frames = ByteArrayOutputStream()
            StreamWireCodec.write(frames, WireMessage.Hello(WireRole.PRODUCER, "producer", "client"))
            declarations.forEach { StreamWireCodec.write(frames, it) }
            StreamWireCodec.write(frames, WireMessage.Binary(BinaryMessage(
                BinaryHeader(BinaryKind.AUDIO, 0, 0, 0u, 123, 456), byteArrayOf(1, 2))))
            val errors = CopyOnWriteArrayList<Throwable>()
            val packets = CopyOnWriteArrayList<AudioPacket>()
            val input = WireInput(WireTransport.streams(ByteArrayInputStream(frames.toByteArray()), ByteArrayOutputStream()),
                onError = { errors.add(it) })
            try {
                input.start(SessionConfig(emptyMap()), object : InputSink {
                    override fun session(state: SessionState) = Unit
                    override fun videoFormat(screen: ScreenId, format: VideoFormat?) = Unit
                    override fun videoFrame(frame: VideoFrame) = Unit
                    override fun audioFormat(stream: Int, format: AudioFormat?) = Unit
                    override fun audioPacket(packet: AudioPacket) { packets.add(packet) }
                    override fun microphone(stream: Int, request: MicRequest?) = Unit
                })
                eventually { errors.size == 1 }
                assertEquals("Audio requires active session and format", errors.single().message)
                assertTrue(packets.isEmpty())
            } finally { input.stop() }
        }
    }

}
