// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14.

package org.openxplay.stream.replay

import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.io.TempDir
import org.openxplay.stream.core.*
import org.openxplay.stream.core.audio.*
import kotlin.test.*

public class ReplayAudioTest {
    @TempDir public lateinit var directory: Path

    private fun wav(rate: Int = 16000, channels: Int = 1, frames: Int = 480): Path {
        val path = directory.resolve("voice-$rate-$channels.wav")
        WavWriter(path, rate, channels).use { it.write(ByteArray(frames * channels * 2) { i -> (i % 127).toByte() }) }
        return path
    }

    @Test
    public fun wavRoundTripsRatesChannelsAndRejectsMalformedFiles(): Unit {
        for (rate in listOf(8000, 11025, 16000, 44100, 48000)) for (channels in 1..2) {
            val bytes = Files.readAllBytes(wav(rate, channels))
            val clip = WavClip.parse(bytes)
            assertEquals(rate, clip.sampleRate)
            assertEquals(channels, clip.channels)
            assertEquals(480, clip.frames)
            assertContentEquals(bytes.copyOfRange(44, bytes.size), clip.pcm)
            for (offset in listOf(0, 4, 8, 20, 22, 24, 28, 32, 34, 40)) {
                val broken = bytes.copyOf().also { it[offset] = 0x7f }
                assertFailsWith<IllegalArgumentException>("offset $offset") { WavClip.parse(broken) }
            }
            assertFailsWith<IllegalArgumentException> { WavClip.parse(bytes.copyOf(bytes.size - 1)) }
        }
        assertFailsWith<IllegalArgumentException> { WavClip.parse(ByteArray(0)) }
    }

    @Test
    public fun wavSkipsOddUnknownChunksAndRejectsDuplicateOrMissingChunks(): Unit {
        val bytes = Files.readAllBytes(wav())
        fun riff(body: ByteArray): ByteArray = bytes.copyOfRange(0, 12).also {
            val size = body.size + 4
            repeat(4) { i -> it[4 + i] = (size ushr (i * 8)).toByte() }
        } + body
        val fmt = bytes.copyOfRange(12, 36)
        val data = bytes.copyOfRange(36, bytes.size)
        val junk = byteArrayOf(74, 85, 78, 75, 1, 0, 0, 0, 42, 0)
        assertContentEquals(WavClip.parse(bytes).pcm, WavClip.parse(riff(junk + data + fmt)).pcm)
        for (body in listOf(fmt + fmt + data, fmt + data + data, fmt, data, junk.copyOf(9))) {
            assertFailsWith<IllegalArgumentException> { WavClip.parse(riff(body)) }
        }
        val incomplete = data.copyOf(data.size - 1)
        val size = incomplete.size - 8
        repeat(4) { incomplete[4 + it] = (size ushr (8 * it)).toByte() }
        assertFailsWith<IllegalArgumentException> { WavClip.parse(riff(fmt + incomplete + byteArrayOf(0))) }
    }

    @Test
    public fun adtsConfigRawUnitsPacingAndLooping(): Unit {
        val config = AudioSpecificConfig(2, 48000, 2)
        val first = byteArrayOf(1, 2, 3)
        val second = byteArrayOf(4, 5)
        val bytes = Adts.buildHeader(config, first.size) + first + Adts.buildHeader(config, second.size) + second
        val parsed = AdtsClip.parse(bytes)
        assertContentEquals(byteArrayOf(0x11, 0x90.toByte()), parsed.format.config)
        assertContentEquals(first, parsed.units[0])
        for (invalid in listOf(ByteArray(0), bytes.copyOf(bytes.size - 1),
            Adts.buildHeader(AudioSpecificConfig(2, 44100, 2), 1) + byteArrayOf(1),
            Adts.buildHeader(AudioSpecificConfig(1, 48000, 2), 1) + byteArrayOf(1),
            Adts.buildHeader(AudioSpecificConfig(2, 48000, 1), 1) + byteArrayOf(1))) {
            assertFailsWith<IllegalArgumentException> { AdtsClip.parse(invalid) }
        }
        val path = directory.resolve("media.aac").also { Files.write(it, bytes) }
        val sink = AudioSink()
        ReplayAudioRun(ReplayAudioOptions(media = path), sink, SessionId(1), true, {}).use { run ->
            run.start()
            run.tick(0, 1_000_000)
            assertEquals(21_333_333L, run.nextDeadline())
            run.tick(21_333_332, 1_021_333)
            assertEquals(1, sink.packets.size)
            run.tick(21_333_333, 1_021_333)
            run.tick(42_666_666, 1_042_666)
            assertEquals(listOf(1_000_000L, 1_021_333L, 1_042_666L), sink.packets.map { it.ptsUs })
            assertContentEquals(first, sink.packets[2].data)
            assertEquals(1, sink.formats.size)
            run.end()
            assertNull(sink.formats.last().second)
        }
    }

