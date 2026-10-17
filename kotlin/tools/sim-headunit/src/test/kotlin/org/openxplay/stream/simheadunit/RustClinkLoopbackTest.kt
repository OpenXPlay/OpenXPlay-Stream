// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.simheadunit

import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.Timeout
import org.openxplay.stream.clink.*
import org.openxplay.stream.wire.*
import java.nio.file.Files
import java.nio.file.Path
import java.util.Base64
import kotlinx.serialization.json.*
import org.junit.jupiter.api.Assumptions.assumeTrue
import org.openxplay.stream.core.*
import org.openxplay.stream.core.codec.Nal
import org.openxplay.stream.core.audio.Adts
import kotlin.test.*

@Timeout(20)
class RustClinkLoopbackTest {
    @Test
    fun rustOutputMatchesKotlinLoopback() {
        val repository = Path.of(checkNotNull(System.getProperty("sim.repository")))
        val binary = repository.resolve("../rust/target/debug/oxp-clink")
        assumeTrue(Files.isExecutable(binary), "Rust loopback skipped: run cargo build -p oxp-bin --offline in ../rust/ to build ../rust/target/debug/oxp-clink")
        val ports = freePorts()
        val stateDir = Files.createTempDirectory("rust-clink-loopback-")
        val clip = BaselineClip()
        val input = ClipInput(clip)
        val errors = CopyOnWriteArrayList<Throwable>()
        val diagnostics = CopyOnWriteArrayList<String>()
        val states = LinkedBlockingQueue<String>()
        val first = SimulatedHeadUnit(ports)
        var child: Process? = null
        var reader: Thread? = null
        lateinit var audioSink: InputSink
        val producer = object : ProjectionInput {
            override val capabilities = input.capabilities.copy(microphone = true)
            override fun start(config: SessionConfig, sink: InputSink) { audioSink = sink; input.start(config, sink) }
            override fun reconfigure(config: SessionConfig) = input.reconfigure(config)
            override fun control(event: ControlEvent) = input.control(event)
            override fun stop() = input.stop()
        }
        val hub = ProjectionHub(producer)
        try {
            val process = ProcessBuilder(binary.toString(), "--stdio", "--link", "static", "--head-unit", "127.0.0.1",
                "--local", "127.0.0.1", "--pin", "012345", "--state-dir", stateDir.toString(), "--port",
                "auth=${ports.auth},control=${ports.control},media=${ports.media},cert=${ports.cert},rtsp=${ports.rtsp},uibc=${ports.uibc},rtp=${ports.rtp}").start()
            child = process
            val stderr = Thread({
                try {
                    process.errorStream.bufferedReader().useLines { lines -> lines.forEach { line ->
                        diagnostics += line
                        val event = Json.parseToJsonElement(line).jsonObject
                        if (event["type"]?.jsonPrimitive?.content == "state") states.add(event.getValue("state").jsonPrimitive.content)
                        if (event["type"]?.jsonPrimitive?.content == "error") errors += AssertionError(line)
                    } }
                } catch (e: Exception) { errors += e }
            }, "rust-clink-stderr").apply { isDaemon = true; start() }
            reader = stderr
            val output = WireOutput(WireTransport.streams(process.inputStream, process.outputStream), id = "clink",
                inputCapabilities = producer.capabilities, onError = { errors += it })
            hub.start(); hub.addOutput(output)
            first.await("placeholder") { first.playing && first.accessUnits.isNotEmpty() &&
                first.configurationAnswered && first.requestedUibcPort == ports.uibc && first.requestedMediaPort == ports.media && first.heartbeats > 0 }
            assertTrue(first.authenticated)
            val saved = Json.parseToJsonElement(Files.readString(stateDir.resolve("trust.json"))).jsonObject
            assertContentEquals(first.peerKey, Base64.getDecoder().decode(saved.getValue("peers").jsonObject.getValue("127.0.0.1").jsonPrimitive.content))
            assertEquals(1280 to 720, first.accessUnits.first().size)
            assertTrue(first.accessUnits.first().keyFrame)
            val request = input.next<ControlEvent.KeyFrameRequest>()
            assertEquals(ScreenId.MAIN, request.screen)
            assertEquals(Origin("clink", "127.0.0.1"), request.origin)

            val before = first.accessUnits.size
            input.send(1) // A delta frame cannot replace the placeholder.
            first.await("placeholder continues after a delta frame") { first.accessUnits.size >= before + 2 }
            assertTrue(first.accessUnits.all { it.size == 1280 to 720 })
            input.send(0); input.send(1)
            first.await("live IDR") { first.accessUnits.any { it.size == 640 to 360 } }
            for (index in 2 until clip.frames.size) {
                input.send(index)
                first.await("clip AU ${index - 1}") { first.accessUnits.count { it.size == 640 to 360 } >= index }
            }
            // A following PES start completes the final zero-length video PES in the capture.
            input.send(0, clip.frames.size)
            first.await("all clip access units") { first.accessUnits.count { it.size == 640 to 360 } >= clip.frames.size }
            val received = first.accessUnits.filter { it.size == 640 to 360 }.take(clip.frames.size)
            assertEquals(60, received.size)
            assertTrue(received.first().keyFrame)
            received.zip(clip.frames).forEach { (actual, expected) ->
                val vcl = Nal.units(actual.data, Nal.Format.ANNEX_B).filter { it[0].toInt() and 31 in setOf(1, 5) }.toList()
                val source = Nal.units(expected).filter { it[0].toInt() and 31 in setOf(1, 5) }.toList()
                assertEquals(source.size, vcl.size)
                source.zip(vcl).forEach { (a, b) -> assertContentEquals(a, b) }
            }
            assertTrue(first.patCount >= 3 && first.pmtCount >= 3 && first.pcrCount > 0)
            assertTrue(first.accessUnits.zipWithNext().all { (a, b) -> b.pts90kHz > a.pts90kHz })

            val session = SessionId(1)
            val au = byteArrayOf(0x21, 0x10, 0x56, 0x78)
            audioSink.audioFormat(0, AudioFormat(0, AudioRole.MEDIA, AudioCodec.AAC_LC, 48000, 2,
                byteArrayOf(0x11, 0x90.toByte())))
            audioSink.audioPacket(AudioPacket(0, session, au, clip.frames.size * 33_333L))
            first.await("AAC in TS") { first.audioUnits.isNotEmpty() }
            assertEquals("AAC 00000001 00", first.selectedAudio)
            assertContentEquals(au, Adts.parse(first.audioUnits.first().data).single().data)
            val videoPts = first.accessUnits.last().pts90kHz
            assertTrue(kotlin.math.abs(first.audioUnits.first().pts90kHz - videoPts) <= 4500)
            audioSink.audioFormat(0, null)

            first.tailAckDelayMillis = 150
            val voice = ByteArray(320 * 8 * 2) { if (it % 2 == 0) 0xd2.toByte() else 4 }
            audioSink.audioFormat(1, AudioFormat(1, AudioRole.VOICE, AudioCodec.PCM_S16LE, 16000, 1))
            audioSink.audioPacket(AudioPacket(1, session, voice, 0))
            audioSink.audioFormat(1, null)
            first.await("voice stop") { first.audioCommands.any { it.method == 8 && it.fields.number(2) == 1uL } }
            val commands = first.audioCommands.filter { it.method == 8 }
            assertEquals(listOf(0uL, 1uL), commands.map { it.fields.number(2) })
            assertEquals(listOf(0, 0, 0, 0, 0, 3), first.voiceFrames.map { it.kind })
            assertContentEquals(voice, first.voiceFrames.fold(byteArrayOf()) { a, b -> a + b.data })
            assertTrue(commands.last().receivedNanos - first.tailAckNanos >= 20_000_000)

            audioSink.microphone(2, MicRequest(2, AudioCodec.PCM_S16LE, 16000, 1, 20))
            val automatic = input.next<ControlEvent.MicPacket>()
            assertEquals(2, automatic.stream); assertEquals(640, automatic.data.size)
            assertEquals(request.origin, automatic.origin)
            val mic = ByteArray(640) { 7 }
            first.microphone(mic)
            assertContentEquals(mic, input.next<ControlEvent.MicPacket>().data)
            audioSink.microphone(2, null)
            first.await("microphone stop") { first.audioCommands.any { it.method == 6 && it.fields.number(1) == 0uL } }
            first.awakenVoiceAssistant(true)
            assertEquals(ControlEvent.Key(KeyCode.VOICE, true, request.origin), input.next<ControlEvent.Key>())
            assertEquals(ControlEvent.Key(KeyCode.VOICE, false, request.origin), input.next<ControlEvent.Key>())

            input.events.clear()
            first.requestKeyframe()
            assertEquals(request.origin, input.next<ControlEvent.KeyFrameRequest>().origin)
            first.touch(0, listOf(PixelContact(41, 640, 360)))
            val center = input.next<ControlEvent.Touch>()
            assertEquals(request.origin, center.origin)
            assertEquals(ScreenId.MAIN, center.screen)
            assertEquals(1, center.contacts.size)
            assertEquals(0.5, center.contacts.single().x, 0.0001)
            assertEquals(0.5, center.contacts.single().y, 0.0001)
            assertTrue(center.contacts.single().pressed)
            first.touch(1, listOf(PixelContact(41, 640, 360)))
            assertFalse(input.next<ControlEvent.Touch>().contacts.single().pressed)

            val a = PixelContact(91, 320, 180); val b = PixelContact(12, 960, 540)
            first.touch(0, listOf(a))
            val down = input.next<ControlEvent.Touch>()
            first.touch(5 or (1 shl 8), listOf(a, b)) // ACTION_POINTER_DOWN, index 1.
            val two = input.next<ControlEvent.Touch>()
            assertContacts(listOf(TouchContact(0, 0.25, 0.25, true)), down.contacts)
            assertContacts(listOf(TouchContact(0, 0.25, 0.25, true), TouchContact(1, 0.75, 0.75, true)), two.contacts)
            first.touch(2, listOf(b.copy(x = 640), a.copy(x = 640))) // Wire order changes; slots stay stable.
            assertContacts(listOf(TouchContact(0, 0.5, 0.25, true), TouchContact(1, 0.5, 0.75, true)),
                input.next<ControlEvent.Touch>().contacts)
            first.touch(6 or (1 shl 8), listOf(b, a)) // Lift index 1, leaving track 12 pressed.
            assertContacts(listOf(TouchContact(0, 0.25, 0.25, false), TouchContact(1, 0.75, 0.75, true)),
                input.next<ControlEvent.Touch>().contacts)
            first.touch(1, listOf(b))
            assertContacts(listOf(TouchContact(1, 0.75, 0.75, false)), input.next<ControlEvent.Touch>().contacts)
            first.key(66, 0); first.key(66, 1)
            assertEquals(ControlEvent.Key(KeyCode.SELECT, true, request.origin), input.next<ControlEvent.Key>())
            assertEquals(ControlEvent.Key(KeyCode.SELECT, false, request.origin), input.next<ControlEvent.Key>())

            first.assertHealthy(); first.disconnect()
            val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(3)
            while (true) {
                val remaining = deadline - System.nanoTime()
                assertTrue(remaining > 0, "m23 did not end Rust session: $diagnostics")
                val state = states.poll(remaining, TimeUnit.NANOSECONDS) ?: fail("m23 did not end Rust session: $diagnostics")
                if (state == "Ended") break
            }
            // Session ENDED joins Rust workers; closing Wire stdin then ends the shell.
            hub.stop()
            assertTrue(process.waitFor(3, TimeUnit.SECONDS), "Rust process did not exit: $diagnostics")
            stderr.join(1000)
            assertFalse(stderr.isAlive, "stderr reader leaked")
            assertEquals(0, process.exitValue(), diagnostics.joinToString("\n"))
            assertTrue(errors.isEmpty(), errors.joinToString("\n"))
        } finally {
            first.expectRelayClose()
            child?.let { if (it.isAlive) it.destroyForcibly() }
            hub.stop()
            child?.waitFor(2, TimeUnit.SECONDS)
            reader?.join(1000)
            try { first.close() } finally {
                Files.walk(stateDir).use { paths -> paths.sorted(Comparator.reverseOrder()).forEach { Files.deleteIfExists(it) } }
            }
        }
    }

    /** Wire touch coordinates are quantized to 16 bits, so compare within one quantization step. */
    private fun assertContacts(expected: List<TouchContact>, actual: List<TouchContact>) {
        assertEquals(expected.size, actual.size, "contact count")
        expected.zip(actual).forEach { (e, a) ->
            assertEquals(e.id, a.id); assertEquals(e.pressed, a.pressed)
            assertEquals(e.x, a.x, 1e-4); assertEquals(e.y, a.y, 1e-4)
        }
    }
}
