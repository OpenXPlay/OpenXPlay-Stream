// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

package org.openxplay.stream.cast

import java.io.ByteArrayOutputStream
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import org.junit.jupiter.api.AfterEach
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.Timeout
import org.openxplay.stream.core.*
import org.openxplay.stream.core.codec.HevcConfig
import org.openxplay.stream.core.codec.Nal
import kotlin.test.*

@Timeout(15)
class CastHevcLoopbackTest {
    @Test fun hevcPlaysWithParameterSetsAndRandomAccessInMpegTs() {
        FakeRenderer().use { fixture ->
            val states = CopyOnWriteArrayList<CastState>(); val failures = CopyOnWriteArrayList<CastFailure>()
            val threads = TrackedThreads()
            val output = fixture.output(object : CastListener {
                override fun state(state: CastState, renderer: CastRenderer?) { states += state }
                override fun failure(failure: CastFailure) { failures += failure }
            }, threads, audio = false)
            try {
                output.open(NO_HOST)
                await("Renderer not discovered") { states.lastOrNull() == CastState.WAITING_FOR_VIDEO }
                output.onSession(SessionState(SessionId(1), SessionPhase.ACTIVE), HEVC_SESSION)
                output.onVideoFormat(ScreenId.MAIN, hevcFormat()); output.onVideoFrame(hevcFrame())
                await("HEVC stream incomplete: $states $failures") { fixture.streams.values.any { it.size >= 6 } }
                await("HEVC did not play: $states $failures") { states.lastOrNull() == CastState.PLAYING }
                assertEquals(listOf(CastState.SEARCHING, CastState.WAITING_FOR_VIDEO, CastState.STARTING, CastState.PLAYING), states.take(4))
                assertHevcStream(fixture.streams.getValue(fixture.uris.single()).toList())
                assertTrue(failures.isEmpty(), failures.joinToString())
                assertTrue(fixture.errors.isEmpty(), fixture.errors.joinToString())
            } finally { output.close(); threads.awaitEnded() }
        }
    }

    @Test fun switchingAvcToHevcRepushesANewUrlAndIdenticalHevcDoesNotRestart() {
        FakeRenderer().use { fixture ->
            val states = CopyOnWriteArrayList<CastState>(); val failures = CopyOnWriteArrayList<CastFailure>()
            val threads = TrackedThreads()
            val output = fixture.output(object : CastListener {
                override fun state(state: CastState, renderer: CastRenderer?) { states += state }
                override fun failure(failure: CastFailure) { failures += failure }
            }, threads, audio = false)
            try {
                output.open(NO_HOST)
                output.onSession(SessionState(SessionId(1), SessionPhase.ACTIVE), SESSION_CONFIG)
                output.onVideoFormat(ScreenId.MAIN, videoFormat()); output.onVideoFrame(frame())
                await("AVC did not play") { states.lastOrNull() == CastState.PLAYING && fixture.packets.size >= 3 }
                val firstUri = fixture.uris.single()
                assertEquals(0x1b, streamType(fixture.streams.getValue(firstUri)[1]))
                val starts = states.count { it == CastState.STARTING }
                output.onVideoFormat(ScreenId.MAIN, hevcFormat()); output.onVideoFrame(hevcFrame())
                await("HEVC generation did not play: $states $failures") {
                    fixture.uris.size == 2 && states.lastOrNull() == CastState.PLAYING &&
                        fixture.streams[fixture.uris.last()]?.size?.let { it >= 6 } == true
                }
                val secondUri = fixture.uris.last()
                assertNotEquals(firstUri, secondUri)
                assertEquals(starts + 1, states.count { it == CastState.STARTING })
                assertEquals(2, fixture.actions.count { it == "SetAVTransportURI" })
                assertEquals(2, fixture.actions.count { it == "Play" })
                assertHevcStream(fixture.streams.getValue(secondUri).toList())
                val packetCount = fixture.streams.getValue(secondUri).size
                output.onVideoFormat(ScreenId.MAIN, hevcFormat())
                output.onVideoFrame(hevcFrame())
                await("Identical HEVC format interrupted playback") {
                    fixture.streams.getValue(secondUri).size > packetCount || fixture.uris.size > 2
                }
                assertEquals(2, fixture.uris.size)
                assertTrue(failures.isEmpty(), failures.joinToString())
                assertTrue(fixture.errors.isEmpty(), fixture.errors.joinToString())
            } finally { output.close(); threads.awaitEnded() }
        }
    }

