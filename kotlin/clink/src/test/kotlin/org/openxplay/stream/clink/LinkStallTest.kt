// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

package org.openxplay.stream.clink

import java.io.ByteArrayOutputStream
import java.net.Socket
import java.util.concurrent.ThreadFactory
import kotlin.test.*
import org.junit.jupiter.api.Test
import org.openxplay.stream.core.*

class LinkStallTest {
    private class Fixture {
        val diagnostics = mutableListOf<String>()
        val queue = VideoQueue()
        var keyframes = 0
        val written = ByteArrayOutputStream()
        val socket = object : Socket() {
            override fun getOutputStream() = written
        }
        val session = ClinkSession(
            StaticClinkLink(ClinkAddresses("127.0.0.1", "127.0.0.1")), PinProvider { "012345" },
            MemoryTrustStore(), ClinkPorts(), ThreadFactory { Thread(it) }, "clink",
            object : ClinkListener { override fun diagnostic(message: String) { diagnostics += message } },
            {}, {}, {}, { keyframes++ }, onLinkStalled = { queue.setStalled(it) },
        ).also {
            // Install an in-memory video transport without starting sockets or worker threads.
            fun set(name: String, value: Any) = ClinkSession::class.java.getDeclaredField(name).run {
                isAccessible = true; this.set(it, value)
            }
            set("ready", true); set("playing", true); set("video", socket)
            set("connectedVideoPort", ClinkPorts().rtp)
        }
        fun frame(key: Boolean) = VideoFrame(ScreenId.MAIN, SessionId(1), byteArrayOf(1), 0, key, 0)
        fun advance(health: LinkHealth, seconds: Long, received: Boolean = false) {
            session.updateLink(health, seconds * 1_000_000_000, received)
        }
    }
    @Test fun `ten second stall drops queued and new video then recovers with a keyframe request`() {
        val f = Fixture()
        val health = LinkHealth(0)
        f.queue.offer(f.frame(true))
        assertTrue(f.session.sendRtp(byteArrayOf(1)))
        for (second in 1L..10L) {
            f.advance(health, second)
            if (second >= 5) {
                assertTrue(f.session.isStalled)
                assertFalse(f.session.sendRtp(byteArrayOf(2)))
                f.queue.offer(f.frame(true))
                assertNull(f.queue.poll())
            }
        }
        assertTrue(f.session.isPlaying)
        assertEquals(1, f.written.size())
        f.advance(health, 10, received = true)
        f.advance(health, 11, received = true)
        assertFalse(f.session.isStalled)
        assertEquals(1, f.keyframes)
        assertEquals(listOf("link stalled ms=5000", "link recovered after ms=10000"), f.diagnostics)
        f.queue.offer(f.frame(false)); assertNull(f.queue.poll())
        f.queue.offer(f.frame(true)); assertNotNull(f.queue.poll())
        assertTrue(f.session.sendRtp(byteArrayOf(3)))
        f.advance(health, 16)
        assertEquals("link stalled ms=5000", f.diagnostics.last())
    }
    @Test fun `sixteen seconds without traffic times out including late arriving bytes`() {
        for (received in listOf(false, true)) {
            val f = Fixture()
            val health = LinkHealth(0)
            f.advance(health, 5)
            f.advance(health, 15)
            val error = assertFailsWith<IllegalStateException> { f.advance(health, 16, received) }
            assertEquals("heartbeat_timeout", error.message)
        }
    }
}
