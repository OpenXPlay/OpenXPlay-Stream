// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-11-02

package org.openxplay.stream.carlife

import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.Closeable
import java.io.OutputStream
import java.net.Socket
import java.net.SocketTimeoutException
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import org.openxplay.stream.core.*
import org.openxplay.stream.core.codec.Nal
import kotlin.test.*
import org.junit.jupiter.api.Test

class CarlifeOutputTest {
    private fun await(predicate: () -> Boolean) {
        val end = System.nanoTime() + TimeUnit.SECONDS.toNanos(10)
        while (!predicate()) {
            check(System.nanoTime() < end) { "CarLife asynchronous operation did not finish" }
            Thread.sleep(5)
        }
    }

    private class Peer(val output: CarlifeOutput) : Closeable {
        val sockets = (1..7).associateWith { Socket("127.0.0.1", output.boundPorts[it - 1]).apply { soTimeout = 5000 } }
        fun send(id: Int, payload: ByteArray = byteArrayOf(), channel: Int = 1) {
            CarlifeFraming.write(sockets.getValue(channel).getOutputStream(), CarlifeMessage(channel, id, payload))
        }
        fun next(id: Int, channel: Int = 1): CarlifeMessage {
            repeat(20) {
                val message = checkNotNull(CarlifeFraming.read(sockets.getValue(channel).getInputStream(), channel))
                if (message.id == id) return message
            }
            error("Expected CarLife response $id")
        }
        fun handshake() {
            send(98305, ProtoWriter().number(1, 4).number(2, 1).build())
            assertEquals(1L, Proto.parse(next(65538).payload).number(1))
            send(98343)
            next(65540)
            send(98307)
            send(98376, ProtoWriter().text(1, "2.0;2026-11-02-10-20-30").build())
            assertEquals("abbc46749f4bce7b06c1b88be4f613ac", Proto.parse(next(65609).payload).text(1))
            send(98378, ProtoWriter().number(1, 1).build())
            next(65611)
            send(98380)
            send(98311, ProtoWriter().number(1, 1280).number(2, 720).number(3, 30).build())
            next(65544)
            send(98313)
        }
        override fun close() { sockets.values.forEach { runCatching { it.close() } } }
    }

    @Test fun loopbackHeadUnitAuthenticatesReceivesAnnexBAndControlsTheSource() {
        val host = RecordingHost()
        val output = CarlifeOutput(CarlifeConfig(bindAddress = "127.0.0.1", discovery = false, ports = List(7) { 0 }))
        try {
            output.open(host)
            val session = SessionId(7)
            output.onSession(SessionState(session, SessionPhase.ACTIVE), SessionConfig(emptyMap()))
            output.onVideoFormat(ScreenId.MAIN, testFormat())
            await { output.boundPorts.size == 7 }
            Peer(output).use { peer ->
                peer.handshake()
                await { host.events.any { it is ControlEvent.KeyFrameRequest } }
                output.onVideoFrame(VideoFrame(ScreenId.MAIN, session, byteArrayOf(0, 0, 0, 2, 0x65, 0x01), 0, true, 0))
                val video = peer.next(131073, 2)
                assertEquals(listOf(7, 8, 5), Nal.units(video.payload, Nal.Format.ANNEX_B).map { it[0].toInt() and 31 }.toList())
                await { output.status.frames == 1L }
                assertEquals(640, output.status.width)
                peer.send(425985, ProtoWriter().number(1, 0).number(2, 320).number(3, 240).build(), 6)
                await { host.events.filterIsInstance<ControlEvent.Touch>().any { it.contacts.isNotEmpty() } }
                assertEquals(0.5, host.events.filterIsInstance<ControlEvent.Touch>().last().contacts.single().x)
            }
            await { output.status.state == CarlifeState.LISTENING }
            assertTrue(host.events.filterIsInstance<ControlEvent.Touch>().last().contacts.none { it.pressed })
            assertTrue(host.caps.last().displays.isEmpty())
            // A new socket set must repeat authentication and cannot inherit the previous video start.
            Peer(output).use { peer ->
                peer.handshake()
                await { output.status.state == CarlifeState.AUTHENTICATED }
                assertEquals(0L, output.status.frames)
            }
        } finally { output.close() }
    }

