// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-11-02

package org.openxplay.stream.carlife

import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.Closeable
import org.openxplay.stream.core.*
import kotlin.test.*
import org.junit.jupiter.api.Test

internal class RecordingHost : OutputHost {
    val caps = java.util.concurrent.CopyOnWriteArrayList<OutputCapabilities>()
    val events = java.util.concurrent.CopyOnWriteArrayList<ControlEvent>()
    override fun capabilities(capabilities: OutputCapabilities) { caps += capabilities }
    override fun control(event: ControlEvent) { events += event }
    override fun stats(stats: OutputStats): Unit = Unit
}

internal fun testFormat(): VideoFormat {
    val hex = "01 42 c0 1e ff e1 00 09 67 42 c0 1e f4 05 01 ec 80 01 00 04 68 ce 3c 80"
    return VideoFormat.create(ScreenId.MAIN, VideoCodec.H264,
        hex.split(' ').map { it.toInt(16).toByte() }.toByteArray(), 30.0)
}

class CarlifeSessionTest {
    private class Fixture(val source: VideoFormat? = null) {
        val host = RecordingHost()
        val states = mutableListOf<CarlifeStatus>()
        val diagnostics = mutableListOf<String>()
        val outputs = (1..7).associateWith { ByteArrayOutputStream() }
        val session = CarlifeSession("test", CarlifeConfig(), CarlifeDeviceInfo(), host,
            Origin("carlife", "test"), { states += it }, { diagnostics += it }, { source })
        init { for (channel in 1..7) session.attach(CarlifeConnection(ByteArrayInputStream(byteArrayOf()),
            outputs.getValue(channel), Closeable {}, false, channel)) }
        fun receive(id: Int, bytes: ByteArray = byteArrayOf(), channel: Int = 1) = session.receive(CarlifeMessage(channel, id, bytes))
        fun messages(channel: Int = 1): List<CarlifeMessage> {
            val input = ByteArrayInputStream(outputs.getValue(channel).toByteArray())
            return generateSequence { CarlifeFraming.read(input, channel) }.toList()
        }
        fun authenticate() {
            receive(98305, ProtoWriter().number(1, 4).number(2, 1).build())
            receive(98343)
            receive(98376, ProtoWriter().text(1, "2.0;2026-11-02-10-20-30").build())
            receive(98378, ProtoWriter().number(1, 1).build())
            receive(98380)
        }
        fun negotiate(width: Int = 1280, height: Int = 720) {
            receive(98311, ProtoWriter().number(1, width.toLong()).number(2, height.toLong()).number(3, 60).build())
        }
    }

    @Test fun passiveUntilVersionAndDeviceFieldsMatchTheAuthDigest() {
        val fixture = Fixture()
        fixture.session.tick()
        fixture.receive(123)
        assertTrue(fixture.messages().isEmpty())
        fixture.authenticate()
        val messages = fixture.messages()
        assertEquals(listOf(65538, 65540, 65609, 65611, 65617), messages.map { it.id })
        assertEquals(1L, Proto.parse(messages[0].payload).number(1))
        val device = Proto.parse(messages[1].payload)
        assertEquals("OpenXPlay", device.text(4))
        assertEquals("Relay", device.text(14))
        assertEquals("1", device.text(20))
        assertEquals("abbc46749f4bce7b06c1b88be4f613ac", Proto.parse(messages[2].payload).text(1))
        assertTrue(fixture.session.authenticated)
        assertFalse(fixture.diagnostics.any { it.contains("abbc") || it.contains("2026-11") })
        assertTrue(fixture.diagnostics.all { it.contains("session=test phase=") && it.contains("elapsedMs=") })
        assertTrue(fixture.diagnostics.any { it.contains("phase=established") })
        assertFalse(fixture.diagnostics.any { it.contains("OpenXPlay") || it.contains("Relay") })
    }

    @Test fun failedOrOutOfOrderAuthenticationNeverEstablishes() {
        val fixture = Fixture()
        fixture.receive(98305, ProtoWriter().number(1, 4).build())
        assertEquals("unexpected_phase", assertFailsWith<CarlifeProtocolException> { fixture.receive(98380) }.code)
        fixture.receive(98343)
        fixture.receive(98376, ProtoWriter().text(1, "2.0;2026-11-02-10-20-30").build())
        assertEquals("auth_rejected", assertFailsWith<CarlifeProtocolException> {
            fixture.receive(98378, ProtoWriter().number(1, 0).build())
        }.code)
        assertFalse(fixture.session.authenticated)
    }

