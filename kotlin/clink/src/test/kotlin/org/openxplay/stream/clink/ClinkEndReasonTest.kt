// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-30

package org.openxplay.stream.clink

import java.io.EOFException
import java.net.SocketException
import java.net.SocketTimeoutException
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.ThreadFactory
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.Timeout
import org.openxplay.stream.core.*
import kotlin.test.*

@Timeout(10)
class ClinkEndReasonTest {
    @Test fun eofResetTimeoutProtocolCancellationAndConnectFailuresAreClassifiedWithoutPeerText() {
        for ((error, expected) in listOf(
            EOFException("private") to ("peer" to "peer_eof"),
            SocketException("Connection reset: private") to ("peer" to "peer_reset"),
            SocketTimeoutException("private") to ("timeout" to "read_timeout"),
            IllegalStateException("heartbeat_timeout") to ("timeout" to "health_timeout"),
            IllegalStateException("projection_timeout") to ("timeout" to "record_timeout"),
            IllegalStateException("auth_timeout") to ("timeout" to "read_timeout"),
            IllegalStateException("audio_ack_timeout") to ("timeout" to "read_timeout"),
            IllegalStateException("pin_cancelled") to ("local" to "stopped"),
            IllegalStateException("rtsp_teardown") to ("peer" to "teardown"),
            IllegalArgumentException("private") to ("local" to "protocol_error"),
        )) {
            val reason = clinkEndReason(error)
            assertEquals(expected, reason.by to reason.cause)
            assertFalse(reason.fields().contains("private"))
        }
        assertContains(clinkEndReason(IllegalStateException("heartbeat_timeout")).fields(), "timeout=heartbeat limitMs=15000")
        assertEquals("connect_failed", clinkEndReason(java.net.ConnectException(), true).cause)
        assertContains(clinkEndReason(SocketTimeoutException(), true).fields(), "timeout=connect limitMs=12000")
    }
    @Test fun failedBootstrapAndLocalStopReportStageAndDiagnosticExceptionsAreIsolated() {
        for (throwing in listOf(false, true)) {
            val lines = CopyOnWriteArrayList<String>()
            val session = session(object : ClinkLink {
                override fun connect(onLinkLost: (Throwable?) -> Unit): ClinkAddresses = throw java.io.IOException("private")
                override fun disconnect(): Unit = Unit
            }, lines, throwing)
            session.run()
            val end = lines.single { it.startsWith("failed ") }
            assertContains(end, "reached=connecting by=local cause=connect_failed")
            assertContains(end, "exception=IOException"); assertFalse(end.contains("private"))
        }
        val lines = CopyOnWriteArrayList<String>()
        val stopped = session(StaticClinkLink(ClinkAddresses("127.0.0.1", "127.0.0.1")), lines)
        stopped.stop(); stopped.run()
        assertContains(lines.single(), "link_lost link=session reached=created by=local cause=stopped")
    }
    @Test fun networkLossWithoutAnExceptionIsPreservedThroughAuthentication() {
        val lines = CopyOnWriteArrayList<String>()
        val ports = freePorts()
        java.net.ServerSocket(ports.auth).use {
            val session = session(object : ClinkLink {
                override fun connect(onLinkLost: (Throwable?) -> Unit): ClinkAddresses {
                    onLinkLost(null)
                    return ClinkAddresses("127.0.0.1", "127.0.0.1", ports.auth)
                }
                override fun disconnect(): Unit = Unit
            }, lines, ports = ports)
            session.run()
        }
        val end = lines.single { it.startsWith("failed ") }
        assertContains(end, "reached=authenticating by=os cause=network_lost")
        assertTrue(lines.any { it.startsWith("link_lost link=tcp service=auth") })
    }
    @Test fun closingBeforeTheLifecycleWorkerStartsStillReportsAnEnd() {
        val lines = CopyOnWriteArrayList<String>(); var lifecycle: Runnable? = null
        val output = ClinkOutput(StaticClinkLink(ClinkAddresses("127.0.0.1", "127.0.0.1")), PinProvider { null },
            MemoryTrustStore(), executor = java.util.concurrent.Executor { lifecycle = it }, listener = object : ClinkListener {
                override fun diagnostic(message: String) { lines += message }
            })
        output.open(object : OutputHost {
            override fun capabilities(capabilities: OutputCapabilities): Unit = Unit
            override fun control(event: ControlEvent): Unit = Unit
            override fun stats(stats: OutputStats): Unit = Unit
        })
        output.close(); checkNotNull(lifecycle).run()
        assertContains(lines.single { it.startsWith("link_lost ") }, "reached=created by=local cause=stopped")
    }
    private fun session(link: ClinkLink, lines: MutableList<String>, throwing: Boolean = false, ports: ClinkPorts = freePorts()): ClinkSession =
        ClinkSession(link, PinProvider { "012345" }, MemoryTrustStore(), ports,
            ThreadFactory { Thread(it).apply { isDaemon = true } }, "clink", object : ClinkListener {
                override fun diagnostic(message: String) { lines += message; if (throwing) throw IllegalStateException("observer") }
            }, {}, {}, {}, {})
}
