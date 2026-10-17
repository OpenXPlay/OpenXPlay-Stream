// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

package org.openxplay.stream.web

import java.io.ByteArrayOutputStream
import java.net.Socket
import java.nio.file.Files
import java.nio.file.Path
import kotlinx.serialization.json.*
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.io.TempDir
import org.openxplay.stream.core.*
import kotlin.test.*

class PluginsTest {
    @TempDir lateinit var root: Path
    private fun vectors(name: String): JsonArray = Json.parseToJsonElement(checkNotNull(javaClass.getResource("/vectors/plugins/$name.json")).readText()).jsonArray
    private fun fixture(): JsonObject = vectors("schema").first { it.jsonObject["name"] == JsonPrimitive("complete manifest") }.jsonObject.getValue("value").jsonObject
    private fun installed(persistent: Boolean = false): Plugins {
        val manifest = fixture(); val id = manifest.getValue("id").jsonPrimitive.content
        val pkg = root.resolve("p/$id"); Files.createDirectories(pkg)
        Files.writeString(pkg.resolve("oxp-plugin.json"), manifest.toString())
        Files.createDirectories(root.resolve("host")); Files.writeString(root.resolve("host/index.html"), "hello")
        return Plugins(PluginsConfig(root, listOf(id), if (persistent) root.resolve("state") else null))
    }
    @Test fun partialRoutesAreAdvertisedAndDeliveredWithoutClearing() {
        val p = installed()
        val manifest = JsonObject(fixture() + ("permissions" to JsonArray(fixture().getValue("permissions").jsonArray + JsonPrimitive("route"))))
        assertTrue(PluginContract.manifest(manifest))
        assertTrue(JsonPrimitive("route") in p.welcome().getValue("topics").jsonArray)
        val value = Json.parseToJsonElement("""{"active":true,"destination":null,"etaMs":null,"distanceRemainingM":null,"maneuver":{"type":2,"road":null,"distanceM":null},"appName":null}""")
        p.update(ContextJson.decode(buildJsonObject { put("topic", "route"); put("value", value) }))
        val channel = p.Channel()
        channel.receive("""{"type":"hello","spec":"oxp-web-plugin/0","host":"test"}""")
        val state = channel.receive("""{"type":"subscribe","topics":["route"]}""").single()
        assertEquals(value, state["value"])
        assertTrue(PluginContract.channel(state))
    }
    @Test fun copiedVectors() {
        vectors("paths").forEach { value -> val v = value.jsonObject; assertEquals(v.getValue("allowed").jsonPrimitive.boolean, PluginContract.safePath(v.getValue("path").jsonPrimitive.content), v.toString()) }
        vectors("csp").forEach { value ->
            val v = value.jsonObject; val h = PluginContract.headers(v.getValue("origin").jsonPrimitive.content, v["plugin"]?.jsonPrimitive?.content)
            v.getValue("headers").jsonObject.forEach { (key, expected) -> assertEquals(expected.jsonPrimitive.content, h[key]) }
            assertEquals("null", h["Access-Control-Allow-Origin"])
        }
        vectors("schema").forEach { value ->
            val v = value.jsonObject; val schema = v.getValue("schema").jsonPrimitive.content
            val actual = when {
                schema == "manifest" -> PluginContract.manifest(v.getValue("value").jsonObject)
                schema == "channel-message" -> PluginContract.channel(v.getValue("value").jsonObject)
                schema.startsWith("topic-") -> PluginContract.topic(schema.removePrefix("topic-"), v.getValue("value"))
                else -> return@forEach
            }
            assertEquals(v.getValue("valid").jsonPrimitive.boolean, actual, v.getValue("name").jsonPrimitive.content)
        }
    }
    @Test fun disabledGoldenWelcomeAndOrdinaryStaticRoutes() {
        val url = "https://github.com/OpenXPlay/OpenXPlay-Stream"
        val output = WebOutput(WebOutputConfig(bindAddress = "127.0.0.1", port = 0, staticAssetRoot = root.resolve("absent"), tokenVerifier = TokenVerifier { true }, sourceUrl = url))
        assertFalse(output.wantsContext())
        val golden = Json.parseToJsonElement(checkNotNull(javaClass.getResource("/vectors/web/welcome.json")).readText()).jsonObject
        val welcome = receiverWelcome("browser-1", SessionState(SessionId(42), SessionPhase.ACTIVE), url)
        assertContentEquals(golden.toString().toByteArray(), WebJsonCodec.encode(welcome).toByteArray())
        output.open(NoopHost)
        try {
            val port = output.awaitListening()
            fun request(method: String, path: String, upgrade: Boolean = false): ByteArray = Socket("127.0.0.1", port).use { socket ->
                socket.soTimeout = 3000
                val headers = if (upgrade) {
                    "Connection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n"
                } else "Connection: close\r\n"
                socket.getOutputStream().write("$method $path HTTP/1.1\r\nHost: localhost\r\n$headers\r\n".toByteArray())
                socket.getInputStream().readBytes().also { assertTrue(it.isNotEmpty()) }
            }
            for (method in listOf("GET", "HEAD", "POST")) for (tail in listOf(
                "index.json", "host/index.html", "channel", "p/org.test.clock/index.html", "artwork/x",
            )) {
                assertContentEquals(request(method, "/zz-missing/$tail"), request(method, "/plugins/$tail"), "$method /plugins/$tail")
            }
            assertContentEquals(request("GET", "/zz-missing/channel", true), request("GET", "/plugins/channel", true), "WebSocket upgrade /plugins/channel")
        } finally { output.close() }
        assertFalse(Files.exists(root.resolve("absent")))
    }
    @Test fun storageRateLimitPersistenceAndConnectionCap() {
        val p = installed(true); val c = p.Channel(); val id = fixture().getValue("id").jsonPrimitive.content
        fun receive(raw: String): JsonObject = c.receive(raw).single()
        assertEquals("protocol", receive("{}").getValue("code").jsonPrimitive.content)
        assertTrue(c.receive("{\"type\":\"future\"}").isEmpty())
        assertEquals(p.welcome(), receive("{\"type\":\"hello\",\"spec\":\"oxp-web-plugin/0\",\"host\":\"test\"}"))
        val set = buildJsonObject { put("type", "storage-set"); put("plugin", id); put("id", "1"); put("value", buildJsonObject { put("saved", 1) }) }
        assertEquals(JsonPrimitive("storage-ok"), c.receive(set.toString()).single()["type"])
        assertEquals(JsonPrimitive("rate"), c.receive(set.toString()).single()["code"])
        val reopened = installed(true); val other = reopened.Channel()
        other.receive("{\"type\":\"hello\",\"spec\":\"oxp-web-plugin/0\",\"host\":\"test\"}")
        val get = buildJsonObject { put("type", "storage-get"); put("plugin", id); put("id", "2") }
        assertEquals(set["value"], other.receive(get.toString()).single()["value"])
        val permits = List(4) { p.reserve() }; val newest = p.reserve()
        assertTrue(permits.first().replaced.get()); assertFalse(newest.replaced.get())
        permits.forEach { it.close() }; newest.close()
    }
    @Test fun latestSnapshotsAreBoundedAndStaticRoutesAreConfined() {
        val p = installed(); val c = p.Channel()
        c.receive("{\"type\":\"hello\",\"spec\":\"oxp-web-plugin/0\",\"host\":\"test\"}")
        assertEquals(JsonNull, c.receive("{\"type\":\"subscribe\",\"topics\":[\"session\"]}").single()["value"])
        p.update(ContextUpdate.Session(SessionContext(CarPlayState.CONNECTED))); p.update(ContextUpdate.Session(SessionContext(CarPlayState.DISCONNECTED)))
        assertTrue(c.flush().isEmpty())
        val update = c.flush(System.nanoTime() + 251_000_000).single()
        assertEquals(JsonPrimitive(2), update["seq"]); assertEquals(JsonPrimitive("disconnected"), update["value"]!!.jsonObject["carplay"])
        val subscriber = p.Channel()
        subscriber.receive("{\"type\":\"hello\",\"spec\":\"oxp-web-plugin/0\",\"host\":\"test\"}")
        val snapshots = subscriber.receive("{\"type\":\"subscribe\",\"topics\":[\"session\",\"activity\",\"session\"]}")
        assertEquals(listOf("session", "activity"), snapshots.map { it.getValue("topic").jsonPrimitive.content })
        assertTrue(snapshots.all { it["seq"] == JsonPrimitive(2) })
        assertTrue(subscriber.flush(System.nanoTime() + 251_000_000).isEmpty())
        fun request(target: String, method: String = "GET"): String {
            val out = ByteArrayOutputStream(); PluginContract.serve(p, HttpRequest(method, target, emptyMap()), "http://localhost", out); return out.toString(Charsets.UTF_8)
        }
        assertTrue(request("/plugins/host/index.html").endsWith("\r\n\r\nhello"))
        assertTrue(request("/plugins/host/index.html", "HEAD").endsWith("\r\n\r\n"))
        listOf("/plugins/host/../index.html", "/plugins/host/%2e%2e/index.html", "/plugins/host/.hidden.js", "/plugins/p/org.disabled/oxp-plugin.json").forEach { assertTrue(request(it).startsWith("HTTP/1.1 404")) }
        val outside = Files.createTempFile(root.parent, "oxp-private-", ".txt")
        Files.createSymbolicLink(root.resolve("host/escape.txt"), outside)
        Files.writeString(outside, "private")
        try { assertTrue(request("/plugins/host/escape.txt").startsWith("HTTP/1.1 404")) } finally { Files.deleteIfExists(outside) }
    }
    @Test fun oversizedStorageRepliesRespectChannelCap() {
        val p = installed(true); val id = fixture().getValue("id").jsonPrimitive.content
        val directory = root.resolve("state/plugins/storage"); Files.createDirectories(directory)
        val value = buildJsonObject { put("s", "x".repeat(PluginContract.LIMIT - 8)) }
        assertEquals(PluginContract.LIMIT, value.toString().toByteArray().size)
        Files.writeString(directory.resolve("$id.json"), value.toString())
        val reloaded = Plugins(p.config); val channel = reloaded.Channel()
        channel.receive("{\"type\":\"hello\",\"spec\":\"oxp-web-plugin/0\",\"host\":\"test\"}")
        val get = buildJsonObject { put("type", "storage-get"); put("plugin", id); put("id", "read") }
        assertEquals(JsonPrimitive("quota"), channel.receive(get.toString()).single()["code"])
    }

