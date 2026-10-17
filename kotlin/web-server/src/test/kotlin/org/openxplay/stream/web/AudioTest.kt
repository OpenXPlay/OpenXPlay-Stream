// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.web

import java.net.Socket
import java.net.SocketTimeoutException
import java.nio.file.Path
import java.util.concurrent.CopyOnWriteArrayList
import kotlinx.serialization.json.*
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.io.TempDir
import org.openxplay.stream.core.*
import org.openxplay.stream.core.wire.*
import kotlin.test.*

class AudioTest {
    @TempDir lateinit var root: Path
    private fun format(codec: AudioCodec = AudioCodec.PCM_S16LE) = AudioFormat(1, AudioRole.VOICE, codec, 16000, 1)
    private fun packet(pts: Long, size: Int = 640) = AudioPacket(1, SessionId(1), ByteArray(size), pts)

    @Test fun queueDropsOldestBySamplesAndTimestampSpan() {
        val queue = AudioQueue(format())
        repeat(40) { queue.offer(packet(it * 20_000L)) }
        assertEquals(300_000L, queue.poll()!!.ptsUs)
        var remaining = 0
        while (queue.poll() != null) remaining++
        assertEquals(24, remaining)
        queue.offer(packet(0)); queue.offer(packet(1_000_000))
        assertEquals(1_000_000L, queue.poll()!!.ptsUs); assertNull(queue.poll())
        queue.offer(packet(2_000_000, 16002)); assertNull(queue.poll())
        val aac = AudioQueue(AudioFormat(1, AudioRole.MEDIA, AudioCodec.AAC_LC, 48000, 2))
        repeat(30) { aac.offer(packet(it * 21_333L, 5)) }
        assertEquals(7 * 21_333L, aac.poll()!!.ptsUs)
    }

    @Test fun queueBoundsTheWholeClientAndInvalidatesOnlyChangedStreams() {
        val queue = AudioQueue()
        val first = format()
        val second = AudioFormat(2, AudioRole.ALERT, AudioCodec.PCM_S16LE, 16000, 1)
        repeat(20) {
            queue.offer(packet(it * 20_000L), first)
            queue.offer(AudioPacket(2, SessionId(1), ByteArray(640), it * 20_000L), second)
        }
        var count = 0
        while (queue.poll() != null) count++
        assertEquals(25, count)
        queue.offer(packet(0), first)
        queue.offer(AudioPacket(2, SessionId(1), ByteArray(640), 0), second)
        val generation = queue.generation(1)
        queue.remove(1)
        assertTrue(queue.generation(1) != generation)
        assertEquals(2, queue.poll()!!.stream); assertNull(queue.poll())
    }

    @Test fun packetThenImmediateNullAndReplacementKeepOrder() {
        val queue = AudioQueue(format())
        val first = WebJsonCodec.audio(1, format())
        val replacement = WebJsonCodec.audio(1, AudioFormat(1, AudioRole.VOICE, AudioCodec.PCM_S16LE, 8000, 1))
        val end = WebJsonCodec.audio(1, null)
        assertTrue(queue.format(first)); queue.offer(packet(0))
        assertTrue(queue.format(replacement)); queue.offer(packet(20_000))
        assertTrue(queue.format(end))
        assertNull(queue.poll())
        assertEquals(first, queue.pollFormat())
        assertEquals(0L, queue.poll()!!.ptsUs)
        assertEquals(replacement, queue.pollFormat())
        assertEquals(20_000L, queue.poll()!!.ptsUs)
        assertEquals(end, queue.pollFormat()); assertNull(queue.poll())
    }

    @Test fun overflowDropsOldestPacketsAcrossRunMarkers() {
        val queue = AudioQueue(format())
        repeat(25) { queue.offer(packet(it * 20_000L)) }
        val end = WebJsonCodec.audio(1, null)
        val start = WebJsonCodec.audio(1, format())
        assertTrue(queue.format(end)); assertTrue(queue.format(start))
        queue.offer(packet(0)) // A new run resets timestamps; the sample-duration bound still applies.
        repeat(24) { assertEquals((it + 1) * 20_000L, queue.poll()!!.ptsUs) }
        assertEquals(end, queue.pollFormat()); assertEquals(start, queue.pollFormat())
        assertEquals(0L, queue.poll()!!.ptsUs); assertNull(queue.poll())
    }

    @Test fun validatesWholeTwentyMillisecondFrames() {
        val request = MicRequest(1, AudioCodec.PCM_S16LE, 16000, 1, 20)
        assertTrue(validMicPacket(request, 640)); assertTrue(validMicPacket(request, 1280))
        for (bytes in listOf(0, 1, 639, 641)) assertFalse(validMicPacket(request, bytes))
        assertFalse(validMicPacket(request.copy(codec = AudioCodec.AAC_LC), 640))
        assertFalse(validMicPacket(request.copy(frameMs = 10), 640))
        assertFalse(validMicPacket(request.copy(sampleRate = 0), 640))
        assertFalse(validMicPacket(request.copy(channels = 0), 640))
        assertTrue(validMicPacket(request.copy(sampleRate = 48000, channels = 2), 3840))
    }

    private fun TestClient.advertise(codecs: List<AudioCodec>, mic: Boolean) {
        val hello = WebJsonCodec.parse(vector("hello"))
        send(1, JsonObject(hello + mapOf("audio" to JsonArray(codecs.map { JsonPrimitive(it.name) }),
            "microphone" to JsonPrimitive(mic))).toString().toByteArray())
    }
    private fun TestClient.noMessage() {
        socket.soTimeout = 150
        try { assertFailsWith<SocketTimeoutException> { read() } }
        finally { socket.soTimeout = 5000 }
    }

