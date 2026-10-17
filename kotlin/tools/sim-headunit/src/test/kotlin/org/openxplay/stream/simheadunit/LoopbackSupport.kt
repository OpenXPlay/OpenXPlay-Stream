// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.simheadunit

import org.openxplay.stream.clink.ClinkPorts

import java.io.ByteArrayOutputStream
import java.io.DataOutputStream
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.nio.file.Path
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import org.openxplay.stream.core.*
import org.openxplay.stream.core.codec.AvcConfig
import org.openxplay.stream.core.codec.Nal

internal fun freePorts(): ClinkPorts {
    // Reserve the whole set simultaneously so no two services receive the same ephemeral port.
    val reservations = mutableListOf<ServerSocket>()
    try {
        repeat(8) { reservations += ServerSocket().apply { bind(InetSocketAddress("127.0.0.1", 0)) } }
        val p = reservations.map { it.localPort }
        return ClinkPorts(p[0], p[1], p[2], p[3], p[4], p[5], p[6], p[7])
    } finally { reservations.forEach { it.close() } }
}

internal class BaselineClip {
    val frames: List<ByteArray>
    val format: VideoFormat
    init {
        // Resolve the clip independently of the module working directory.
        val path = Path.of(checkNotNull(System.getProperty("sim.repository")), "core/src/test/resources/media/baseline-640x360.h264")
        val units = Nal.units(java.nio.file.Files.readAllBytes(path), Nal.Format.ANNEX_B).toList()
        val sps = units.first { it[0].toInt() and 31 == 7 }
        val pps = units.first { it[0].toInt() and 31 == 8 }
        val config = ByteArrayOutputStream().also { bytes ->
            DataOutputStream(bytes).use { out ->
                out.write(byteArrayOf(1, sps[1], sps[2], sps[3], 0xff.toByte(), 0xe1.toByte()))
                out.writeShort(sps.size); out.write(sps); out.writeByte(1); out.writeShort(pps.size); out.write(pps)
            }
        }.toByteArray()
        check(AvcConfig.parse(config).let { it.width == 640 && it.height == 360 })
        format = VideoFormat.create(ScreenId.MAIN, VideoCodec.H264, config, 30.0)
        val groups = mutableListOf<ByteArray>()
        var current = ByteArrayOutputStream()
        fun flush() {
            if (current.size() > 0) { groups += Nal.annexBToAvcc(current.toByteArray()); current = ByteArrayOutputStream() }
        }
        for (unit in units) {
            if (unit[0].toInt() and 31 == 9) flush()
            if (unit[0].toInt() and 31 in setOf(1, 5, 6, 9)) {
                current.write(byteArrayOf(0, 0, 0, 1)); current.write(unit)
            }
        }
        flush(); frames = groups
        check(frames.size == 60 && Nal.isKeyFrame(frames.first(), VideoCodec.H264))
    }
}

/** Test-driven producer; acknowledgment of received AUs paces playback without wall-clock sleeps. */
internal class ClipInput(private val clip: BaselineClip) : ProjectionInput {
    private val fake = FakeInput(InputCapabilities(setOf(VideoCodec.H264), setOf(ScreenId.MAIN), true,
        KeyCode.entries.toSet(), false, true, ReconfigureCost.NONE))
    override val capabilities: InputCapabilities get() = fake.capabilities
    val events = LinkedBlockingQueue<ControlEvent>()
    private val session = SessionId(1)
    override fun start(config: SessionConfig, sink: InputSink) {
        fake.start(config, sink)
        fake.emit { it.session(SessionState(session, SessionPhase.ACTIVE)); it.videoFormat(ScreenId.MAIN, clip.format) }
    }
    override fun reconfigure(config: SessionConfig): ReconfigureResult = ReconfigureResult.UNSUPPORTED
    override fun control(event: ControlEvent): Boolean { events.add(event); return fake.control(event) }
    override fun stop() { fake.stop() }
    fun send(index: Int, ptsIndex: Int = index) {
        val data = clip.frames[index]
        fake.emit { it.videoFrame(VideoFrame(ScreenId.MAIN, session, data, ptsIndex * 33_333L,
            Nal.isKeyFrame(data, VideoCodec.H264), System.nanoTime())) }
    }
    inline fun <reified T : ControlEvent> next(): T {
        val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(3)
        while (true) {
            val remaining = deadline - System.nanoTime()
            check(remaining > 0) { "Timed out waiting for ${T::class.simpleName}" }
            val event = events.poll(remaining, TimeUnit.NANOSECONDS)
                ?: error("Timed out waiting for ${T::class.simpleName}")
            if (event is T) return event
        }
    }
}
