// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.web

import java.io.InputStream
import java.io.OutputStream
import java.net.URI
import java.nio.file.Files
import java.nio.file.Path
import java.security.MessageDigest
import java.util.Base64
import java.util.Locale

internal class HttpFailure(val status: Int) : Exception()
internal data class HttpRequest(val method: String, val target: String, val headers: Map<String, String>)

internal fun readHttp(input: InputStream): HttpRequest {
    var total = 0
    fun line(limit: Int): String {
        val result = StringBuilder()
        while (true) {
            val b = input.read()
            if (b < 0) throw HttpFailure(400)
            total++
            if (total > 32768 || result.length >= limit) throw HttpFailure(431)
            if (b == 13) {
                if (input.read() != 10) throw HttpFailure(400)
                total++
                return result.toString()
            }
            if (b !in 32..126 && b != 9) throw HttpFailure(400)
            result.append(b.toChar())
        }
    }
    val request = line(4096).split(' ')
    if (request.size != 3 || !Regex("[A-Z]+").matches(request[0]) || !request[1].startsWith('/') || request[2] != "HTTP/1.1") throw HttpFailure(400)
    val headers = linkedMapOf<String, String>()
    while (true) {
        val line = line(8192)
        if (line.isEmpty()) break
        val colon = line.indexOf(':')
        if (colon <= 0 || !Regex("[!#$%&'*+.^_`|~0-9A-Za-z-]+").matches(line.substring(0, colon))) throw HttpFailure(400)
        val name = line.substring(0, colon).lowercase(Locale.ROOT)
        if (headers.put(name, line.substring(colon + 1).trim()) != null || headers.size > 100) throw HttpFailure(400)
    }
    if (headers["host"].isNullOrEmpty() || headers.containsKey("transfer-encoding") || headers["content-length"]?.let { it != "0" } == true) throw HttpFailure(400)
    return HttpRequest(request[0], request[1], headers)
}

internal fun upgrade(request: HttpRequest, output: OutputStream) {
    fun tokens(name: String): List<String> = request.headers[name]?.lowercase(Locale.ROOT)?.split(',')?.map { it.trim() } ?: emptyList()
    val key = request.headers["sec-websocket-key"] ?: throw HttpFailure(400)
    val decoded = try { Base64.getDecoder().decode(key) } catch (_: IllegalArgumentException) { throw HttpFailure(400) }
    if ("websocket" !in tokens("upgrade") || "upgrade" !in tokens("connection") || request.headers["sec-websocket-version"] != "13" || decoded.size != 16) throw HttpFailure(400)
    val accept = Base64.getEncoder().encodeToString(MessageDigest.getInstance("SHA-1").digest((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").toByteArray(Charsets.US_ASCII)))
    output.write("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: $accept\r\n\r\n".toByteArray(Charsets.US_ASCII))
    output.flush()
}

internal fun serveStatic(request: HttpRequest, root: Path, output: OutputStream) {
    val uri = try { URI(request.target) } catch (_: Exception) { throw HttpFailure(404) }
    if (uri.isAbsolute || uri.rawAuthority != null || uri.rawFragment != null) throw HttpFailure(404)
    var path = uri.path ?: throw HttpFailure(404)
    if (path.contains('\\') || path.contains('\u0000') || path.split('/').any { it == ".." || it == "." }) throw HttpFailure(404)
    path = when (path) { "/" -> "/receiver/index.html"; "/probe/" -> "/probe/index.html"; else -> path }
    val base = try { root.toRealPath() } catch (_: Exception) { throw HttpFailure(404) }
    val file = try { base.resolve(path.removePrefix("/")).normalize().toRealPath() } catch (_: Exception) { throw HttpFailure(404) }
    if (!file.startsWith(base) || !Files.isRegularFile(file)) throw HttpFailure(404)
    val type = when (file.fileName.toString().substringAfterLast('.').lowercase(Locale.ROOT)) {
        "html" -> "text/html; charset=utf-8"; "js", "mjs" -> "text/javascript; charset=utf-8"
        "css" -> "text/css; charset=utf-8"; "json", "map" -> "application/json"
        "svg" -> "image/svg+xml"; "png" -> "image/png"; "jpg", "jpeg" -> "image/jpeg"
        "ico" -> "image/x-icon"; "wasm" -> "application/wasm"; "woff2" -> "font/woff2"
        else -> "application/octet-stream"
    }
    val size = Files.size(file)
    output.write(("HTTP/1.1 200 OK\r\nContent-Type: $type\r\nContent-Length: $size\r\nConnection: close\r\n" +
        if (type.startsWith("text/html")) "Cache-Control: no-store\r\n\r\n" else "\r\n").toByteArray(Charsets.US_ASCII))
    Files.newInputStream(file).use { it.copyTo(output) }
    output.flush()
}

/**
 * Half-closes after an error response and discards unread request bytes for a bounded time, so the
 * kernel does not reset the connection before the client reads the response.
 */
internal fun drainBeforeClose(socket: java.net.Socket) {
    socket.shutdownOutput()
    socket.soTimeout = 500
    val input = socket.getInputStream()
    val buffer = ByteArray(4096)
    var remaining = 65536
    while (remaining > 0) {
        val read = input.read(buffer)
        if (read < 0) break
        remaining -= read
    }
}

internal fun httpError(output: OutputStream, status: Int) {
    val reason = when (status) { 400 -> "Bad Request"; 405 -> "Method Not Allowed"; 431 -> "Request Header Fields Too Large"; 503 -> "Service Unavailable"; else -> "Not Found" }
    output.write(("HTTP/1.1 $status $reason\r\nContent-Length: 0\r\nConnection: close\r\n" +
        if (status == 405) "Allow: GET\r\n\r\n" else "\r\n").toByteArray(Charsets.US_ASCII))
    output.flush()
}
