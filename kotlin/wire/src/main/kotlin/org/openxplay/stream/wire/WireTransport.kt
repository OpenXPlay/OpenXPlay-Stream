// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.wire

import java.io.InputStream
import java.io.OutputStream
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.StandardProtocolFamily
import java.net.UnixDomainSocketAddress
import java.nio.ByteBuffer
import java.nio.channels.ServerSocketChannel
import java.nio.channels.SocketChannel
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.attribute.PosixFilePermissions
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit

/** Owned duplex streams. Closing a binding closes both streams and the optional transport owner. */
public class WireStreams(
    public val input: InputStream,
    public val output: OutputStream,
    private val owner: AutoCloseable? = null,
) : AutoCloseable {
    override fun close() {
        owner?.let { runCatching { it.close() } }
        runCatching { input.close() }; runCatching { output.close() }
    }
}

/** Creates one owned connection per start/open. All transport I/O runs on binding threads. */
public class WireTransport private constructor(private val factory: () -> PendingTransport) {
    @Volatile private var listening = CompletableFuture<Int>()
    private var previous: PendingTransport? = null

    /** Listener readiness; Unix listeners return zero, TCP listeners return the bound port. */
    public fun awaitListening(timeoutMillis: Long = 5000): Int = listening.get(timeoutMillis, TimeUnit.MILLISECONDS)

    internal fun prepare(): PendingTransport = synchronized(this) {
        listening = CompletableFuture()
        factory().also {
            it.listening = listening; it.predecessor = previous?.finished; previous = it
        }
    }

    public companion object {
        public fun unixConnect(path: Path): WireTransport = WireTransport {
            PendingTransport(UnixDomainSocketAddress.of(path), false, true, null)
        }
        public fun unixListen(path: Path): WireTransport = WireTransport {
            PendingTransport(UnixDomainSocketAddress.of(path), true, true, path)
        }
        public fun tcpConnect(host: String = "127.0.0.1", port: Int): WireTransport = tcp(host, port, false)
        public fun tcpListen(host: String = "127.0.0.1", port: Int = 0): WireTransport = tcp(host, port, true)
        private fun tcp(host: String, port: Int, listener: Boolean): WireTransport {
            require(port in (if (listener) 0 else 1)..65535)
            // Parse only loopback literals, without DNS or caller-thread I/O.
            val bytes = when (host) {
                "localhost" -> byteArrayOf(127, 0, 0, 1)
                "::1", "0:0:0:0:0:0:0:1" -> ByteArray(16).also { it[15] = 1 }
                else -> {
                    val parts = host.split('.')
                    require(parts.size == 4 && parts[0] == "127")
                    parts.map {
                        val n = it.toIntOrNull()
                        require(n != null && n in 0..255 && it == n.toString())
                        n!!.toByte()
                    }.toByteArray()
                }
            }
            val address = InetAddress.getByAddress(bytes)
            return WireTransport { PendingTransport(InetSocketAddress(address, port), listener, false, null) }
        }
        /** Use a factory for repeatable start/stop; each pair is owned and closed by the binding. */
        public fun streams(factory: () -> WireStreams): WireTransport = WireTransport { PendingTransport(factory) }
        /** A supplied pair is single-use. */
        public fun streams(input: InputStream, output: OutputStream): WireTransport {
            var used = false
            return streams {
                synchronized(input) {
                    check(!used) { "Supplied streams are single-use" }; used = true
                    WireStreams(input, output)
                }
            }
        }
    }
}

internal class PendingTransport private constructor(
    private val address: java.net.SocketAddress?, private val listener: Boolean,
    private val unix: Boolean, private val path: Path?, private val streams: (() -> WireStreams)?,
) : AutoCloseable {
    constructor(address: java.net.SocketAddress, listener: Boolean, unix: Boolean, path: Path?) :
        this(address, listener, unix, path, null)
    constructor(streams: () -> WireStreams) : this(null, false, false, null, streams)
    var listening = CompletableFuture<Int>()
    val finished = CompletableFuture<Unit>()
    var predecessor: CompletableFuture<Unit>? = null
    private val lock = Any()
    private var closed = false
    private val owned = mutableListOf<AutoCloseable>()
    private var boundPath = false

    private fun <T : AutoCloseable> retain(resource: T): T = synchronized(lock) {
        if (closed) { resource.close(); error("Transport closed") }
        owned.add(resource); resource
    }
    fun open(): WireStreams {
        try {
            predecessor?.join()
            synchronized(lock) { check(!closed) { "Transport closed" } }
            streams?.let { return retain(it()) }
            val family = if (unix) StandardProtocolFamily.UNIX
                else if ((address as InetSocketAddress).address is java.net.Inet6Address) StandardProtocolFamily.INET6
                else StandardProtocolFamily.INET
            val channel = if (listener) {
                val server = retain(ServerSocketChannel.open(family))
                server.bind(address)
                synchronized(lock) {
                    boundPath = path != null
                    if (closed) {
                        path?.let { Files.deleteIfExists(it) }; boundPath = false
                        error("Transport closed")
                    }
                    path?.let { Files.setPosixFilePermissions(it, PosixFilePermissions.fromString("rw-------")) }
                }
                listening.complete(if (unix) 0 else (server.localAddress as InetSocketAddress).port)
                retain(server.accept()).also { server.close() }
            } else retain(SocketChannel.open(family)).also { it.connect(address) }
            if (!unix) channel.socket().tcpNoDelay = true
            // Channels.newInputStream/newOutputStream share a blocking lock; direct channel I/O is duplex.
            return retain(WireStreams(object : InputStream() {
                override fun read(): Int {
                    val one = ByteArray(1); return if (read(one, 0, 1) < 0) -1 else one[0].toInt() and 255
                }
                override fun read(b: ByteArray, off: Int, len: Int): Int = channel.read(ByteBuffer.wrap(b, off, len))
            }, object : OutputStream() {
                override fun write(b: Int) { write(byteArrayOf(b.toByte())) }
                override fun write(b: ByteArray, off: Int, len: Int) {
                    val buffer = ByteBuffer.wrap(b, off, len)
                    while (buffer.hasRemaining()) channel.write(buffer)
                }
            }, channel))
        } catch (e: Exception) { listening.completeExceptionally(e); throw e }
    }
    override fun close() {
        val resources = synchronized(lock) {
            if (closed) return
            closed = true; owned.toList()
        }
        resources.asReversed().forEach { runCatching { it.close() } }
        synchronized(lock) {
            if (boundPath) { runCatching { Files.deleteIfExists(path!!) }; boundPath = false }
        }
        listening.completeExceptionally(IllegalStateException("Transport closed"))
        finished.complete(Unit)
    }
}
