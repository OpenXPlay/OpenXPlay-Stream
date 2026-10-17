// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.clink.wfd

/** RTSP text messages; header names are lower case. */
public data class RtspMessage(public val line: String, public val headers: Map<String, String>, public val body: String = "") {
    public fun encode(): ByteArray {
        require(!line.contains('\r') && !line.contains('\n'))
        val payload = body.toByteArray(Charsets.UTF_8)
        val text = buildString {
            append(line).append("\r\n")
            for ((key, value) in headers.toSortedMap()) {
                require(key.none { it == '\r' || it == '\n' } && value.none { it == '\r' || it == '\n' })
                if (key.lowercase() != "content-length" && key.lowercase() != "content-type")
                    append(key.lowercase()).append(": ").append(value).append("\r\n")
            }
            if (payload.isNotEmpty()) append("content-length: ${payload.size}\r\ncontent-type: text/parameters\r\n")
            append("\r\n")
        }
        return text.toByteArray(Charsets.US_ASCII) + payload
    }
}

/** Incremental, bounded decoder; skips incoming interleaved feedback. */
public class RtspReader {
    private var buffer = byteArrayOf()
    public fun feed(bytes: ByteArray): List<RtspMessage> {
        require(buffer.size.toLong() + bytes.size <= 131072) { "rtsp_buffer_limit" }
        buffer += bytes
        val messages = mutableListOf<RtspMessage>()
        while (buffer.isNotEmpty()) {
            if (buffer[0] == 36.toByte()) {
                if (buffer.size < 4) break
                val size = ((buffer[2].toInt() and 255) shl 8) or (buffer[3].toInt() and 255)
                if (buffer.size < size + 4) break
                buffer = buffer.copyOfRange(size + 4, buffer.size); continue
            }
            val end = buffer.toString(Charsets.ISO_8859_1).indexOf("\r\n\r\n")
            if (end < 0) { require(buffer.size <= 8192); break }
            require(end <= 8192)
            val lines = buffer.copyOfRange(0, end).toString(Charsets.US_ASCII).split("\r\n")
            require(lines[0].isNotEmpty())
            val headers = mutableMapOf<String, String>()
            for (line in lines.drop(1)) {
                val colon = line.indexOf(':'); require(colon > 0)
                val key = line.substring(0, colon).trim().lowercase()
                require(key.isNotEmpty() && key !in headers) { "duplicate_rtsp_header" }
                headers[key] = line.substring(colon + 1).trim()
            }
            val size = headers["content-length"]?.let { it.toIntOrNull() ?: error("invalid_content_length") } ?: 0
            require(size in 0..65536)
            if (buffer.size < end + 4 + size) break
            // Body framing headers are owned by RtspMessage.encode, so they are not exposed.
            headers.remove("content-length"); headers.remove("content-type")
            messages += RtspMessage(lines[0], headers, buffer.copyOfRange(end + 4, end + 4 + size).toString(Charsets.UTF_8))
            buffer = buffer.copyOfRange(end + 4 + size, buffer.size)
        }
        return messages
    }
}

internal fun parameters(body: String): Map<String, String> = body.lineSequence().map { it.trim() }
    .filter { it.isNotEmpty() }.associate { it.substringBefore(':').trim() to it.substringAfter(':', "").trim() }
internal fun parameterBody(values: Map<String, String>): String = values.toSortedMap().entries.joinToString("") { "${it.key}: ${it.value}\r\n" }