    private fun assertHevcStream(packets: List<ByteArray>) {
        packets.forEach { assertEquals(188, it.size); assertEquals(0x47, u8(it[0])) }
        assertEquals(listOf(0, 0x100, 0x101), packets.take(3).map(::pid))
        assertEquals(0, u8(section(packets[0])[0])); assertEquals(2, u8(section(packets[1])[0]))
        assertEquals(0x24, streamType(packets[1]))
        val first = packets[2]
        assertTrue(u8(first[1]) and 0x40 != 0)
        assertEquals(3, (u8(first[3]) ushr 4) and 3)
        assertTrue(u8(first[4]) >= 1 && u8(first[5]) and 0x40 != 0)
        val pes = ByteArrayOutputStream()
        for (packet in packets.drop(2)) {
            if (pid(packet) != 0x101) continue
            if (pes.size() > 0 && u8(packet[1]) and 0x40 != 0) break
            pes.write(payload(packet))
        }
        val bytes = pes.toByteArray()
        assertContentEquals(byteArrayOf(0, 0, 1, 0xe0.toByte()), bytes.copyOfRange(0, 4))
        val units = Nal.units(bytes.copyOfRange(9 + u8(bytes[8]), bytes.size), Nal.Format.ANNEX_B).toList()
        // A Xiaomi DLNA renderer stopped H.265 playback after tens of seconds without access unit delimiters.
        assertEquals(listOf(35, 32, 33, 34, 19), units.map { (u8(it[0]) ushr 1) and 63 })
        val config = HevcConfig.parse(hevcFormat().config)
        (config.vps + config.sps + config.pps).zip(units.drop(1)).forEach { (expected, actual) -> assertContentEquals(expected, actual) }
        assertContentEquals(Nal.units(hevcFrame().data).single(), units.last())
    }
    private fun streamType(packet: ByteArray): Int = u8(section(packet)[12])
    private fun section(packet: ByteArray): ByteArray {
        val bytes = payload(packet)
        return bytes.copyOfRange(1 + u8(bytes[0]), bytes.size)
    }
    private fun u8(byte: Byte): Int = byte.toInt() and 0xff
    private fun pid(packet: ByteArray): Int = ((u8(packet[1]) and 31) shl 8) or u8(packet[2])
    private fun payload(packet: ByteArray): ByteArray {
        val offset = if ((u8(packet[3]) ushr 4) and 2 != 0) 5 + u8(packet[4]) else 4
        return packet.copyOfRange(offset, packet.size)
    }
}

@Timeout(10)
class CastHevcOutputContractTest : ProjectionOutputContract() {
    private val fixture = FakeRenderer()
    private val threads = TrackedThreads()
    private val ended = CountDownLatch(1)
    private var stimulations = 0
    override fun createOutput(): ProjectionOutput = fixture.output(object : CastListener {
        override fun state(state: CastState, renderer: CastRenderer?) { if (state == CastState.ENDED) ended.countDown() }
    }, threads, audio = false)
    override fun videoFormat(): VideoFormat = hevcFormat()
    override fun stimulate(output: ProjectionOutput) {
        stimulations++
        output.onSession(SessionState(SessionId(2), SessionPhase.ACTIVE), HEVC_SESSION)
        output.onVideoFormat(ScreenId.MAIN, videoFormat()); output.onVideoFrame(hevcFrame(session = 2))
    }
    override fun awaitIdle() {
        if (stimulations == 1) await("HEVC contract player not started") { fixture.packets.size >= 6 }
        else { assertTrue(ended.await(3, TimeUnit.SECONDS)); threads.awaitEnded() }
    }
    @AfterEach fun closePeer() { fixture.close() }
}

private val HEVC_SESSION = SessionConfig(mapOf(ScreenId.MAIN to ScreenConfig(VideoCodec.H265, 1920, 1080, 30.0)))
private fun hevcFormat(): VideoFormat = VideoFormat.create(ScreenId.MAIN, VideoCodec.H265, vector("hevc-main-1920x1080"))
private fun hevcFrame(session: Long = 1): VideoFrame {
    val nal = ByteArray(600) { 0x55 }.also { it[0] = (19 shl 1).toByte(); it[1] = 1; it[2] = 0x80.toByte() }
    return VideoFrame(ScreenId.MAIN, SessionId(session), Nal.annexBToAvcc(byteArrayOf(0, 0, 1) + nal), 0, true, System.nanoTime())
}
