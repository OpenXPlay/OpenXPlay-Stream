// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

package org.openxplay.stream.replay

import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.io.TempDir
import org.openxplay.stream.core.*
import org.openxplay.stream.core.codec.HevcConfig
import org.openxplay.stream.core.codec.Nal
import kotlin.test.*

public class HevcReplayTest {
    @Test public fun detectsHevcAndSplitsFirstSlicesWithAndWithoutAud(): Unit {
        for (aud in listOf(false, true)) {
            val clip = AnnexBClip.parse(hevcClip(aud))
            assertEquals(VideoCodec.H265, clip.format.codec)
            assertEquals("hvc1.1.6.L120.90", clip.format.codecString)
            assertEquals(1920 to 1080, clip.format.width to clip.format.height)
            assertEquals(12, clip.accessUnits.size)
            assertEquals(listOf(0, 2, 4, 6, 8, 10), clip.accessUnits.indices.filter { clip.accessUnits[it].keyFrame })
            val config = HevcConfig.parse(clip.format.config)
            assertEquals(4, config.nalLengthSize)
            assertContentEquals(vps, config.vps.single()); assertContentEquals(sps, config.sps.single())
            assertContentEquals(pps, config.pps.single())
            clip.accessUnits.forEachIndexed { index, unit ->
                val pictures = Nal.units(unit.data).filter { type(it) in 0..31 }.toList()
                assertEquals(if (index % 2 == 0) 2 else 1, pictures.size)
                assertEquals(if (index % 2 == 0) 16 + index / 2 else 1, type(pictures.first()))
                assertEquals(index % 2 == 0, unit.keyFrame)
                if (index % 2 == 0) {
                    assertEquals(39, Nal.units(unit.data).first { type(it) !in 32..35 }.let(::type))
                    assertEquals(40, type(Nal.units(unit.data).last()))
                }
            }
        }
    }

    @Test public fun builderPreservesProfileAndRejectsMalformedHevc(): Unit {
        val high = sps.copyOf().also { it[3] = 0xe2.toByte() }
        assertEquals("hvc1.C2.6.H120.90", HevcConfig.parse(HevcConfigBuilder.build(vps, high, pps)).codecString)
        assertFailsWith<IllegalArgumentException> { HevcConfigBuilder.build(pps, sps, vps) }
        assertFailsWith<IllegalArgumentException> { HevcConfigBuilder.build(vps, sps.copyOf(4), pps) }
        for (tail in listOf(
            byteArrayOf(0x26, 1), byteArrayOf(0x26, 0, 0x80.toByte()), nal(1), nal(22), nal(23), ByteArray(0),
        )) assertFailsWith<IllegalArgumentException> { AnnexBClip.parse(annexB(listOf(vps, sps, pps, tail).filter { it.isNotEmpty() })) }
        val avcSps = Nal.units(clipBytes(), Nal.Format.ANNEX_B).first { it[0].toInt() and 31 == 7 }
        assertEquals(VideoCodec.H264, AnnexBClip.detectCodec(hevcClip() + annexB(listOf(avcSps))))
        val avc = AnnexBClip.parse(clipBytes())
        assertEquals(VideoCodec.H264, avc.format.codec)
        assertEquals(60, avc.accessUnits.size)
    }

    @Test public fun inputAdvertisesOnlyHevcAndReplaysIrapFlags(@TempDir directory: Path): Unit {
        val path = directory.resolve("capture.h264")
        Files.write(path, hevcClip())
        val input = AnnexBFileInput(path, frameRate = 500.0, loop = false)
        Files.delete(path)
        assertEquals(setOf(VideoCodec.H265), input.capabilities.codecs)
        val config = SessionConfig(mapOf(ScreenId.MAIN to ScreenConfig(VideoCodec.H265, 1920, 1080, 30.0)))
        assertEquals(ReconfigureResult.APPLIED, input.reconfigure(config))
        assertEquals(ReconfigureResult.UNSUPPORTED, input.reconfigure(configuration()))
        assertFailsWith<IllegalArgumentException> { input.start(configuration(), TestSink()) }
        val ended = CountDownLatch(1)
        val formats = CopyOnWriteArrayList<VideoFormat>(); val frames = CopyOnWriteArrayList<VideoFrame>()
        try {
            input.start(config, object : TestSink() {
                override fun session(state: SessionState) { if (state.phase == SessionPhase.ENDED) ended.countDown() }
                override fun videoFormat(screen: ScreenId, format: VideoFormat?) { if (format != null) formats += format }
                override fun videoFrame(frame: VideoFrame) { frames += frame }
            })
            ended.awaitResult(); input.awaitStopped()
            assertEquals(VideoCodec.H265, formats.single().codec)
            assertEquals(4, HevcConfig.parse(formats.single().config).nalLengthSize)
            val units = AnnexBClip.parse(hevcClip()).accessUnits
            assertEquals(units.map { it.keyFrame }, frames.map { it.keyFrame })
            units.zip(frames).forEach { (unit, frame) -> assertContentEquals(unit.data, frame.data) }
        } finally { input.stop(); input.awaitStopped() }
    }

    @Test public fun missingFileStillReportsEndedOnWorker(@TempDir directory: Path): Unit {
        val input = AnnexBFileInput(directory.resolve("missing.h265"))
        val ended = CountDownLatch(1)
        val states = CopyOnWriteArrayList<SessionState>(); val threads = CopyOnWriteArrayList<String>()
        try {
            input.start(configuration(), object : TestSink() {
                override fun session(state: SessionState) {
                    states += state; threads += Thread.currentThread().name
                    if (state.phase == SessionPhase.ENDED) ended.countDown()
                }
            })
            ended.awaitResult(); input.awaitStopped()
            assertEquals(SessionPhase.ENDED, states.single().phase)
            assertNotNull(states.single().reason)
            assertTrue(threads.single().startsWith("oxp-replay-"))
        } finally { input.stop(); input.awaitStopped() }
    }

    private fun hevcClip(aud: Boolean = true): ByteArray {
        val units = mutableListOf(vps, sps, pps)
        for (type in 16..21) {
            if (aud) units += nal(35)
            units += nal(39); units += nal(type); units += nal(type, false); units += nal(40)
            if (aud) units += nal(35)
            units += nal(1)
        }
        return annexB(units)
    }
    private fun nal(type: Int, first: Boolean = true): ByteArray = byteArrayOf(
        (type shl 1).toByte(), 1, (if (first) 0x80 else 0x40).toByte(),
    )
    private fun annexB(units: List<ByteArray>): ByteArray = units.fold(ByteArray(0)) { bytes, nal -> bytes + byteArrayOf(0, 0, 1) + nal }
    private fun type(nal: ByteArray): Int = (nal[0].toInt() ushr 1) and 63
    private fun hex(value: String): ByteArray = value.split(' ').map { it.toInt(16).toByte() }.toByteArray()
    private val vps = hex("40 01 0c 01 ff ff 01 60 00 00 03 00 90 00 00 03 00 00 03 00 78 f0 24")
    private val sps = hex("42 01 01 01 60 00 00 03 00 90 00 00 03 00 00 03 00 78 a0 03 c0 80 11 07 cb 97 e4 93 6b 20")
    private val pps = hex("44 01 c1 71 80 89")
}
