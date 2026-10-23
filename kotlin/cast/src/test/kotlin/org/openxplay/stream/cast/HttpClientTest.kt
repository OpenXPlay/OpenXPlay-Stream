// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-22

package org.openxplay.stream.cast

import java.io.ByteArrayInputStream
import java.io.InputStream
import java.net.ServerSocket
import java.net.Socket
import java.net.URI
import java.util.concurrent.CopyOnWriteArrayList
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.Timeout
import kotlin.test.*

internal class OneHttpServer(private val handler: (Socket, HttpHead, ByteArray) -> Unit) : AutoCloseable {
    private val server = ServerSocket(0, 8, LOOPBACK)
    private val sockets = CopyOnWriteArrayList<Socket>()
    private val errors = CopyOnWriteArrayList<Throwable>()
    val uri = URI("http://127.0.0.1:${server.localPort}/test")
    private val thread = Thread({
        try {
            server.accept().use { socket ->
                sockets += socket
                val input = HttpInput(socket, 2000)
                val head = input.head()
                handler(socket, head, if (head.headers["content-length"] != null) input.body(head.headers, 65536) else byteArrayOf())
            }
        } catch (error: Throwable) { if (!server.isClosed) errors += error }
    }, "test-http").apply { isDaemon = true; start() }
    fun assertHealthy() { thread.join(2000); assertTrue(errors.isEmpty(), errors.joinToString()); assertFalse(thread.isAlive) }
    override fun close() { server.close(); sockets.forEach { runCatching { it.close() } }; thread.join(2000) }
}

@Timeout(10)
class HttpClientTest {
    @Test fun lfAndDuplicateHeadersKeepFirstWithinExactHeadLimit() {
        for (ending in listOf("\n", "\r\n")) {
            val wire = "GET / HTTP/1.1${ending}Host: first${ending}hOsT: second$ending$ending".toByteArray()
            fun input(): HttpInput = HttpInput(object : Socket() {
                override fun getInputStream(): InputStream = ByteArrayInputStream(wire)
                override fun setSoTimeout(timeout: Int): Unit = Unit
            }, 1000)
            val head = input().head(wire.size)
            assertEquals("GET / HTTP/1.1", head.start); assertEquals("first", head.headers["host"])
            assertFailsWith<java.io.IOException> { input().head(wire.size - 1) }
        }
    }
    @Test fun clientAcceptsLfAndFirstDuplicateResponseHeadersForLengthAndChunks() {
        for (wire in listOf("HTTP/1.0 200 Fine\nContent-Length: 5\ncontent-length: 999999\n\nhello",
            "HTTP/1.1 200 Fine\nTransfer-Encoding: chunked\ntransfer-encoding: identity\n\n2\nhe\n3\nllo\n0\nX-Trailer: ok\n\n")) {
            OneHttpServer { socket, _, _ -> socket.getOutputStream().write(wire.toByteArray()) }.use { server ->
                assertEquals("hello", String(CastHttpClient(LOOPBACK_NETWORK).request(server.uri).body))
                server.assertHealthy()
            }
        }
    }
    @Test fun contentLengthChunkedAndCloseDelimitedBodies() {
        for (wire in listOf("HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2;ext=a\r\nhe\r\n3\r\nllo\r\n0\r\nX-Trailer: ok\r\n\r\n",
            "HTTP/1.0 200 OK\r\n\r\nhello")) {
            OneHttpServer { socket, head, _ ->
                assertEquals("GET /test HTTP/1.1", head.start)
                socket.getOutputStream().apply { write(wire.toByteArray()); flush() }
            }.use { server ->
                val response = CastHttpClient(LOOPBACK_NETWORK).request(server.uri)
                assertEquals(200, response.status); assertEquals("hello", String(response.body)); assertEquals(LOOPBACK, response.localAddress)
                server.assertHealthy()
            }
        }
    }
    @Test fun bodyLimitsApplyToEveryFramingAndTruncationIsRejected() {
        for (wire in listOf("HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\n123456",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n6\r\n123456\r\n0\r\n\r\n",
            "HTTP/1.0 200 OK\r\n\r\n123456", "HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n1")) {
            OneHttpServer { socket, _, _ -> socket.getOutputStream().write(wire.toByteArray()) }.use { server ->
                assertFails { CastHttpClient(LOOPBACK_NETWORK).request(server.uri, limit = 5) }
                server.assertHealthy()
            }
        }
    }
    @Test fun timeoutAndInjectedSocketAreUsed() {
        var supplied = 0
        val network = object : CastNetwork {
            override fun localAddresses(): List<java.net.Inet4Address> = listOf(LOOPBACK)
            override fun socket(): Socket { supplied++; return Socket() }
        }
        OneHttpServer { _, _, _ -> Thread.sleep(150) }.use { server ->
            assertFailsWith<java.net.SocketTimeoutException> { CastHttpClient(network).request(server.uri, readMs = 30, budgetMs = 50) }
            assertEquals(1, supplied); server.assertHealthy()
        }
    }
    @Test fun scopeCloseCancelsAnOutstandingRead() {
        val scope = SocketScope()
        OneHttpServer { _, _, _ -> Thread.sleep(200) }.use { server ->
            val errors = CopyOnWriteArrayList<Throwable>()
            val reader = Thread { try { CastHttpClient(LOOPBACK_NETWORK, scope).request(server.uri) } catch (error: Exception) { errors += error } }
            reader.start(); Thread.sleep(50); scope.close(); reader.join(1000)
            assertFalse(reader.isAlive); assertEquals(1, errors.size)
        }
    }
}