    @Test
    public fun voiceTurnsPadEndResetClockAndGateMicrophone(): Unit {
        val sink = AudioSink()
        val summaries = mutableListOf<MicSummary>()
        val recording = directory.resolve("microphone.wav")
        ReplayAudioRun(ReplayAudioOptions(voice = wav(), voiceEverySeconds = 0.1,
            micRate = 16000, micOut = recording), sink, SessionId(2), true, { summaries.add(it) }).use { run ->
            run.start()
            val mic = ControlEvent.MicPacket(1, ByteArray(640), 0, origin)
            assertFalse(run.accepts(mic))
            run.tick(0, 1_000_000)
            assertEquals(MicRequest(1, AudioCodec.PCM_S16LE, 16000, 1, 20), sink.requests.last())
            assertTrue(run.accepts(mic))
            assertFalse(run.accepts(ControlEvent.MicPacket(0, ByteArray(640), 0, origin)))
            assertFalse(run.accepts(ControlEvent.MicPacket(1, ByteArray(2), 0, origin)))
            run.receive(mic)
            run.tick(20_000_000, 1_020_000)
            assertEquals(640, sink.packets.last().data.size)
            assertTrue(sink.packets.last().data.takeLast(320).all { it == 0.toByte() })
            run.tick(40_000_000, 1_040_000)
            assertNull(sink.formats.last().second)
            assertNull(sink.requests.last())
            assertFalse(run.accepts(mic))
            run.tick(100_000_000, 1_100_000)
            assertEquals(1_100_000L, sink.packets.last().ptsUs)
            run.tick(1_000_000_000, 2_000_000)
            assertEquals(1L, summaries.single().packets)
            assertNull(summaries.single().rmsDbfs)
            run.end()
            assertNull(sink.requests.last())
        }
        assertEquals(320, WavClip.parse(Files.readAllBytes(recording)).frames)
    }

    @Test
    public fun oddRateVoiceUsesRationalTwentyMillisecondSamples(): Unit {
        val sink = AudioSink()
        ReplayAudioRun(ReplayAudioOptions(voice = wav(11025, 2, 1000)), sink, SessionId(1), true, {}).use { run ->
            run.start()
            repeat(4) { run.tick(it * 20_000_000L, 1_000_000 + it * 20_000L) }
            assertEquals(listOf(880, 884, 880, 884), sink.packets.map { it.data.size })
            assertEquals(listOf(1_000_000L, 1_019_954L, 1_040_000L, 1_059_954L), sink.packets.map { it.ptsUs })
            run.end()
        }
    }

    @Test
    public fun finiteAudioEndsAndLatePacketsReanchor(): Unit {
        val config = AudioSpecificConfig(2, 48000, 2)
        val media = directory.resolve("single.aac")
        Files.write(media, Adts.buildHeader(config, 1) + byteArrayOf(1))
        val sink = AudioSink()
        ReplayAudioRun(ReplayAudioOptions(media = media, voice = wav(frames = 160)),
            sink, SessionId(1), false, {}).use { run ->
            run.start()
            run.tick(0, 1_000_000)
            assertEquals(2, sink.packets.size)
            assertTrue(sink.formats.filter { it.second == null }.map { it.first }.containsAll(listOf(0, 1)))
            run.tick(200_000_000, 1_200_000)
            assertEquals(2, sink.packets.size)
            run.end()
        }
        val clock = AudioClock(48000)
        assertEquals(1_000_000L, clock.timestamp(1024, 1_000_000))
        assertEquals(1_300_000L, clock.timestamp(1024, 1_300_000))
        assertEquals(1_321_333L, clock.timestamp(1024, 1_321_333))
    }

    @Test
    public fun oddMicrophoneRateAcceptsWholeRationalFrames(): Unit {
        val sink = AudioSink()
        ReplayAudioRun(ReplayAudioOptions(micRate = 11025), sink, SessionId(1), true, {}).use { run ->
            run.start()
            assertEquals(11025, sink.requests.single()!!.sampleRate)
            assertTrue(run.accepts(ControlEvent.MicPacket(1, ByteArray(440), 0, origin)))
            assertTrue(run.accepts(ControlEvent.MicPacket(1, ByteArray(442), 0, origin)))
            assertFalse(run.accepts(ControlEvent.MicPacket(1, ByteArray(444), 0, origin)))
            run.end()
        }
    }

