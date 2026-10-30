// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-22

package org.openxplay.stream.cast

import java.io.IOException
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.net.SocketTimeoutException
import java.security.SecureRandom
import java.util.ArrayDeque
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.ThreadFactory
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong

internal data class ServerOptions(val clientBytes: Int = 8 * 1024 * 1024, val keyframeWaitMs: Long = 10_000, val headMs: Int = 5000)
internal class StreamClient(private val maximumBytes: Int) {
    private val lock = Object()
    private val queue = ArrayDeque<ByteArray>()
    private var bytes = 0
    @Volatile var closed = false; private set
    @Volatile var awaitingKeyframe = true
    fun offer(data: ByteArray): Boolean = synchronized(lock) {
        if (closed) return false
        if (data.size > maximumBytes - bytes) { end("by=local cause=queue_overflow"); return false }
        queue.addLast(data); bytes += data.size; lock.notifyAll(); true
    }
    fun take(): ByteArray? = synchronized(lock) {
        while (!closed && queue.isEmpty()) lock.wait(100)
        if (closed) null else queue.removeFirst().also { bytes -= it.size }
    }
    @Volatile var endReason: String? = null; private set
    fun end(reason: String = "by=local cause=teardown") { synchronized(lock) { if (!closed) endReason = reason; closed = true; queue.clear(); bytes = 0; lock.notifyAll() } }
}