    @Test fun rejectsWrongStreamPartialFramesAndUnadvertisedMicrophone() {
        val controls = CopyOnWriteArrayList<ControlEvent>()
        val web = WebOutput(WebOutputConfig(port = 0, staticAssetRoot = root,
            tokenVerifier = TokenVerifier { true }, sourceUrl = "https://example.org/source"))
        web.open(object : OutputHost {
            override fun capabilities(capabilities: OutputCapabilities): Unit = Unit
            override fun control(event: ControlEvent) { controls.add(event) }
            override fun stats(stats: OutputStats): Unit = Unit
        })
        web.onMicrophone(1, MicRequest(1, AudioCodec.PCM_S16LE, 16000, 1, 20))
        try {
            val port = web.awaitListening()
            for ((stream, bytes, advertised) in listOf(Triple(2, 640, true), Triple(1, 639, true), Triple(1, 640, false))) {
                TestClient(Socket("127.0.0.1", port)).use { client ->
                    client.advertise(emptyList(), advertised); client.json()
                    if (advertised) client.json()
                    client.send(2, OxpWebCodec.encode(BinaryMessage(
                        BinaryHeader(BinaryKind.MICROPHONE, stream, 0, 0u, 0, 0), ByteArray(bytes))))
                    assertContentEquals(closePayload(1008), client.read().payload)
                }
            }
            web.awaitHostIdle(); assertTrue(controls.isEmpty())
        } finally { web.close() }
    }

    @Test fun codecFilteringLateJoinPauseMicAndCapabilityUnion() {
        val caps = CopyOnWriteArrayList<OutputCapabilities>()
        val controls = CopyOnWriteArrayList<ControlEvent>()
        val observedStats = CopyOnWriteArrayList<OutputStats>()
        val web = WebOutput(WebOutputConfig(port = 0, staticAssetRoot = root,
            tokenVerifier = TokenVerifier { true }, sourceUrl = "https://example.org/source"))
        web.open(object : OutputHost {
            override fun capabilities(capabilities: OutputCapabilities) { caps.add(capabilities) }
            override fun control(event: ControlEvent) { controls.add(event) }
            override fun stats(stats: OutputStats) { observedStats.add(stats) }
        })
        web.onSession(SessionState(SessionId(1), SessionPhase.ACTIVE), SessionConfig(emptyMap()))
        web.onAudioFormat(1, format())
        val request = MicRequest(1, AudioCodec.PCM_S16LE, 16000, 1, 20)
        web.onMicrophone(1, request)
        try {
            val port = web.awaitListening()
            TestClient(Socket("127.0.0.1", port)).use { pcm ->
                pcm.advertise(listOf(AudioCodec.PCM_S16LE), true)
                val welcome = pcm.json()
                assertEquals(WebJsonCodec.audio(1, format()), pcm.json())
                assertEquals(WebJsonCodec.microphone(1, request), pcm.json())
                TestClient(Socket("127.0.0.1", port)).use { aac ->
                    aac.advertise(listOf(AudioCodec.AAC_LC), false); aac.json(); aac.noMessage()
                    eventually { caps.last().audio == setOf(AudioCodec.PCM_S16LE, AudioCodec.AAC_LC) && caps.last().microphone }
                    pcm.send(1, vector("pause").toByteArray()); pcm.send(1, vector("ping").toByteArray()); pcm.json()
                    web.onAudioPacket(packet(20_000))
                    web.onAudioFormat(1, null)
                    val audio = OxpWebCodec.decode(pcm.read().payload)
                    assertEquals(BinaryKind.AUDIO, audio.header.kind); assertEquals(1, audio.header.stream)
                    assertEquals(20_000L, audio.header.ptsUs)
                    assertEquals(WebJsonCodec.audio(1, null), pcm.json()); aac.noMessage()
                    fun microphone(bytes: Int, stream: Int = 1) = pcm.send(2, OxpWebCodec.encode(BinaryMessage(
                        BinaryHeader(BinaryKind.MICROPHONE, stream, 0, 0u, 123, 123), ByteArray(bytes))))
                    microphone(640)
                    eventually { controls.any { it is ControlEvent.MicPacket } }
                    val event = controls.filterIsInstance<ControlEvent.MicPacket>().single()
                    assertEquals(Origin("web", welcome["clientId"]!!.jsonPrimitive.content), event.origin)
                    assertEquals(123L, event.ptsUs); assertEquals(640, event.data.size)
                    pcm.send(1, vector("stats").toByteArray()); eventually { observedStats.isNotEmpty() }
                    assertEquals(60_000L, observedStats.last().audioBufferedUs); assertEquals(2L, observedStats.last().audioUnderruns)
                    web.onMicrophone(1, null); assertEquals(WebJsonCodec.microphone(1, null), pcm.json())
                    microphone(640); assertContentEquals(closePayload(1008), pcm.read().payload)
                    eventually { caps.last().audio == setOf(AudioCodec.AAC_LC) && !caps.last().microphone }
                    web.onAudioFormat(1, format(AudioCodec.AAC_LC)); assertEquals(WebJsonCodec.audio(1, format(AudioCodec.AAC_LC)), aac.json())
                    web.onAudioPacket(packet(40_000, 8)); assertEquals(BinaryKind.AUDIO, OxpWebCodec.decode(aac.read().payload).header.kind)
                    web.onAudioFormat(1, null); assertEquals(WebJsonCodec.audio(1, null), aac.json())
                }
                eventually { caps.last().audio.isEmpty() && !caps.last().microphone }
            }
        } finally { web.close() }
    }
}