    @Test
    public fun rmsWeightsSamplesAndSeparatesOrigins(): Unit {
        val stats = MicAccumulator()
        val half = byteArrayOf(0, 64, 0, -64)
        stats.add(ControlEvent.MicPacket(1, half, 0, origin))
        stats.add(ControlEvent.MicPacket(1, half, 20_000, origin))
        stats.add(ControlEvent.MicPacket(1, ByteArray(2), 0, Origin("other", "client")))
        val values = stats.drain()
        assertEquals(2L, values[0].packets)
        assertEquals(-6.020599913, values[0].rmsDbfs!!, 0.000001)
        assertNull(values[1].rmsDbfs)
        assertTrue(stats.drain().isEmpty())
    }

    @Test
    public fun workerPacesAdtsAndFinalizesRecordingBeforeRestart(): Unit {
        val config = AudioSpecificConfig(2, 48000, 2)
        val media = directory.resolve("paced.aac")
        Files.write(media, Adts.buildHeader(config, 1) + byteArrayOf(1))
        val recording = directory.resolve("recording.wav")
        val input = AnnexBFileInput(clipBytes(), audio = ReplayAudioOptions(media = media,
            micRate = 16000, micOut = recording))
        val arrivals = CopyOnWriteArrayList<Long>()
        val packets = CopyOnWriteArrayList<AudioPacket>()
        val ready = CountDownLatch(4)
        val requested = CountDownLatch(1)
        try {
            input.start(configuration(), object : TestSink() {
                override fun audioPacket(packet: AudioPacket) {
                    arrivals.add(System.nanoTime()); packets.add(packet); ready.countDown()
                }
                override fun microphone(stream: Int, request: MicRequest?) {
                    if (request != null) requested.countDown()
                }
            })
            requested.awaitResult()
            assertTrue(input.control(ControlEvent.MicPacket(1, ByteArray(640) { 42 }, 0, origin)))
            ready.awaitResult()
            input.stop()
            input.awaitStopped()
            assertTrue(arrivals[3] - arrivals[0] >= 45_000_000L)
            assertEquals(320, WavClip.parse(Files.readAllBytes(recording)).frames)
            packets.take(4).zipWithNext().forEach { (a, b) ->
                assertTrue(b.ptsUs - a.ptsUs in 21333L..21334L)
            }
            assertTrue(packets[0].ptsUs > 0)
            val restarted = CountDownLatch(1)
            input.start(configuration(), object : TestSink() {
                override fun microphone(stream: Int, request: MicRequest?) {
                    if (request != null) restarted.countDown()
                }
            })
            restarted.awaitResult()
            assertTrue(input.control(ControlEvent.MicPacket(1, ByteArray(640) { 7 }, 0, origin)))
            input.stop()
            input.awaitStopped()
            assertTrue(WavClip.parse(Files.readAllBytes(recording)).pcm.all { it == 7.toByte() })
        } finally { input.stop(); input.awaitStopped() }
    }

    @Test
    public fun microphoneOnlyRequestsWhileRunningAndStopFencesPackets(): Unit {
        val input = AnnexBFileInput(clipBytes(), audio = ReplayAudioOptions(micRate = 16000))
        val requests = CopyOnWriteArrayList<MicRequest?>()
        val started = CountDownLatch(1)
        try {
            input.start(configuration(), object : TestSink() {
                override fun microphone(stream: Int, request: MicRequest?) {
                    requests.add(request)
                    if (request != null) started.countDown()
                }
            })
            started.awaitResult()
            assertTrue(input.control(ControlEvent.MicPacket(1, ByteArray(640), 0, origin)))
            input.stop()
            assertNull(requests.last())
            assertFalse(input.control(ControlEvent.MicPacket(1, ByteArray(640), 0, origin)))
            val count = requests.size
            Thread.sleep(30)
            assertEquals(count, requests.size)
        } finally { input.stop() }
    }
}

private class AudioSink : TestSink() {
    val formats = mutableListOf<Pair<Int, AudioFormat?>>()
    val packets = mutableListOf<AudioPacket>()
    val requests = mutableListOf<MicRequest?>()
    override fun audioFormat(stream: Int, format: AudioFormat?) { formats.add(stream to format) }
    override fun audioPacket(packet: AudioPacket) { packets.add(packet) }
    override fun microphone(stream: Int, request: MicRequest?) { requests.add(request) }
}