/** Writers own socket I/O. Publishing only appends references under a bounded queue lock. */
internal class CastStreamServer(
    private val requestedPort: Int,
    private var token: String,
    private val threads: ThreadFactory,
    private val options: ServerOptions = ServerOptions(),
    private val now: () -> Long = System::nanoTime,
    private val diagnostic: (String) -> Unit = {},
) : AutoCloseable {
    private val lock = Object()
    private val sockets = ConcurrentHashMap<Socket, StreamClient>()
    private val workers = CopyOnWriteArrayList<Thread>()
    private val cache = ArrayDeque<CastChunk>()
    private var cacheBytes = 0
    private var generation = 0L
    private var readyGeneration = -1L
    private var ended = false
    private var server: ServerSocket? = null
    val needsKeyframe = AtomicBoolean(false)
    val bytesSent = AtomicLong()
    val overflows = AtomicLong()
    @Volatile var port = 0; private set
    val clients: Int get() = synchronized(lock) { sockets.values.count { !it.closed && it !== pending && !it.awaitingKeyframe } }
    val reservedClients: Int get() = synchronized(lock) { sockets.values.count { !it.closed && it !== pending } }
    private val pending = StreamClient(0)

    fun path(generation: Long): String = "/cast/$token/$generation.ts"
    fun ready(generation: Long): Boolean = synchronized(lock) { !ended && readyGeneration == generation }
    fun reset(next: Long) {
        synchronized(lock) {
            generation = next; readyGeneration = -1; cache.clear(); cacheBytes = 0
            sockets.values.filter { it !== pending }.forEach { it.end("by=local cause=replaced") }
            needsKeyframe.set(false); lock.notifyAll()
        }
    }
    fun publish(generation: Long, chunk: CastChunk) {
        synchronized(lock) {
            if (ended || this.generation != generation) return
            if (chunk.keyframeStart) { cache.clear(); cacheBytes = 0; readyGeneration = generation }
            if (chunk.keyframeStart || cache.isNotEmpty()) {
                cache.addLast(chunk); cacheBytes += chunk.bytes.size
                expireCache(chunk.atNanos)
            }
            sockets.values.filter { it !== pending && !it.closed && !it.awaitingKeyframe }.forEach {
                if (!it.offer(chunk.bytes)) overflows.incrementAndGet()
            }
            lock.notifyAll()
        }
    }
    private fun expireCache(time: Long) {
        if (cacheBytes > 4 * 1024 * 1024 || cache.isNotEmpty() && time - cache.first.atNanos > 10_000_000_000L) {
            cache.clear(); cacheBytes = 0
        }
    }
    fun bind() {
        if (token.isEmpty()) token = ByteArray(16).also { SecureRandom().nextBytes(it) }.joinToString("") { "%02x".format(it.toInt() and 255) }
        val socket = ServerSocket()
        try {
            if (requestedPort != 0) socket.reuseAddress = true
            socket.bind(InetSocketAddress("0.0.0.0", requestedPort)); socket.soTimeout = 100
            synchronized(lock) {
                if (ended) throw IOException("Stream server closed")
                server = socket; port = socket.localPort; lock.notifyAll()
            }
        } catch (failure: Exception) { socket.close(); throw failure }
    }
    fun accept(active: () -> Boolean) {
        val listener = synchronized(lock) { checkNotNull(server) }
        try {
            while (active() && !synchronized(lock) { ended }) {
                sockets.forEach { (socket, client) -> if (client !== pending && client.closed) runCatching { socket.close() } }
                try {
                    val socket = listener.accept()
                    if (sockets.size >= 32) { socket.close(); runCatching { diagnostic("link_lost link=renderer service=http reached=accepted by=local cause=queue_overflow") }; continue }
                    val retained = synchronized(lock) {
                        if (ended) false else { sockets[socket] = pending; true }
                    }
                    if (!retained) {
                        socket.close()
                        runCatching { diagnostic("link_lost link=renderer service=http reached=accepted by=local cause=stopped") }
                        return
                    }
                    val worker = threads.newThread { serve(socket) }.apply { name = "cast-http-client" }
                    workers += worker; worker.start()
                } catch (_: SocketTimeoutException) { } catch (error: java.net.SocketException) {
                    if (synchronized(lock) { ended }) return else throw error
                }
            }
        } finally { runCatching { listener.close() } }
    }
    private fun response(socket: Socket, code: Int, reason: String) {
        socket.getOutputStream().apply {
            write("HTTP/1.1 $code $reason\r\nConnection: close\r\nContent-Length: 0\r\n\r\n".toByteArray(Charsets.US_ASCII)); flush()
        }
    }
    private fun serve(socket: Socket) {
        var client: StreamClient? = null
        var reached = "accepted"
        var reason = "by=local cause=teardown"
        var status = 0
        fun reply(code: Int, text: String) { status = code; response(socket, code, text) }
        try {
            socket.use {
                val request = HttpInput(socket, options.headMs).head()
                reached = "request"
                val fields = request.start.split(' ')
                if (fields.size != 3 || fields[2] !in setOf("HTTP/1.0", "HTTP/1.1")) { reply(400, "Bad Request"); reason = "by=local cause=protocol_error"; return }
                if (fields[0] !in setOf("GET", "HEAD")) { reply(405, "Method Not Allowed"); reason = "by=local cause=protocol_error"; return }
                val selected = synchronized(lock) { generation }
                if (fields[1] != path(selected)) { reply(404, "Not Found"); reason = "by=local cause=protocol_error"; return }
                if (fields[0] == "GET") {
                    var code = 200
                    synchronized(lock) {
                        expireCache(now())
                        if (ended || generation != selected) code = 404
                        else if (reservedClients >= 4) code = 503
                        else {
                            val queue = StreamClient(options.clientBytes); client = queue; sockets[socket] = queue
                            val deadline = System.nanoTime() + options.keyframeWaitMs * 1_000_000
                            if (cache.isEmpty()) needsKeyframe.set(true)
                            while (!ended && generation == selected && cache.isEmpty() && !queue.closed) {
                                val remaining = (deadline - System.nanoTime()) / 1_000_000
                                if (remaining <= 0) break
                                lock.wait(minOf(100, remaining.coerceAtLeast(1)))
                            }
                            if (ended || generation != selected) code = 404
                            else if (cache.isEmpty() || queue.closed) code = 503
                            else {
                                queue.awaitingKeyframe = false
                                for (chunk in cache) if (!queue.offer(chunk.bytes)) { code = 503; overflows.incrementAndGet(); break }
                            }
                        }
                    }
                    if (code != 200) {
                        reason = client?.endReason ?: if (code == 503 && client != null) "by=timeout cause=record_timeout timeout=keyframe limitMs=${options.keyframeWaitMs}"
                            else "by=local cause=${if (code == 503) "queue_overflow" else "replaced"}"
                        reply(code, if (code == 404) "Not Found" else "Service Unavailable"); return
                    }
                }
                status = 200; reached = if (client != null) "streaming" else "request"
                socket.getOutputStream().apply {
                    write(("HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nCache-Control: no-cache\r\nConnection: close\r\n" +
                        "transferMode.dlna.org: Streaming\r\ncontentFeatures.dlna.org: $DLNA_FEATURES\r\nServer: $USER_AGENT\r\n\r\n").toByteArray(Charsets.US_ASCII))
                    flush()
                    if (fields[0] == "GET") while (true) {
                        val bytes = client?.take() ?: break
                        write(bytes); flush(); bytesSent.addAndGet(bytes.size.toLong())
                    }
                }
            }
        } catch (error: Exception) {
            reason = castEndReason(error)
        } finally {
            val end = client?.endReason ?: if (synchronized(lock) { ended }) "by=local cause=stopped" else reason
            runCatching { diagnostic("link_lost link=renderer service=http reached=$reached $end status=$status") }
            client?.end(); sockets.remove(socket); runCatching { socket.close() }; workers.remove(Thread.currentThread())
        }
    }
    override fun close() {
        val listener = synchronized(lock) {
            if (ended) return
            ended = true; sockets.values.filter { it !== pending }.forEach { it.end("by=local cause=stopped") }; lock.notifyAll(); server
        }
        runCatching { listener?.close() }; sockets.keys.forEach { runCatching { it.close() } }
    }
    fun awaitWriters() { workers.toList().forEach { if (it !== Thread.currentThread()) it.join(1500) } }
}
