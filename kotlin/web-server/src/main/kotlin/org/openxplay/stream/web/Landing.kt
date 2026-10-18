// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-18

package org.openxplay.stream.web

import java.io.OutputStream
import java.net.URI

internal fun serveLanding(request: HttpRequest, tlsPort: Int, output: OutputStream) {
    val authority = request.headers["host"].orEmpty()
    if (authority.isEmpty() || authority.any { it.code > 127 || !it.isLetterOrDigit() && it !in "._:-[]" }) throw HttpFailure(400)
    val uri = try { URI("http://$authority") } catch (_: Exception) { throw HttpFailure(400) }
    val host = uri.host ?: throw HttpFailure(400)
    if (uri.rawAuthority != authority || authority.endsWith(':') || uri.port !in -1..65535) throw HttpFailure(400)
    if (request.method !in setOf("GET", "HEAD")) throw HttpFailure(405)
    val body = """<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
        <title>Open CarPlay</title><style>:root{color-scheme:light dark}body{font:20px system-ui,sans-serif;text-align:center;margin:0;padding:8vh 6vw}a{display:block;padding:28px 16px;border-radius:16px;background:#16723c;color:white;text-decoration:none;font-size:clamp(24px,4vw,40px);font-weight:700}p{line-height:1.5;max-width:44em;margin:24px auto}</style>
        <a href="https://$host:$tlsPort/">Open CarPlay · 打开 CarPlay</a>
        <p>Your browser may warn about the relay's own certificate; continuing is expected.<br>浏览器可能提示中继设备的自有证书风险，请继续访问。</p></html>""".trimIndent().toByteArray(Charsets.UTF_8)
    output.write("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nConnection: close\r\nContent-Length: ${body.size}\r\n\r\n".toByteArray(Charsets.US_ASCII))
    if (request.method == "GET") output.write(body)
    output.flush()
}
