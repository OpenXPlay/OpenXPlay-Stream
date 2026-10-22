// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-22

package org.openxplay.stream.clink

import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.Timeout
import org.openxplay.stream.clink.protocol.AuthIdentity
import org.openxplay.stream.clink.trace.*
import org.openxplay.stream.clink.uibc.UibcDecoder
import org.openxplay.stream.core.*
import kotlin.test.*

@Timeout(15)
class ClinkViewSizeTest {
    private val origin = Origin("clink", "car")
    private fun touch(decoder: UibcDecoder, view: String = ""): ControlEvent.Touch = decoder.decode(
        """{"type":1,"action":0,"count":1,"trackID0":7,"x0":640,"y0":360$view}""".toByteArray()) as ControlEvent.Touch

    @Test fun firstTouchAndEachChangedSizeAreReportedOnce() {
        val reported = mutableListOf<Pair<Int, Int>>()
        val traces = mutableListOf<ClinkTraceEvent.Link>()
        val decoder = UibcDecoder(origin, { 1280 to 720 },
            ClinkTrace { if (it is ClinkTraceEvent.Link) traces += it }, 42,
            onViewSize = { width, height -> reported += width to height })
        val expected = listOf(1532 to 1013, 1814 to 1200, 1532 to 1013, 1533 to 1013, 1533 to 1014)
        for ((index, size) in expected.withIndex()) {
            repeat(3) {
                val event = touch(decoder, ""","width":${size.first},"height":${size.second}""")
                assertEquals(listOf(TouchContact(0, 0.5, 0.5, true)), event.contacts)
                assertEquals(expected.take(index + 1), reported)
            }
        }
        assertEquals(expected.map { mapOf("width" to it.first.toString(), "height" to it.second.toString()) }, traces.map { it.fields })
        assertTrue(traces.all { it.stage == "view_size" && it.session == 42L })
    }

    @Test fun missingAndMalformedSizesLeaveTouchesAndTheLastReportedSizeUnchanged() {
        val reported = mutableListOf<Pair<Int, Int>>()
        val traces = mutableListOf<ClinkTraceEvent.Link>()
        val decoder = UibcDecoder(origin, { 1280 to 720 },
            ClinkTrace { if (it is ClinkTraceEvent.Link) traces += it },
            onViewSize = { width, height -> reported += width to height })
        val invalid = listOf("", ""","width":1532""", ""","height":1013""",
            ""","width":0,"height":1013""", ""","width":1532,"height":0""",
            ""","width":-1,"height":1013""", ""","width":1532,"height":-1""",
            ""","width":"1532","height":1013""", ""","width":1532,"height":"1013"""",
            ""","width":1.5,"height":1013""", ""","width":1532,"height":1.5""",
            ""","width":2147483648,"height":1013""", ""","width":1532,"height":2147483648""",
            ""","width":null,"height":1013""", ""","width":1532,"height":true""",
            ""","width":[],"height":1013""", ""","width":1532,"height":{}""")
        for (view in invalid) {
            assertEquals(listOf(TouchContact(0, 0.5, 0.5, true)), touch(decoder, view).contacts, view)
        }
        assertTrue(reported.isEmpty())
        assertTrue(traces.isEmpty())
        touch(decoder, ""","width":1532,"height":1013""")
        for (view in invalid) touch(decoder, view)
        touch(decoder, ""","width":1532,"height":1013""")
        assertEquals(listOf(1532 to 1013), reported)
        assertEquals(1, traces.size)
    }

    @Test fun keyAndUnknownReportsDoNotAnnounceViewSizes() {
        val reported = mutableListOf<Pair<Int, Int>>()
        val decoder = UibcDecoder(origin, { 1280 to 720 },
            onViewSize = { width, height -> reported += width to height })
        assertEquals(ControlEvent.Key(KeyCode.BACK, true, origin), decoder.decode(
            """{"type":2,"keycode":4,"action":0,"width":1532,"height":1013}""".toByteArray()))
        assertNull(decoder.decode("""{"type":99,"width":1532,"height":1013}""".toByteArray()))
        assertTrue(reported.isEmpty())
        touch(decoder, ""","width":1532,"height":1013""")
        assertEquals(listOf(1532 to 1013), reported)
    }

