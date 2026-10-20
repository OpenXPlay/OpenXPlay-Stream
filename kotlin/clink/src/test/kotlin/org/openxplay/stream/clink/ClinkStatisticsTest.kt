// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-20
package org.openxplay.stream.clink

import java.util.concurrent.CopyOnWriteArrayList
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.Timeout
import org.openxplay.stream.core.*
import kotlin.test.*

@Timeout(20)
class ClinkStatisticsTest {
    @Test fun countsPlaceholderLiveBytesAndCodecMismatchWithTraceOff() {
        val ports = freePorts()
        val reports = CopyOnWriteArrayList<ClinkStatistics>()
        val errors = CopyOnWriteArrayList<Throwable>()
        val good = BundledPlaceholderVideo()
        val bad = BundledHevcPlaceholderVideo()
        SimulatedHeadUnit(ports).use { peer ->
            val output = ClinkOutput(StaticClinkLink(ClinkAddresses("127.0.0.1", "127.0.0.1", ports.auth)),
                PinProvider { "012345" }, MemoryTrustStore(), ports = ports, hevc = ClinkHevcMode.OFF,
                listener = object : ClinkListener {
                    override fun statistics(stats: ClinkStatistics) { reports.add(stats) }
                    override fun error(error: Throwable) { errors.add(error) }
                })
            output.open(object : OutputHost {
                override fun capabilities(capabilities: OutputCapabilities) {}
                override fun control(event: ControlEvent) {}
                override fun stats(stats: OutputStats) {}
            })
            try {
                val session = SessionId(1)
                output.onSession(SessionState(session, SessionPhase.ACTIVE), SessionConfig(emptyMap()))
                peer.await("placeholder statistics without tracing") { reports.any { it.placeholderFrames > 0 } }
                val placeholder = reports.last()
                assertEquals(0L, placeholder.liveFrames); assertEquals(0L, placeholder.liveBytes)
                assertNull(placeholder.codec); assertEquals(0, placeholder.width); assertEquals(0, placeholder.height)
                fun send(clip: PlaceholderVideo, pts: Long) {
                    output.onVideoFormat(ScreenId.MAIN, clip.format)
                    output.onVideoFrame(VideoFrame(ScreenId.MAIN, session, clip.frames.first(), pts, true, System.nanoTime()))
                }
                send(good, 0)
                peer.await("live statistics without tracing") { reports.any { it.liveFrames == 1L } }
                val live = reports.last()
                assertEquals(good.frames.first().size.toLong(), live.liveBytes)
                assertEquals(VideoCodec.H264, live.codec)
                assertEquals(good.format.width, live.width); assertEquals(good.format.height, live.height)
                send(bad, 33_333)
                peer.await("codec mismatch counted without tracing") { reports.any { it.droppedFrames > live.droppedFrames } }
                val mismatch = reports.last()
                assertEquals(live.liveFrames, mismatch.liveFrames)
                assertEquals(live.liveBytes, mismatch.liveBytes)
                assertEquals(live.droppedFrames + 1, mismatch.droppedFrames)
                assertTrue(mismatch.placeholderFrames >= placeholder.placeholderFrames)
                assertTrue(errors.isEmpty(), errors.joinToString())
                peer.assertHealthy()
            } finally { peer.expectRelayClose(); output.close() }
        }
    }
}