    @Test fun wireIsSilentBeforeVersionAndCloseCancelsAllReaders() {
        val host = RecordingHost()
        val stopped = CountDownLatch(1)
        val output = CarlifeOutput(CarlifeConfig(bindAddress = "127.0.0.1", discovery = false, ports = List(7) { 0 }),
            listener = CarlifeListener { if (it.state == CarlifeState.CLOSED) stopped.countDown() })
        output.open(host)
        await { output.boundPorts.size == 7 }
        Socket("127.0.0.1", output.boundPorts.first()).use { peer ->
            peer.soTimeout = 150
            assertFailsWith<SocketTimeoutException> { peer.getInputStream().read() }
            assertTrue(host.caps.isEmpty())
            output.close()
            assertTrue(stopped.await(10, TimeUnit.SECONDS))
            peer.soTimeout = 5000
            assertEquals(-1, peer.getInputStream().read())
            assertTrue(host.events.isEmpty(), "A closed output cannot send late touch-release callbacks")
        }
        output.close()
    }

    @Test fun independentWatchdogCanCloseAStalledWriter() {
        val entered = CountDownLatch(1)
        val released = CountDownLatch(1)
        val completed = CountDownLatch(1)
        val stream = object : OutputStream() {
            override fun write(value: Int) {
                entered.countDown()
                released.await(10, TimeUnit.SECONDS)
                throw java.io.IOException("closed")
            }
        }
        val connection = CarlifeConnection(ByteArrayInputStream(byteArrayOf()), stream,
            Closeable { released.countDown() }, false, 2)
        val sender = Thread {
            try { connection.write(CarlifeMessage(2, 131073, byteArrayOf(1))) }
            catch (_: java.io.IOException) { }
            finally { completed.countDown() }
        }.apply { isDaemon = true; start() }
        assertTrue(entered.await(10, TimeUnit.SECONDS))
        assertTrue(connection.stalled(System.nanoTime() + TimeUnit.SECONDS.toNanos(5)))
        connection.close()
        assertTrue(completed.await(10, TimeUnit.SECONDS))
        sender.join(1000)
        assertFalse(connection.active.get())
    }

    @Test fun endedAndReplacedSourceSessionsCannotDeliverOldFrames() {
        val host = RecordingHost()
        val output = CarlifeOutput(CarlifeConfig(bindAddress = "127.0.0.1", discovery = false, ports = List(7) { 0 }))
        try {
            output.open(host)
            val old = SessionId(10)
            val fresh = SessionId(11)
            val config = SessionConfig(emptyMap())
            output.onSession(SessionState(old, SessionPhase.ACTIVE), config)
            output.onVideoFormat(ScreenId.MAIN, testFormat())
            await { output.boundPorts.size == 7 }
            Peer(output).use { peer ->
                peer.handshake()
                await { host.events.any { it is ControlEvent.KeyFrameRequest } }
                fun frame(session: SessionId, marker: Byte) = VideoFrame(ScreenId.MAIN, session,
                    byteArrayOf(0, 0, 0, 2, 0x65, marker), 0, true, 0)
                output.onSession(SessionState(old, SessionPhase.ENDED), config)
                output.onVideoFrame(frame(old, 0x11))
                output.onSession(SessionState(fresh, SessionPhase.ACTIVE), config)
                output.onVideoFormat(ScreenId.MAIN, testFormat())
                output.onVideoFrame(frame(old, 0x12))
                output.onVideoFrame(frame(fresh, 0x22))
                val packet = peer.next(131073, 2)
                val idr = Nal.units(packet.payload, Nal.Format.ANNEX_B).last()
                assertContentEquals(byteArrayOf(0x65, 0x22), idr)
            }
        } finally { output.close() }
    }

    @Test fun protocolFailurePreservesRootChannelMessageAndSafeCauseAcrossTeardown() {
        val logs = java.util.concurrent.CopyOnWriteArrayList<String>()
        val output = CarlifeOutput(CarlifeConfig(bindAddress = "127.0.0.1", discovery = false, ports = List(7) { 0 }),
            listener = object : CarlifeListener {
                override fun onStatus(status: CarlifeStatus): Unit = Unit
                override fun diagnostic(event: String) { logs += event }
            })
        try {
            output.open(RecordingHost())
            await { output.boundPorts.size == 7 }
            Peer(output).use { peer ->
                peer.send(98305, ProtoWriter().number(1, 4).build())
                peer.next(65538)
                peer.send(98343)
                peer.next(65540)
                peer.send(98376, ProtoWriter().text(1, "2.0;2026-11-02-10-20-30").build())
                peer.next(65609)
                peer.send(98378, ProtoWriter().number(1, 0).build())
                await { logs.any { it.contains("session_closed cause=auth_rejected originChannel=1") } }
                val error = logs.single { it.contains(" error ") }
                assertTrue(error.contains("transport=tcp channel=1 id=98378 cause=auth_rejected"))
                assertTrue(error.contains("session=") && error.contains("phase=auth_answered") && error.contains("affected="))
                assertFalse(logs.any { it.contains("2026-11-02-10-20-30") || it.contains("abbc46749") })
            }
        } finally { output.close() }
    }
}
