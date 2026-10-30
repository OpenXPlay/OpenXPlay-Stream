// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-22

package org.openxplay.stream.cast

import java.io.BufferedInputStream
import java.io.ByteArrayOutputStream
import java.io.EOFException
import java.io.IOException
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.Socket
import java.net.SocketTimeoutException
import java.net.URI

internal const val USER_AGENT = "OpenXPlay/0 UPnP/1.1 OpenXPlay-Stream/0"
internal const val DLNA_FEATURES = "DLNA.ORG_OP=00;DLNA.ORG_CI=0;DLNA.ORG_FLAGS=01700000000000000000000000000000"

internal class SocketScope : AutoCloseable {
    private val lock = Any()
    private val resources = mutableSetOf<AutoCloseable>()
    private var closed = false
    fun <T : AutoCloseable> add(resource: T): T {
        val accepted = synchronized(lock) { if (closed) false else { resources.add(resource); true } }
        if (!accepted) { resource.close(); throw IOException("Network scope closed") }
        return resource
    }
    fun remove(resource: AutoCloseable) { synchronized(lock) { resources.remove(resource) } }
    override fun close() {
        val pending = synchronized(lock) { closed = true; resources.toList().also { resources.clear() } }
        pending.forEach { runCatching { it.close() } }
    }
}

internal data class HttpHead(val start: String, val headers: Map<String, String>)
internal class HttpInput(socket: Socket, timeoutMs: Int, budgetMs: Int = timeoutMs) {
    private val socket = socket
    private val input = BufferedInputStream(socket.getInputStream())
    private val timeout = timeoutMs
    private val deadline = System.nanoTime() + budgetMs * 1_000_000L
    private var lineBytes = 0
    private fun prepare() {
        val remaining = (deadline - System.nanoTime()) / 1_000_000
        if (remaining <= 0) throw SocketTimeoutException("HTTP deadline")
        socket.soTimeout = minOf(timeout.toLong(), remaining.coerceAtLeast(1)).toInt()
    }
    fun byte(): Int { prepare(); return input.read() }
    fun line(limit: Int): String {
        val bytes = ByteArrayOutputStream()
        var consumed = 0
        while (consumed < limit) {
            val c = byte(); if (c < 0) throw EOFException("Truncated HTTP line")
            consumed++
            if (c == 10) {
                val data = bytes.toByteArray()
                lineBytes = consumed
                return String(data, 0, data.size - (if (data.lastOrNull() == 13.toByte()) 1 else 0), Charsets.ISO_8859_1)
            }
            bytes.write(c)
        }
        throw IOException("HTTP head too large")
    }
    fun head(limit: Int = 16 * 1024): HttpHead {
        val start = line(limit); var used = lineBytes
        val headers = linkedMapOf<String, String>()
        while (true) {
            val line = line(limit - used); used += lineBytes
            if (line.isEmpty()) return HttpHead(start, headers)
            val colon = line.indexOf(':'); require(colon > 0 && !line[0].isWhitespace()) { "Invalid HTTP header" }
            val key = line.substring(0, colon).lowercase(java.util.Locale.ROOT)
            require(key.all { it.isLetterOrDigit() || it in "!#$%&'*+-.^_`|~" }) { "Invalid HTTP header name" }
            val value = line.substring(colon + 1).trim()
            if (key !in headers) headers[key] = value
        }
    }
    fun exact(count: Int): ByteArray {
        val bytes = ByteArray(count); var at = 0
        while (at < count) {
            prepare(); val n = input.read(bytes, at, count - at)
            if (n < 0) throw EOFException("Truncated HTTP body"); at += n
        }
        return bytes
    }
    fun body(headers: Map<String, String>, limit: Int): ByteArray {
        val transfer = headers["transfer-encoding"]
        if (transfer != null) {
            require(transfer.equals("chunked", true)) { "Unsupported transfer encoding" }
            val body = ByteArrayOutputStream()
            while (true) {
                val size = line(1024).substringBefore(';').trim().toLongOrNull(16)
                require(size != null && size >= 0 && size <= limit - body.size()) { "HTTP body too large" }
                if (size == 0L) {
                    var trailers = 0
                    do { val line = line(16 * 1024 - trailers); trailers += lineBytes } while (line.isNotEmpty())
                    return body.toByteArray()
                }
                body.write(exact(size.toInt())); require(line(2).isEmpty()) { "Invalid HTTP chunk ending" }
            }
        }
        headers["content-length"]?.let {
            val size = it.toLongOrNull(); require(size != null && size in 0..limit.toLong()) { "HTTP body too large" }
            return exact(size.toInt())
        }
        val body = ByteArrayOutputStream()
        while (true) {
            val c = byte(); if (c < 0) return body.toByteArray()
            require(body.size() < limit) { "HTTP body too large" }; body.write(c)
        }
    }
}

