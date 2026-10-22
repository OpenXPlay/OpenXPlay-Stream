// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-18

package org.openxplay.stream.clink

import java.util.concurrent.CopyOnWriteArrayList
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.Timeout
import org.openxplay.stream.clink.protocol.*
import org.openxplay.stream.clink.trace.*
import org.openxplay.stream.clink.uibc.UibcDecoder
import org.openxplay.stream.core.*
import kotlin.test.*

@Timeout(15)
class ClinkTraceTest {
    @Test fun sensitiveServicesAndUndecryptableFramesHaveNoFields() {
        val secrets = Fields(mapOf(1 to FieldValue.Bytes("012345 private-key accessory-identity cert-body".toByteArray()))).encode()
        for (service in listOf("auth", "cert")) assertTrue(ClinkTraceFields.decode(service, secrets).isEmpty())
        assertTrue(ClinkTraceFields.decode("control", secrets, false).isEmpty())
        val safe = Fields(mapOf(1 to FieldValue.Varint(42uL), 2 to FieldValue.Bytes(ByteArray(100) { 65 }))).encode()
        val decoded = ClinkTraceFields.decode("control", safe)
        assertEquals(ClinkTraceField.Number(42uL), decoded[1])
        assertEquals(ClinkTraceField.Bytes(100, "A".repeat(64)), decoded[2])
        assertTrue(ClinkTraceFields.decode("control", byteArrayOf(-1)).isEmpty())
        ClinkTrace.NONE.record(ClinkTraceEvent.Error(1, 1, "test", "Test", "test"))
    }
    @Test fun rawInputUsesTheCurrentStreamAndRecordsTheCarView() {
        val traces = mutableListOf<ClinkTraceEvent.Input>()
        var stream = 1450 to 960
        val decoder = UibcDecoder(Origin("clink", "car"), { stream }, ClinkTrace { if (it is ClinkTraceEvent.Input) traces += it }, 42)
        val payload = """{"type":1,"action":0,"count":1,"width":1532,"height":1013,"trackID0":7,"x0":725,"y0":480}""".toByteArray()
        val touch = decoder.decode(payload) as ControlEvent.Touch
        assertEquals(listOf(TouchContact(0, 0.5, 0.5, true)), touch.contacts)
        val first = traces.single()
        assertEquals(42L, first.session)
        assertEquals(listOf(725L to 480L), first.rawCoordinates)
        assertEquals(listOf(0.5 to 0.5), first.coordinates)
        assertEquals(1450 to 960, first.streamSize)
        assertEquals(1532 to 1013, first.viewSize)
        stream = 1280 to 720
        val move = decoder.decode("""{"type":1,"action":2,"count":1,"trackID0":7,"x0":640,"y0":360}""".toByteArray()) as ControlEvent.Touch
        assertEquals(listOf(TouchContact(0, 0.5, 0.5, true)), move.contacts)
        assertEquals(stream, traces.last().streamSize)
        assertEquals(listOf(640L to 360L), traces.last().rawCoordinates)
        assertNull(traces.last().viewSize)
        decoder.decode("""{"type":1,"action":3,"count":0}""".toByteArray())
        assertEquals("touch_cancel", traces.last().kind)
        assertEquals(stream, traces.last().streamSize)
        assertNull(traces.last().viewSize)
        decoder.decode("""{"type":2,"keycode":4,"action":1,"width":1532,"height":1013}""".toByteArray())
        assertEquals("key_up", traces.last().kind)
        assertTrue(traces.last().rawCoordinates.isEmpty())
        assertEquals(stream, traces.last().streamSize)
        assertEquals(1532 to 1013, traces.last().viewSize)
    }
    @Test fun inputTraceUsesTheNormalizationSizeSnapshot() {
        val traces = mutableListOf<ClinkTraceEvent.Input>()
        var reads = 0
        val decoder = UibcDecoder(Origin("clink", "car"), { if (reads++ == 0) 1450 to 960 else 1280 to 720 },
            ClinkTrace { if (it is ClinkTraceEvent.Input) traces += it })
        val touch = decoder.decode("""{"type":1,"action":0,"count":1,"trackID0":7,"x0":725,"y0":480}""".toByteArray()) as ControlEvent.Touch
        assertEquals(listOf(TouchContact(0, 0.5, 0.5, true)), touch.contacts)
        assertEquals(1, reads)
        assertEquals(1450 to 960, traces.single().streamSize)
        assertEquals(listOf(0.5 to 0.5), traces.single().coordinates)
    }
    @Test fun absentOrInvalidViewSizeIsNullWithoutRejectingTouch() {
        val traces = mutableListOf<ClinkTraceEvent.Input>()
        val decoder = UibcDecoder(Origin("clink", "car"), { 1450 to 960 }, ClinkTrace { if (it is ClinkTraceEvent.Input) traces += it })
        for (view in listOf("", ""","width":1532""", ""","height":1013""",
            ""","width":0,"height":1013""", ""","width":1532,"height":-1""",
            ""","width":"1532","height":1013""", ""","width":1532,"height":1.5""",
            ""","width":2147483648,"height":1013""", ""","width":null,"height":1013""",
            ""","width":1532,"height":true""", ""","width":[],"height":1013""", ""","width":1532,"height":{}""")) {
            val touch = decoder.decode("""{"type":1,"action":0,"count":1,"trackID0":7,"x0":725,"y0":480$view}""".toByteArray()) as ControlEvent.Touch
            assertEquals(listOf(TouchContact(0, 0.5, 0.5, true)), touch.contacts)
            assertNull(traces.last().viewSize, view)
        }
    }
    @Test fun loopbackRecordsTimelineWithoutAuthenticationPayloads() {
        val events = CopyOnWriteArrayList<ClinkTraceEvent>()
        val packets = CopyOnWriteArrayList<ByteArray>()
        val ports = freePorts()
        val input = ClipInput(BaselineClip())
        val output = ClinkOutput(StaticClinkLink(ClinkAddresses("127.0.0.1", "127.0.0.1", ports.auth, "secret-accessory")),
            PinProvider { "012345" }, MemoryTrustStore(), ports = ports, vehicleData = true, trace = ClinkTrace { events += it },
            mediaTap = ClinkMediaTap { bytes, offset, length -> packets += bytes.copyOfRange(offset, offset + length) })
        val hub = ProjectionHub(input)
        val peer = SimulatedHeadUnit(ports, sendVehicleSamples = false)
        try {
            hub.start(); hub.addOutput(output)
            peer.await("media trace") { events.filterIsInstance<ClinkTraceEvent.Media>().any { it.rtpPackets > 0 } }
            peer.requestKeyframe()
            peer.screenNotice(5)
            peer.await("screen notice requests a key frame") {
                events.any { it is ClinkTraceEvent.Video && it.event == "keyframe_request" && it.source == "head_unit_m5" }
            }
            peer.touch(0, listOf(PixelContact(1, 640, 360)))
            peer.await("input trace") { events.any { it is ClinkTraceEvent.Input } }
            peer.await("sensor connected") { peer.sensorConnected }
            val undecryptable = Fields(mapOf(1 to FieldValue.Bytes("undecryptable-secret".toByteArray()))).encode()
            peer.sensorBytes(Frame(777, 1, 0, 1, 0, 2, 1, undecryptable).encode())
            peer.await("redacted undecryptable frame") {
                events.filterIsInstance<ClinkTraceEvent.Frame>().any { it.service == "sensor" && it.sequence == 777L }
            }
            assertTrue(events.filterIsInstance<ClinkTraceEvent.Frame>().single { it.sequence == 777L }.fields.isEmpty())
            assertFalse(events.joinToString().contains("undecryptable-secret"))
            val rtsp = events.filterIsInstance<ClinkTraceEvent.Rtsp>()
            assertEquals("TX", rtsp.first().direction)
            assertTrue(rtsp.first().startLine.startsWith("OPTIONS "))
            assertTrue(rtsp.any { it.direction == "RX" })
            val frames = events.filterIsInstance<ClinkTraceEvent.Frame>()
            assertTrue(frames.any { it.service == "control" && it.fields.isNotEmpty() })
            for (service in listOf("auth", "cert")) {
                val redacted = frames.filter { it.service == service }
                assertTrue(redacted.isNotEmpty()); assertTrue(redacted.all { it.fields.isEmpty() })
                assertTrue(redacted.any { it.bodySize > 0 })
            }
            val sockets = events.filterIsInstance<ClinkTraceEvent.Socket>()
            assertTrue(sockets.any { it.event == "LISTEN" })
            assertTrue(sockets.any { it.event == "ACCEPTED" })
            assertTrue(sockets.indexOfFirst { it.event == "DIAL" } < sockets.indexOfFirst { it.event == "CONNECTED" })
            assertTrue(events.any { it is ClinkTraceEvent.Negotiation })
            assertTrue(packets.isNotEmpty()); assertEquals(0x80, packets.first()[0].toInt() and 255)
            assertEquals(0x47, packets.first()[12].toInt() and 255)
            assertTrue(events.map { it.session }.distinct().size == 1)
            assertFalse(events.joinToString().contains("012345"))
            assertFalse(events.joinToString().contains("secret-accessory"))
            peer.assertHealthy()
        } finally { peer.expectRelayClose(); hub.stop(); peer.close() }
    }
}