    @Test fun providerVectorsAndDefaultTopics() {
        val provider = Json.parseToJsonElement(checkNotNull(javaClass.getResource("/vectors/plugins/provider-relay.json")).readText()).jsonObject
        val p = Plugins(PluginsConfig(root, emptyList()))
        assertEquals(provider["topics"], p.welcome()["topics"])
        provider.getValue("vectors").jsonArray.forEach { entry ->
            val v = entry.jsonObject
            assertEquals(v.getValue("valid").jsonPrimitive.boolean, PluginContract.topic(v.getValue("topic").jsonPrimitive.content, v.getValue("value")), v["name"].toString())
        }
        provider.getValue("messages").jsonArray.forEach { entry ->
            val v = entry.jsonObject
            assertEquals(v.getValue("valid").jsonPrimitive.boolean, PluginContract.channel(v.getValue("value").jsonObject), v["name"].toString())
        }
        val channel = p.Channel()
        channel.receive("""{"type":"hello","spec":"oxp-web-plugin/0","host":"test"}""")
        p.update(ContextUpdate.AudioSource(AudioSourceContext("Music")))
        assertEquals(JsonPrimitive("Music"), channel.receive("""{"type":"subscribe","topics":["audioSource"]}""").single().getValue("value").jsonObject["appName"])
    }
    @Test fun configuredAvailabilityFiltersDeliveryAndUnknownSubscriptions() {
        listOf(listOf("session", "activity", "route", "audioSource"), emptyList()).forEach { topics ->
            val p = Plugins(PluginsConfig(root, emptyList(), topics = topics))
            val channel = p.Channel()
            val welcome = channel.receive("""{"type":"hello","spec":"oxp-web-plugin/0","host":"test"}""").single()
            assertEquals(JsonArray(topics.map(::JsonPrimitive)), welcome["topics"])
            assertTrue(channel.receive("""{"type":"subscribe","topics":["nowPlaying","call","future","motion","parked","future2","future3"]}""").isEmpty())
            p.update(ContextUpdate.Call(CallContext(CallState.ACTIVE)))
            assertTrue(channel.flush(System.nanoTime() + 1_000_000_000).isEmpty())
            assertTrue(channel.receive("""{"type":"subscribe","topics":["future"]}""").isEmpty())
            val states = channel.receive("""{"type":"subscribe","topics":["future","session"]}""")
            assertEquals(if (topics.isEmpty()) 0 else 1, states.size)
            states.firstOrNull()?.let { assertEquals(JsonPrimitive("session"), it["topic"]) }
        }
    }