    @Test fun displayNegotiationIsH264OnlyWithinPanelAnd1080pBounds() {
        val fixture = Fixture()
        fixture.authenticate()
        fixture.negotiate(2400, 1080)
        val info = Proto.parse(fixture.messages().last().payload)
        assertEquals(1920L, info.number(1))
        assertEquals(864L, info.number(2))
        assertEquals(30L, info.number(3))
        val cap = fixture.host.caps.single()
        assertEquals(Viewport(1920, 864), cap.displays.getValue(ScreenId.MAIN).frameSize)
        assertTrue(cap.decoders.all { it.codecString.startsWith("avc1.") })
        assertFalse(fixture.session.video(byteArrayOf(1)))
        fixture.receive(98313)
        assertTrue(fixture.session.video(byteArrayOf(0, 0, 0, 1, 0x65)))
        assertEquals(131073, fixture.messages(2).single().id)
        fixture.receive(98314)
        assertFalse(fixture.session.video(byteArrayOf(1)))
    }

    @Test fun fixedSourceSpsSizeIsUsedInTheFirstEncoderReply() {
        val fixture = Fixture(testFormat())
        fixture.authenticate()
        fixture.negotiate()
        val replies = fixture.messages().filter { it.id == 65544 }
        assertEquals(1, replies.size)
        val info = Proto.parse(replies.single().payload)
        assertEquals(640L, info.number(1))
        assertEquals(480L, info.number(2))
    }

    @Test fun changedActualFormatRequiresReceiverRestartBeforeMoreFrames() {
        val fixture = Fixture()
        fixture.authenticate()
        fixture.negotiate()
        fixture.receive(98313)
        fixture.session.format(testFormat())
        assertFalse(fixture.session.streaming)
        assertEquals(2, fixture.messages().count { it.id == 65544 })
        fixture.receive(98313)
        assertTrue(fixture.session.streaming)
    }

    @Test fun protobufAndRawTouchUseVideoCoordinatesAndReleaseOnClose() {
        val fixture = Fixture()
        fixture.authenticate()
        fixture.negotiate()
        fixture.receive(425985, ProtoWriter().number(1, 0).number(2, 640).number(3, 360).build(), 6)
        var touch = fixture.host.events.filterIsInstance<ControlEvent.Touch>().last()
        assertEquals(0.5, touch.contacts.single().x)
        assertEquals(0.5, touch.contacts.single().y)
        val raw = java.nio.ByteBuffer.allocate(14).putInt(2).put(0).putShort(0).putShort(0)
            .put(1).putShort(1280).putShort(720).array()
        fixture.receive(425998, raw, 6)
        touch = fixture.host.events.filterIsInstance<ControlEvent.Touch>().last()
        assertEquals(listOf(0.0, 1.0), touch.contacts.map { it.x })
        fixture.receive(425985, ProtoWriter().number(1, 99).number(2, 1).number(3, 2).build(), 6)
        assertTrue(fixture.session.active.get())
        assertTrue(fixture.diagnostics.last().contains("input_ignored"))
        fixture.session.close()
        assertTrue(fixture.host.events.filterIsInstance<ControlEvent.Touch>().last().contacts.none { it.pressed })
        assertTrue(fixture.host.caps.last().displays.isEmpty())
        assertTrue(fixture.diagnostics.any { it.contains("session_closed cause=local_close") })
        assertEquals(7, fixture.diagnostics.count { it.contains("transport_closed") })
        assertTrue(CarlifeInput.touch(ProtoWriter().number(1, 1).build(), false, 640, 480,
            Origin("test", "test")).contacts.isEmpty())
    }

    @Test fun unknownMessageAndUnknownProtoGroupDoNotBreakNegotiation() {
        val fixture = Fixture()
        val version = ProtoWriter().number(1, 4).build() + byteArrayOf(27, 8, 99, 28)
        fixture.receive(98305, version)
        fixture.receive(987654, byteArrayOf(-1, -1))
        assertEquals(65538, fixture.messages().single().id)
    }

    @Test fun contentEncryptionIsOnlyRequestedWhenHeadUnitAdvertisesIt() {
        val fixture = Fixture()
        fixture.authenticate()
        fun features(value: Long) = ProtoWriter().number(1, 1).data(2,
            ProtoWriter().text(1, "CONTENT_ENCRYPTION").number(2, value).build()).build()
        fixture.receive(98386, features(0))
        assertFalse(fixture.messages().any { it.id == 65642 })
        fixture.receive(98386, features(1))
        fixture.receive(98386, features(1))
        assertEquals(1, fixture.messages().count { it.id == 65642 })
    }
}
