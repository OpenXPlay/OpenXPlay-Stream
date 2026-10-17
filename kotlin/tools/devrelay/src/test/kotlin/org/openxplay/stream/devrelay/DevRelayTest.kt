// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.devrelay

import java.io.ByteArrayOutputStream
import java.io.PrintStream
import java.net.URI
import java.net.http.HttpClient
import java.net.http.WebSocket
import java.nio.ByteBuffer
import java.nio.file.Files
import java.nio.file.Path
import java.time.Duration
import java.util.concurrent.CompletableFuture
import java.util.concurrent.CompletionStage
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.Timeout
import org.junit.jupiter.api.io.TempDir
import org.openxplay.stream.core.TouchContact
import org.openxplay.stream.core.wire.*
import kotlin.test.*

class DevRelayTest {
    @TempDir lateinit var assets: Path

    @Test
    @Timeout(value = 12, unit = TimeUnit.SECONDS)
    fun webTouchReachesInputAndEventLog() {
        val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(9)
        fun remaining(): Long = (deadline - System.nanoTime()).coerceAtLeast(1)
        val root = repositoryRoot()
        val token = "example-pairing-token"
        val capture = ByteArrayOutputStream()
        val messages = Messages()
        var socket: WebSocket? = null
        PrintStream(capture, true, Charsets.UTF_8).use { stdout ->
            DevRelay.start(RelayOptions(
                clip = root.resolve("core/src/test/resources/media/baseline-640x360.h264"),
                webPort = 0, webBind = "127.0.0.1", webRoot = assets, token = token,
            ), stdout).use { relay ->
                try {
                    assertTrue(relay.webPort!! > 0)
                    assertEquals("http://127.0.0.1:${relay.webPort}/#k=$token", relay.receiverUrl)
                    val client = HttpClient.newBuilder().connectTimeout(Duration.ofSeconds(2)).build()
                    val connected = client.newWebSocketBuilder().connectTimeout(Duration.ofSeconds(2))
                        .buildAsync(URI("ws://127.0.0.1:${relay.webPort}/oxp/v0"), messages)
                        .get(remaining(), TimeUnit.NANOSECONDS)
                    socket = connected
                    // Use the complete canonical hello, including its fixed token.
                    val hello = Files.readString(root.resolve("../spec/vectors/web/hello.json"))
                    assertTrue(hello.contains("\"token\": \"$token\""))
                    connected.sendText(hello, true).get(remaining(), TimeUnit.NANOSECONDS)
                    var welcome: String? = null
                    var format: String? = null
                    while (welcome == null || format == null) {
                        val text = messages.text.poll(remaining(), TimeUnit.NANOSECONDS)
                            ?: fail("Timed out waiting for welcome/video-format: ${messages.failure}")
                        if (Regex("\"type\"\\s*:\\s*\"welcome\"").containsMatchIn(text)) welcome = text
                        if (Regex("\"type\"\\s*:\\s*\"video-format\"").containsMatchIn(text) &&
                            Regex("\"width\"\\s*:\\s*640").containsMatchIn(text)) format = text
                    }
                    val clientId = Regex("\"clientId\"\\s*:\\s*\"([^\"]+)\"")
                        .find(assertNotNull(welcome))!!.groupValues[1]
                    assertTrue(Regex("\"height\"\\s*:\\s*360").containsMatchIn(assertNotNull(format)))
                    val contacts = listOf(TouchContact(2, 0.25, 0.75, true))
                    val payload = OxpWebCodec.encodeTouch(contacts)
                    val touch = OxpWebCodec.encode(BinaryMessage(
                        BinaryHeader(BinaryKind.TOUCH, 0, 0, 0u, 0, System.nanoTime() / 1000), payload))
                    connected.sendBinary(ByteBuffer.wrap(touch), true).get(remaining(), TimeUnit.NANOSECONDS)
                    var touchLine: String? = null
                    while (System.nanoTime() < deadline && touchLine == null) {
                        touchLine = capture.toString(Charsets.UTF_8).lineSequence().firstOrNull {
                            it.contains("\"event\":\"control\"") && it.contains("\"type\":\"touch\"")
                        }
                        if (touchLine == null) Thread.sleep(5)
                    }
                    val line = assertNotNull(touchLine, "No touch event: ${capture.toString(Charsets.UTF_8)}")
                    assertContains(line, "\"origin\":{\"outputId\":\"web\",\"clientId\":\"$clientId\"}")
                    assertContains(line, "\"screen\":0")
                    val decoded = OxpWebCodec.decodeTouch(payload).single()
                    assertContains(line, "\"contacts\":[{\"id\":2,\"x\":${decoded.x},\"y\":${decoded.y},\"pressed\":true}]")
                } finally { socket?.abort() }
            }
            val lines = capture.toString(Charsets.UTF_8).trim().lines()
            assertEquals("{\"event\":\"shutdown\"}", lines.last())
            assertTrue(lines.all { it.startsWith('{') && it.endsWith('}') })
            assertTrue(lines.any { it.contains("\"event\":\"capabilities\"") })
            assertTrue(lines.any { it.contains("\"event\":\"output-opened\"") })
        }
    }
}

private class Messages : WebSocket.Listener {
    val text = LinkedBlockingQueue<String>()
    @Volatile var failure: Throwable? = null
    private val fragments = StringBuilder()
    override fun onOpen(webSocket: WebSocket) { webSocket.request(1) }
    override fun onText(webSocket: WebSocket, data: CharSequence, last: Boolean): CompletionStage<*> {
        fragments.append(data)
        if (last) { text.add(fragments.toString()); fragments.setLength(0) }
        webSocket.request(1)
        return CompletableFuture.completedFuture<Void>(null)
    }
    override fun onBinary(webSocket: WebSocket, data: ByteBuffer, last: Boolean): CompletionStage<*> {
        webSocket.request(1)
        return CompletableFuture.completedFuture<Void>(null)
    }
    override fun onError(webSocket: WebSocket, error: Throwable) { failure = error }
}