    @Test fun listenerAndTraceFailuresDoNotAffectTouchDecoding() {
        val reported = mutableListOf<Pair<Int, Int>>()
        val traces = mutableListOf<ClinkTraceEvent.Link>()
        val decoder = UibcDecoder(origin, { 1280 to 720 }, ClinkTrace {
            if (it is ClinkTraceEvent.Link) { traces += it; throw IllegalStateException("trace failed") }
        }, onViewSize = { width, height -> reported += width to height; throw AssertionError("listener failed") })
        for (size in listOf(1532 to 1013, 1532 to 1013, 1814 to 1200)) {
            assertEquals(listOf(TouchContact(0, 0.5, 0.5, true)),
                touch(decoder, ""","width":${size.first},"height":${size.second}""").contacts)
        }
        assertEquals(listOf(1532 to 1013, 1814 to 1200), reported)
        assertEquals(2, traces.size)
    }

    @Test fun outputForwardsViewSizesAndReportsTheFirstTouchAgainAfterReconnect() {
        val ports = freePorts()
        val peerIdentity = AuthIdentity.generate()
        var pinnedPhone: ByteArray? = null
        val reported = CopyOnWriteArrayList<Pair<Int, Int>>()
        val traces = CopyOnWriteArrayList<ClinkTraceEvent.Link>()
        val errors = CopyOnWriteArrayList<Throwable>()
        val touches = LinkedBlockingQueue<ControlEvent.Touch>()
        val ended = LinkedBlockingQueue<Unit>()
        val output = ClinkOutput(StaticClinkLink(ClinkAddresses("127.0.0.1", "127.0.0.1", ports.auth, "car")),
            PinProvider { "012345" }, MemoryTrustStore(), ports = ports,
            trace = ClinkTrace { if (it is ClinkTraceEvent.Link && it.stage == "view_size") traces += it },
            listener = object : ClinkListener {
                override fun viewSize(width: Int, height: Int) { reported += width to height }
                override fun error(error: Throwable) { errors += error }
                override fun state(state: ClinkState) { if (state == ClinkState.ENDED) ended.add(Unit) }
            })
        val host = object : OutputHost {
            override fun capabilities(capabilities: OutputCapabilities) = Unit
            override fun control(event: ControlEvent) { if (event is ControlEvent.Touch) touches.add(event) }
            override fun stats(stats: OutputStats) = Unit
        }
        repeat(2) { generation ->
            SimulatedHeadUnit(ports, identity = peerIdentity, pinnedPhone = pinnedPhone,
                sensorAttempts = 1, sensorRetryMillis = 0).use { peer ->
                output.open(host)
                try {
                    peer.await("projection ready") { peer.playing && peer.configurationAnswered && peer.accessUnits.isNotEmpty() }
                    pinnedPhone = peer.phoneKey
                    val sizes = if (generation == 0) listOf(1532 to 1013, 1532 to 1013, 1814 to 1200, 1814 to 1200, 1532 to 1013)
                        else listOf(1532 to 1013)
                    for ((index, size) in sizes.withIndex()) {
                        peer.touch(if (index == 0) 0 else 2, listOf(PixelContact(7, 0, 0)), size.first, size.second)
                        assertNotNull(touches.poll(3, TimeUnit.SECONDS), "Missing decoded touch")
                    }
                    val expected = listOf(1532 to 1013, 1814 to 1200, 1532 to 1013) +
                        if (generation == 1) listOf(1532 to 1013) else emptyList()
                    assertEquals(expected, reported.toList())
                    assertEquals(expected.map { mapOf("width" to it.first.toString(), "height" to it.second.toString()) }, traces.map { it.fields })
                    assertEquals(generation + 1, traces.map { it.session }.distinct().size)
                    peer.assertHealthy()
                } finally {
                    peer.expectRelayClose(); output.close()
                    assertNotNull(ended.poll(3, TimeUnit.SECONDS), "Output did not finish closing")
                    touches.clear()
                }
            }
        }
        assertTrue(errors.isEmpty(), errors.joinToString())
    }
}