    private class ChannelServer(val plugins: Plugins) : AutoCloseable {
        private val server = java.net.ServerSocket(0, 10, java.net.InetAddress.getLoopbackAddress())
        private val timers = java.util.concurrent.Executors.newSingleThreadScheduledExecutor()
        private val workers = mutableListOf<Thread>()
        fun connect(): TestClient {
            val socket = Socket("127.0.0.1", server.localPort)
            val peer = server.accept()
            val worker = Thread {
                val permit = plugins.reserve()
                try {
                    upgrade(readHttp(peer.getInputStream()), peer.getOutputStream())
                    plugins.channel(peer, permit, timers) { true }
                } catch (_: java.io.IOException) { }
                finally { permit.close(); peer.close() }
            }
            workers.add(worker); worker.start()
            return TestClient(socket, "/plugins/channel")
        }
        override fun close() {
            server.close()
            workers.forEach { it.join(3000); assertFalse(it.isAlive) }
            timers.shutdownNow()
        }
    }
    private fun TestClient.pluginHello() {
        send(1, """{"type":"hello","spec":"oxp-web-plugin/0","host":"test"}""".toByteArray())
        while (true) {
            val m = read()
            if (m.opcode == 9) send(10, m.payload)
            else { assertEquals(JsonPrimitive("welcome"), Json.parseToJsonElement(utf8(m.payload)).jsonObject["type"]); return }
        }
    }
    @Test fun silentChannelTimesOutAndReleasesSlot() {
        ChannelServer(Plugins(PluginsConfig(root, emptyList()), ChannelTiming(40, 200, 100))).use { server ->
            server.connect().use { client ->
                var pings = 0
                try { while (true) { assertEquals(9, client.read().opcode); pings++ } }
                catch (_: java.io.EOFException) { }
                assertTrue(pings > 0)
            }
        }
    }
    @Test fun responsiveChannelSurvivesIdleTimeout() {
        ChannelServer(Plugins(PluginsConfig(root, emptyList()), ChannelTiming(40, 200, 100))).use { server ->
            server.connect().use { client ->
                client.pluginHello()
                val start = System.nanoTime()
                while (System.nanoTime() - start < 600_000_000) {
                    val m = client.read(); assertEquals(9, m.opcode); client.send(10, m.payload)
                }
                client.send(8, closePayload(1000))
            }
        }
    }
    @Test fun fifthChannelReplacesOldestAndGetsWelcome() {
        ChannelServer(Plugins(PluginsConfig(root, emptyList()))).use { server ->
            val clients = List(4) { server.connect().also { it.pluginHello() } }
            try {
                server.connect().use { newest ->
                    newest.pluginHello()
                    val close = clients.first().read()
                    assertEquals(8, close.opcode)
                    assertContentEquals(closePayload(1001) + "replaced".toByteArray(), close.payload)
                }
            } finally { clients.forEach { it.close() } }
        }
    }

    @Test fun unavailableChannelIs503InsteadOfStatic404() {
        val output = ByteArrayOutputStream()
        PluginContract.serve(installed(), HttpRequest("HEAD", "/plugins/channel", emptyMap()), "http://localhost", output)
        assertTrue(output.toString(Charsets.US_ASCII).startsWith("HTTP/1.1 503 Service Unavailable\r\n"))
    }
    @Test fun everyCompleteFrameIncludingContinuationsSignalsLiveness() {
        val bytes = masked(1, "a".toByteArray(), false) + masked(0, "b".toByteArray(), false) +
            masked(9, byteArrayOf()) + masked(0, "c".toByteArray())
        var frames = 0
        val reader = WebSocketReader(bytes.inputStream(), 1024) { frames++ }
        assertEquals(9, reader.read().opcode)
        assertEquals(3, frames)
        assertEquals("abc", utf8(reader.read().payload))
        assertEquals(4, frames)
    }

}
