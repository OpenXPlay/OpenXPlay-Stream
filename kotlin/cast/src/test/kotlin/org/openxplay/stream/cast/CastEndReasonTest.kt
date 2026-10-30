// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-30

package org.openxplay.stream.cast

import java.io.EOFException
import java.net.SocketException
import java.net.SocketTimeoutException
import java.util.concurrent.CopyOnWriteArrayList
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.Timeout
import kotlin.test.*

@Timeout(10)
class CastEndReasonTest {
    @Test fun endClassificationDoesNotIncludePeerText() {
        for ((error, fields) in listOf(
            EOFException("private") to "by=peer cause=peer_eof",
            SocketException("Connection reset: private") to "by=peer cause=peer_reset",
            SocketTimeoutException("private") to "by=timeout cause=read_timeout",
            IllegalArgumentException("private") to "by=local cause=protocol_error",
        )) {
            assertContains(castEndReason(error), fields)
            assertFalse(castEndReason(error).contains("private"))
        }
        assertContains(castEndReason(java.net.ConnectException("private"), true), "by=local cause=connect_failed")
        val queue = StreamClient(1)
        assertFalse(queue.offer(byteArrayOf(1, 2))); queue.end("by=local cause=stopped")
        assertEquals("by=local cause=queue_overflow", queue.endReason)
    }
    @Test fun truncatedHttpAndSoapFaultReportStatusAndStageAndIsolateObserverFailures() {
        val lines = CopyOnWriteArrayList<String>()
        OneHttpServer { socket, _, _ ->
            socket.getOutputStream().write("HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nx".toByteArray())
        }.use { server ->
            assertFailsWith<EOFException> {
                CastHttpClient(LOOPBACK_NETWORK, diagnostic = { lines += it; throw IllegalStateException("observer") }).request(server.uri)
            }
            assertContains(lines.single(), "reached=http by=peer cause=peer_eof")
            assertContains(lines.single(), "status=200"); server.assertHealthy()
        }
        lines.clear()
        OneHttpServer { socket, _, _ -> respond(socket, fault(701), 500) }.use { server ->
            assertFailsWith<SoapFault> {
                CastSoap(CastHttpClient(LOOPBACK_NETWORK), diagnostic = { lines += it }).action(CastService("${AVTRANSPORT_TYPE}1", server.uri), "Play")
            }
            assertEquals("request_failed link=renderer reached=soap by=peer cause=soap_fault code=701 status=500", lines.single())
            assertFalse(lines.single().contains("Test rejected")); server.assertHealthy()
        }
    }
    @Test fun localStopReportsTheFurthestRendererStageOnce() {
        FakeRenderer().use { fixture ->
            val lines = CopyOnWriteArrayList<String>(); val states = CopyOnWriteArrayList<CastState>(); val threads = TrackedThreads()
            val output = fixture.output(object : CastListener {
                override fun state(state: CastState, renderer: CastRenderer?) { states += state }
                override fun diagnostic(message: String) { lines += message }
            }, threads)
            try {
                output.open(NO_HOST)
                await("Renderer discovery") { CastState.WAITING_FOR_VIDEO in states }
                output.close(); output.close(); threads.awaitEnded()
                assertEquals(1, lines.count { it == "link_lost link=renderer reached=waiting_for_video by=local cause=stopped" })
            } finally { output.close(); threads.awaitEnded() }
        }
    }
    @Test fun streamingHttpReportsOneEndOnGenerationReplacement() {
        val lines = CopyOnWriteArrayList<String>(); val threads = TrackedThreads()
        val server = CastStreamServer(0, "test", threads, diagnostic = { lines += it })
        try {
            server.reset(1); server.bind()
            threads.newThread { server.accept { true } }.apply { start() }
            val chunks = mutableListOf<CastChunk>()
            CastMediaMuxer(videoFormat(), false) { chunks += it }.write(MediaEvent.Video(frame(), 1_000_000), System.nanoTime())
            val chunk = chunks.single(); server.publish(1, chunk)
            java.net.Socket(LOOPBACK, server.port).use { socket ->
                socket.getOutputStream().write("GET ${server.path(1)} HTTP/1.1\r\nHost: localhost\r\n\r\n".toByteArray())
                val input = HttpInput(socket, 2000); assertEquals("HTTP/1.1 200 OK", input.head().start)
                input.exact(chunk.bytes.size); server.reset(2)
                await("No HTTP end") { lines.isNotEmpty() }
                assertContains(lines.single(), "reached=streaming by=local cause=replaced status=200")
            }
        } finally { server.close(); server.awaitWriters(); threads.awaitEnded() }
        assertEquals(1, lines.size)
    }
}