internal data class HttpResponse(val status: Int, val body: ByteArray, val localAddress: InetAddress)
internal fun httpUri(uri: URI): URI {
    require(uri.scheme.equals("http", true) && !uri.host.isNullOrBlank() && uri.userInfo == null && uri.fragment == null &&
        (uri.port == -1 || uri.port in 1..65535)) { "Invalid HTTP URL" }
    return uri
}

internal class CastHttpClient(private val network: CastNetwork, private val scope: SocketScope = SocketScope(),
    private val diagnostic: (String) -> Unit = {}) {
    fun request(uri: URI, method: String = "GET", headers: Map<String, String> = emptyMap(), body: ByteArray = byteArrayOf(),
        limit: Int = 64 * 1024, connectMs: Int = 2000, readMs: Int = 5000, budgetMs: Int = connectMs + readMs): HttpResponse {
        httpUri(uri)
        val started = System.nanoTime()
        var connected = false
        var status = 0
        val socket = try { scope.add(network.socket()) } catch (error: Exception) {
            runCatching { diagnostic("request_failed link=renderer reached=connecting ${castEndReason(error, true)}") }; throw error
        }
        try {
            socket.use {
                socket.connect(InetSocketAddress(uri.host, if (uri.port < 0) 80 else uri.port), minOf(connectMs, budgetMs))
                connected = true
                val remaining = budgetMs - ((System.nanoTime() - started) / 1_000_000).toInt()
                if (remaining <= 0) throw SocketTimeoutException("HTTP deadline")
                val path = uri.rawPath.takeUnless { it.isNullOrEmpty() } ?: "/"
                val target = path + (uri.rawQuery?.let { "?$it" } ?: "")
                val host = (if (':' in uri.host) "[${uri.host}]" else uri.host) + (if (uri.port < 0) "" else ":${uri.port}")
                val head = buildString {
                    append("$method $target HTTP/1.1\r\nHost: $host\r\nConnection: close\r\nUser-Agent: $USER_AGENT\r\n")
                    headers.forEach { (key, value) -> require('\r' !in value && '\n' !in value); append("$key: $value\r\n") }
                    if (method == "POST") append("Content-Length: ${body.size}\r\n")
                    append("\r\n")
                }
                socket.getOutputStream().apply { write(head.toByteArray(Charsets.UTF_8)); write(body); flush() }
                val input = HttpInput(socket, readMs, remaining)
                val response = input.head()
                val parts = response.start.split(' ', limit = 3)
                require(parts.size >= 2 && parts[0] in setOf("HTTP/1.0", "HTTP/1.1")) { "Invalid HTTP response" }
                status = parts[1].toInt(); require(status in 100..599)
                return HttpResponse(status, input.body(response.headers, limit), socket.localAddress)
            }
        } catch (error: Exception) {
            runCatching { diagnostic("request_failed link=renderer reached=${if (connected) "http" else "connecting"} ${castEndReason(error, !connected)} limitMs=$budgetMs status=$status") }
            throw error
        } finally { scope.remove(socket) }
    }
}
