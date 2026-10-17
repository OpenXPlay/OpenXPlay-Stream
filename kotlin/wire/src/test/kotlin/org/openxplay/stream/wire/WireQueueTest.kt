// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.wire

import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.InputStream
import java.util.concurrent.CountDownLatch
import java.util.concurrent.atomic.AtomicInteger
import org.junit.jupiter.api.Test
import org.openxplay.stream.core.*
import org.openxplay.stream.core.wire.*
import kotlin.test.*

class WireQueueTest {
    @Test fun overflowDropsToNextKeyframeAndFencesOldSessions() {
        val hello = ByteArrayOutputStream().also {
            StreamWireCodec.write(it, WireMessage.Hello(WireRole.CONSUMER, "consumer", "client"))
        }.toByteArray()
        val close = CountDownLatch(1)
        val input = object : InputStream() {
            private val prefix = ByteArrayInputStream(hello)
            override fun read(): Int {
                val next = prefix.read()
                if (next >= 0) return next
                close.await(); return -1
            }
            override fun close() { close.countDown() }
        }
        val output = ByteArrayOutputStream()
        val requests = AtomicInteger()
        val errors = AtomicInteger()
        val run = WireConnection(WireTransport.streams(input, output),
            WireMessage.Hello(WireRole.PRODUCER, "producer", "client"), emptyList(), 10, 12, 100_000,
            { _, _ -> }, { _, origin ->
                assertEquals(Origin("consumer", "client"), origin); requests.incrementAndGet()
            }, { errors.incrementAndGet() })
        try {
            run.session(SessionState(SessionId(2), SessionPhase.ACTIVE))
            run.format(ScreenId.MAIN, testFormat())
            fun frame(pts: Long, key: Boolean, session: Long = 2): VideoFrame = VideoFrame(ScreenId.MAIN,
                SessionId(session), byteArrayOf(0, 0, 0, 2, (if (key) 0x65 else 0x41).toByte(), 0), pts, key, 123000)
            run.frame(frame(0, false))
            run.frame(frame(0, true, session = 1))
            run.frame(frame(1, true))
            run.frame(frame(2, false))
            run.frame(frame(3, false)) // 18 bytes exceeds the 12-byte bound; all three are discarded.
            run.frame(frame(4, true))
            run.start()
            fun decoded(): List<WireMessage> = try {
                val bytes = output.toByteArray()
                val source = ByteArrayInputStream(bytes)
                buildList { while (true) add(StreamWireCodec.read(source) ?: break) }
            } catch (_: WireFormatException) { emptyList() }
            eventually { decoded().filterIsInstance<WireMessage.Binary>().size == 1 && requests.get() > 0 }
            val messages = decoded()
            assertIs<WireMessage.Hello>(messages[0])
            assertIs<WireMessage.Session>(messages[1])
            assertIs<WireMessage.VideoFormatMessage>(messages[2])
            val binary = messages.filterIsInstance<WireMessage.Binary>().single().value
            assertEquals(4L, binary.header.ptsUs)
            assertEquals(OxpWebCodec.KEYFRAME or OxpWebCodec.GAP, binary.header.flags)
            assertEquals(123L, binary.header.senderClockUs)
            assertEquals(0, errors.get())
        } finally { run.close() }
    }
    @Test fun audioDropsOldestAndFencesFormatsAndSessionsWithoutVideoRecovery() {
        val hello = ByteArrayOutputStream().also {
            StreamWireCodec.write(it, WireMessage.Hello(WireRole.CONSUMER, "consumer", "client"))
        }.toByteArray()
        val close = CountDownLatch(1)
        val input = object : InputStream() {
            private val prefix = ByteArrayInputStream(hello)
            override fun read(): Int {
                val next = prefix.read()
                if (next >= 0) return next
                close.await(); return -1
            }
            override fun close() { close.countDown() }
        }
        val output = ByteArrayOutputStream()
        val recovery = AtomicInteger()
        val errors = AtomicInteger()
        val run = WireConnection(WireTransport.streams(input, output),
            WireMessage.Hello(WireRole.PRODUCER, "producer", "client"), emptyList(), 32, 8, 100_000,
            { _, _ -> }, { _, _ -> recovery.incrementAndGet() }, { errors.incrementAndGet() })
        val format = AudioFormat(0, AudioRole.VOICE, AudioCodec.PCM_S16LE, 16000, 1)
        fun packet(pts: Long, session: Long = 2, stream: Int = 0) =
            AudioPacket(stream, SessionId(session), byteArrayOf(1, 2, 3, 4), pts)
        try {
            run.session(SessionState(SessionId(1), SessionPhase.ACTIVE))
            run.audioFormat(0, format); run.audioPacket(packet(0, session = 1))
            run.session(SessionState(SessionId(2), SessionPhase.ACTIVE))
            run.audioPacket(packet(0)) // The new session needs a new declaration.
            run.audioFormat(0, format); run.audioPacket(packet(1))
            run.audioFormat(0, null); run.audioPacket(packet(2))
            run.audioFormat(0, format)
            run.audioPacket(packet(3, session = 1)); run.audioPacket(packet(4, stream = 1))
            run.audioPacket(packet(5)); run.audioPacket(packet(6)); run.audioPacket(packet(7))
            run.audioPacket(packet(600_008)) // The duration bound drops both remaining old packets.
            run.audioPacket(packet(600_009))
            run.format(ScreenId.MAIN, testFormat())
            run.frame(VideoFrame(ScreenId.MAIN, SessionId(2), byteArrayOf(0, 0, 0, 2, 0x65, 0), 123, true, 456000))
            run.start()
            fun decoded(): List<WireMessage> = try {
                val source = ByteArrayInputStream(output.toByteArray())
                buildList { while (true) add(StreamWireCodec.read(source) ?: break) }
            } catch (_: WireFormatException) { emptyList() }
            eventually { decoded().filterIsInstance<WireMessage.Binary>().size == 3 }
            val binary = decoded().filterIsInstance<WireMessage.Binary>().map { it.value }
            val audio = binary.filter { it.header.kind == BinaryKind.AUDIO }
            assertEquals(listOf(600_008L, 600_009L), audio.map { it.header.ptsUs })
            assertEquals(listOf(0u, 1u), audio.map { it.header.sequence })
            assertTrue(audio.all { it.header.flags == 0 })
            assertEquals(1, binary.count { it.header.kind == BinaryKind.VIDEO })
            assertEquals(0, recovery.get()); assertEquals(0, errors.get())
        } finally { run.close() }
    }
    @Test fun audioPacketThenImmediateReplacementAndNullKeepOrder() {
        val hello = ByteArrayOutputStream().also {
            StreamWireCodec.write(it, WireMessage.Hello(WireRole.CONSUMER, "consumer", "client"))
        }.toByteArray()
        val close = CountDownLatch(1)
        val input = object : InputStream() {
            private val prefix = ByteArrayInputStream(hello)
            override fun read(): Int {
                val next = prefix.read()
                if (next >= 0) return next
                close.await(); return -1
            }
            override fun close() { close.countDown() }
        }
        val output = ByteArrayOutputStream()
        val recovery = AtomicInteger()
        val errors = AtomicInteger()
        val run = WireConnection(WireTransport.streams(input, output),
            WireMessage.Hello(WireRole.PRODUCER, "producer", "client"), emptyList(), 32, 8, 100_000,
            { _, _ -> }, { _, _ -> recovery.incrementAndGet() }, { errors.incrementAndGet() })
        val format = AudioFormat(0, AudioRole.VOICE, AudioCodec.PCM_S16LE, 16000, 1)
        fun packet(pts: Long, session: Long = 2, stream: Int = 0) =
            AudioPacket(stream, SessionId(session), byteArrayOf(1, 2, 3, 4), pts)
        try {
            run.session(SessionState(SessionId(2), SessionPhase.ACTIVE))
            run.audioFormat(0, format); run.audioPacket(packet(1))
            val replacement = AudioFormat(0, AudioRole.VOICE, AudioCodec.PCM_S16LE, 8000, 1)
            run.audioFormat(0, replacement); run.audioPacket(packet(2))
            run.audioFormat(0, null)
            run.start()
            fun decoded(): List<WireMessage> = try {
                val source = ByteArrayInputStream(output.toByteArray())
                buildList { while (true) add(StreamWireCodec.read(source) ?: break) }
            } catch (_: WireFormatException) { emptyList() }
            eventually { decoded().size == 7 }
            val events = decoded().drop(2)
            assertEquals(format.sampleRate, assertIs<WireMessage.AudioFormatMessage>(events[0]).format!!.sampleRate)
            assertEquals(1L, assertIs<WireMessage.Binary>(events[1]).value.header.ptsUs)
            assertEquals(8000, assertIs<WireMessage.AudioFormatMessage>(events[2]).format!!.sampleRate)
            assertEquals(2L, assertIs<WireMessage.Binary>(events[3]).value.header.ptsUs)
            assertNull(assertIs<WireMessage.AudioFormatMessage>(events[4]).format)
            assertEquals(0, errors.get()); assertEquals(0, recovery.get())
        } finally { run.close() }
    }
    @Test fun controlOverflowReportsOneFailureAndRejectsFurtherWork() {
        val errors = AtomicInteger()
        val run = WireConnection(WireTransport.streams(ByteArrayInputStream(byteArrayOf()), ByteArrayOutputStream()),
            WireMessage.Hello(WireRole.PRODUCER, "producer", "client"), emptyList(), 2, 12, 100_000,
            { _, _ -> }, { _, _ -> }, { errors.incrementAndGet() })
        try {
            assertTrue(run.enqueue(WireMessage.Keyframe(ScreenId.MAIN)))
            assertTrue(run.enqueue(WireMessage.Keyframe(ScreenId.MAIN)))
            assertFalse(run.enqueue(WireMessage.Keyframe(ScreenId.MAIN)))
            assertFalse(run.enqueue(WireMessage.Keyframe(ScreenId.MAIN)))
            eventually { errors.get() == 1 }
            run.close(); run.close()
            assertEquals(1, errors.get())
        } finally { run.close() }
    }
}
