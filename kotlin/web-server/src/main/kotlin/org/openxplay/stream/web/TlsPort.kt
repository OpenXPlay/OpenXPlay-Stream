// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-18

package org.openxplay.stream.web

import java.io.ByteArrayInputStream
import java.io.SequenceInputStream
import java.net.Socket
import java.util.concurrent.TimeUnit
import javax.net.ssl.SSLSocket
import javax.net.ssl.SSLSocketFactory

/**
 * TLS shares its port with a redirect for plain HTTP: a head-unit browser given an address without
 * https:// connects in plain text. A TLS connection starts with a handshake record (0x16).
 */
internal object TlsPort {
    const val HANDSHAKE = 0x16

    /**
     * Whether [factory] layers a server socket over bytes already read (the Java 8 overload). Without it
     * TLS keeps its own listener and plain HTTP gets no redirect, so TLS itself never depends on this.
     */
    fun supportsConsumedInput(factory: SSLSocketFactory): Boolean =
        try { factory.createSocket(Socket(), ByteArrayInputStream(ByteArray(0)), true).close(); true }
        catch (_: UnsupportedOperationException) { false } catch (_: Exception) { true }

    /** A server-mode TLS socket over [raw] that first reads [consumed], the bytes already taken from it. */
    fun wrap(factory: SSLSocketFactory, raw: Socket, consumed: ByteArray): SSLSocket =
        factory.createSocket(raw, ByteArrayInputStream(consumed), true) as SSLSocket

    /** Answers one plain-HTTP request with 302 to the same host and path over https. */
    fun redirect(raw: Socket, first: Int) {
        val input = SequenceInputStream(ByteArrayInputStream(byteArrayOf(first.toByte())), raw.getInputStream())
        raw.soTimeout = TimeUnit.SECONDS.toMillis(5).toInt()
        val output = raw.getOutputStream()
        val request = try { readHttp(input) } catch (e: HttpFailure) { httpError(output, e.status); return }
        val authority = request.headers["host"].orEmpty()
        if (authority.isEmpty() || authority.any { it.code > 127 || !it.isLetterOrDigit() && it !in "._:-[]" }) {
            httpError(output, 400); return
        }
        // Not cached: a relay later serving plain HTTP must not stay redirected.
        output.write(("HTTP/1.1 302 Found\r\nLocation: https://$authority${request.target}\r\nCache-Control: no-store\r\n" +
            "Content-Length: 0\r\nConnection: close\r\n\r\n").toByteArray(Charsets.US_ASCII))
        output.flush()
    }
}
