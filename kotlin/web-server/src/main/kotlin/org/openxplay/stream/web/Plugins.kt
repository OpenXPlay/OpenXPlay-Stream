// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

package org.openxplay.stream.web

import java.io.OutputStream
import java.net.Socket
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.StandardCopyOption
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import kotlinx.serialization.json.*
import org.openxplay.stream.core.ContextUpdate
import org.openxplay.stream.core.ContextJson

/** Supplying null in WebOutputConfig leaves all plugin work disabled. */
public data class PluginsConfig(
    public val root: Path,
    public val enabled: List<String>,
    public val stateDirectory: Path? = null,
    /** Relay-provider topics available from this relay. */
    public val topics: List<String> = PluginContract.topics,
)

internal object PluginContract {
    const val SPEC = "oxp-web-plugin/0"
    const val LIMIT = 65536
    val topics = listOf("session", "nowPlaying", "call", "activity", "route", "audioSource")
    private val types = mapOf("html" to "text/html; charset=utf-8", "js" to "text/javascript", "mjs" to "text/javascript", "css" to "text/css", "json" to "application/json", "svg" to "image/svg+xml", "png" to "image/png", "webp" to "image/webp", "jpg" to "image/jpeg", "woff2" to "font/woff2", "wasm" to "application/wasm", "mp3" to "audio/mpeg", "ogg" to "audio/ogg", "wav" to "audio/wav", "txt" to "text/plain")
    fun mime(path: String): String? = types[path.substringAfterLast('.')]
    fun id(value: String): Boolean = value.length <= 64 && Regex("[a-z0-9]+(\\.[a-z0-9-]+)+").matches(value)
    fun safePath(value: String): Boolean = Regex("[a-z0-9._/-]+").matches(value) && value.split('/').all { it.isNotEmpty() && !it.startsWith('.') } && mime(value) != null
    fun file(root: Path, relative: String): Path? = try {
        if (!safePath(relative)) null else {
            val base = root.toRealPath(); val full = base.resolve(relative)
            if (!Files.isRegularFile(full, java.nio.file.LinkOption.NOFOLLOW_LINKS)) null
            else full.toRealPath().takeIf { it.startsWith(base) && Files.isRegularFile(it) }
        }
    } catch (_: Exception) { null }
    fun headers(origin: String, plugin: String?): Map<String, String> {
        val base = "$origin/plugins/${plugin?.let { "p/$it" } ?: "host"}/"
        val guard = "$origin/plugins/host/guard.js"; val artwork = "$origin/plugins/artwork/"
        val channel = "${origin.replaceFirst("http", "ws")}/plugins/channel"
        val images = if (plugin == null) " $artwork $origin/plugins/p/" else ""
        val connect = if (plugin == null) "$channel $artwork" else "'none'"
        val frames = if (plugin == null) "$origin/plugins/p/" else "'none'"
        return linkedMapOf("X-Content-Type-Options" to "nosniff", "Referrer-Policy" to "no-referrer", "Access-Control-Allow-Origin" to "null", "Cross-Origin-Resource-Policy" to "same-origin", "Cache-Control" to "no-cache", "Permissions-Policy" to "camera=(), microphone=(), geolocation=(), usb=(), bluetooth=(), serial=(), hid=(), payment=()",
            "Content-Security-Policy" to "default-src 'none'; script-src $base $guard 'wasm-unsafe-eval'; style-src $base 'unsafe-inline'; img-src $base data: blob:$images; media-src $base data: blob:; font-src $base data:; worker-src $base blob:; connect-src $connect; frame-src $frames; form-action 'none'; base-uri 'none'")
    }
    private fun text(v: JsonElement?, max: Int): Boolean = v is JsonPrimitive && v.isString && v.content.length in 1..max
    private fun localId(v: JsonElement?): Boolean = text(v, 64) && Regex("[a-z0-9][a-z0-9-]*").matches(v!!.jsonPrimitive.content)
    private fun localized(v: JsonElement?): Boolean = v is JsonObject && text(v["en"], 2048) && v.all { (k, value) ->
        text(value, 2048) && try { java.util.Locale.Builder().setLanguageTag(k).build(); true } catch (_: Exception) { false }
    }
    private fun version(v: JsonElement?): Boolean {
        if (!text(v, 256)) return false
        val value = v!!.jsonPrimitive.content
        if (!Regex("(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)(-[0-9A-Za-z-]+(\\.[0-9A-Za-z-]+)*)?(\\+[0-9A-Za-z-]+(\\.[0-9A-Za-z-]+)*)?").matches(value)) return false
        return value.substringBefore('+').substringAfter('-', "").split('.').none { Regex("0[0-9]+").matches(it) }
    }
    private fun license(v: JsonElement?): Boolean {
        if (!text(v, 256) || !Regex("[A-Za-z0-9.+(): -]+").matches(v!!.jsonPrimitive.content)) return false
        val tokens = Regex("[()]|[^ ()]+").findAll(v.jsonPrimitive.content).map { it.value }.toList()
        var position = 0
        fun identifier(): Boolean {
            val token = tokens.getOrNull(position++) ?: return false
            return token !in listOf("AND", "OR", "WITH") && Regex("(?:DocumentRef-[A-Za-z0-9.-]+:)?[A-Za-z0-9][A-Za-z0-9.+-]*").matches(token)
        }
        lateinit var expression: () -> Boolean
        fun atom(): Boolean {
            if (tokens.getOrNull(position) == "(") { position++; return expression() && tokens.getOrNull(position++) == ")" }
            if (!identifier()) return false
            if (tokens.getOrNull(position) == "WITH") { position++; return identifier() }
            return true
        }
        expression = {
            var valid = atom()
            while (valid && tokens.getOrNull(position) in listOf("AND", "OR")) { position++; valid = atom() }
            valid
        }
        return expression() && position == tokens.size
    }
    fun manifest(m: JsonObject): Boolean {
        val permissions = m["permissions"] as? JsonArray ?: return false
        val surfaces = m["surfaces"] as? JsonArray ?: return false
        val actions = m["quickActions"] as? JsonArray ?: return false
        fun path(v: JsonElement?, extensions: List<String>): Boolean = text(v, 256) && v.stringOrNull()?.let { it.first().let { c -> c in 'a'..'z' || c in '0'..'9' } && safePath(it) && it.substringAfterLast('.') in extensions } == true
        val ids = mutableSetOf<String>(); val quick = mutableSetOf<String>()
        return m["spec"] == JsonPrimitive(SPEC) && m["id"].stringOrNull()?.let(::id) == true && version(m["version"]) && license(m["license"]) && localized(m["name"]) && localized(m["description"]) && path(m["icon"], listOf("svg", "png")) &&
            permissions.size <= 7 && permissions.distinct().size == permissions.size && permissions.all { it.stringOrNull() in topics + listOf("nowPlaying.artwork", "storage") } &&
            surfaces.size <= 32 && surfaces.all { value ->
                val surface = value as? JsonObject ?: return@all false
                val kind = surface["kind"].stringOrNull()
                localId(surface["id"]) && ids.add(surface.getValue("id").jsonPrimitive.content) && kind in listOf("companion", "ambient", "panel", "fullscreen") && path(surface["entry"], listOf("html")) &&
                    surface["usage"].stringOrNull() in listOf("driving", "parked") && (kind != "fullscreen" || surface["usage"] == JsonPrimitive("parked")) &&
                    (surface["label"] == null || localized(surface["label"])) && (surface["anchor"] == null || surface["anchor"].stringOrNull() in listOf("top-left", "top-right", "bottom-left", "bottom-right", "left", "right", "top", "bottom")) &&
                    (surface["size"] == null || surface["size"].let { size -> size is JsonObject && listOf("w", "h").all { key -> (size[key] as? JsonPrimitive)?.takeUnless { it.isString }?.doubleOrNull?.let { n -> n >= 1 && n.isFinite() && (kind != "companion" || n <= 160) } == true } })
            } && actions.size <= 32 && actions.all { value ->
                val action = value as? JsonObject ?: return@all false
                localId(action["id"]) && quick.add(action.getValue("id").jsonPrimitive.content) && text(action["icon"], 256) && action["icon"].stringOrNull()?.let { Regex("[a-z0-9][a-z0-9._/-]*").matches(it) } == true && localized(action["label"]) &&
                    (action["opens"] == null || localId(action["opens"]) && action["opens"].stringOrNull() in ids)
            }
    }
    fun topic(topic: String, value: JsonElement): Boolean {
        if (value == JsonNull) return true
        val v = value as? JsonObject ?: return false
        return when (topic) {
            "route", "audioSource" -> runCatching { ContextJson.decode(buildJsonObject { put("topic", topic); put("value", value) }) }.isSuccess
            "session" -> v["carplay"].stringOrNull() in listOf("connected", "disconnected")
            "call" -> v["state"].stringOrNull() in listOf("idle", "ringing", "dialing", "active", "held", "ending")
            "activity" -> listOf("siri", "phoneCall", "navigation").all { (v[it] as? JsonPrimitive)?.takeUnless { it.isString }?.booleanOrNull != null }
            "nowPlaying" -> listOf("title", "artist", "album").all { v[it].stringOrNull() != null } && listOf("durationMs", "elapsedMs", "rate").all { (v[it] as? JsonPrimitive)?.takeUnless { it.isString }?.doubleOrNull?.let { n -> n.isFinite() && n >= 0 } == true } &&
                (v["playing"] as? JsonPrimitive)?.takeUnless { it.isString }?.booleanOrNull != null && (v["artwork"] == null || v["artwork"].let { a -> a is JsonObject && a["url"].stringOrNull()?.let { Regex("/plugins/artwork/[a-f0-9]{64}").matches(it) } == true && a["type"].stringOrNull() in listOf("image/png", "image/jpeg", "image/webp") })
            else -> false
        }
    }
    fun channel(m: JsonObject): Boolean {
        if (!text(m["type"], Int.MAX_VALUE)) return false
        return when (m["type"].stringOrNull()) {
            "hello" -> m["spec"] == JsonPrimitive(SPEC) && text(m["host"], 256)
            "subscribe" -> m["topics"].let { it is JsonArray && it.all { t -> t.stringOrNull() != null } }
            "storage-get", "storage-set" -> text(m["id"], 256) && m["plugin"].stringOrNull()?.let(::id) == true && (m["type"] != JsonPrimitive("storage-set") || m["value"] is JsonObject)
            "welcome" -> m["spec"] == JsonPrimitive(SPEC) && m["plugins"].let { it is JsonArray && it.all { p -> p is JsonObject && manifest(p) } } && m["topics"].let { it is JsonArray && it.all { t -> t.stringOrNull() in topics } } && m["storage"].let { it is JsonObject && it["quotaBytes"] == JsonPrimitive(LIMIT) && (it["persistent"] as? JsonPrimitive)?.takeUnless { p -> p.isString }?.booleanOrNull != null }
            "state" -> m["topic"].stringOrNull()?.let { t -> t in topics && m["value"]?.let { topic(t, it) } == true } == true && (m["seq"] as? JsonPrimitive)?.takeUnless { it.isString }?.longOrNull?.let { it in 0..9007199254740991L } == true
            "storage" -> text(m["id"], 256) && (m["value"] is JsonObject || m["value"] == JsonNull)
            "storage-ok" -> text(m["id"], 256)
            "error" -> m["code"].stringOrNull() in listOf("quota", "rate", "unknown-plugin", "permission", "protocol") && (m["id"] == null || text(m["id"], 256))
            else -> true
        }
    }
    fun serve(plugins: Plugins?, r: HttpRequest, origin: String, out: OutputStream) {
        val path = r.target.substringBefore('?'); val plugin = Regex("^/plugins/p/([a-z0-9.-]+)/").find(path)?.groupValues?.get(1)
        var body = byteArrayOf(); var status = if (plugins != null && path == "/plugins/channel") 503 else 404; var type = "text/plain"
        if (plugins != null && r.method in listOf("GET", "HEAD")) {
            if (path == "/plugins/index.json") { body = plugins.index().toString().toByteArray(Charsets.UTF_8); status = 200; type = "application/json" }
            else {
                val relative = when {
                    path.startsWith("/plugins/host/") -> path.removePrefix("/plugins/")
                    plugin != null && plugin in plugins.manifests -> path.removePrefix("/plugins/")
                    else -> null
                }
                val file = relative?.let { file(plugins.config.root, it) }
                if (file != null) try { body = Files.readAllBytes(file); status = 200; type = mime(relative!!)!! } catch (_: Exception) { }
            }
        }
        val header = "HTTP/1.1 $status ${when (status) { 200 -> "OK"; 503 -> "Service Unavailable"; else -> "Not Found" }}\r\nContent-Type: $type\r\nContent-Length: ${body.size}\r\nConnection: close\r\n" + headers(origin, plugin).entries.joinToString("") { "${it.key}: ${it.value}\r\n" } + "\r\n"
        out.write(header.toByteArray(Charsets.US_ASCII)); if (r.method != "HEAD") out.write(body); out.flush()
    }
}
private fun JsonElement?.stringOrNull(): String? = (this as? JsonPrimitive)?.takeIf { it.isString }?.content

internal data class ChannelTiming(val pingMillis: Long = 10_000, val idleMillis: Long = 30_000, val writeMillis: Long = 2_000) {
    init { require(pingMillis > 0 && idleMillis > 0 && idleMillis <= Int.MAX_VALUE && writeMillis > 0) }
}

/** No media lock is taken here. Each socket owns its reader and scheduled writer. */
internal class Plugins(val config: PluginsConfig, private val timing: ChannelTiming = ChannelTiming()) {
    val manifests = linkedMapOf<String, JsonObject>()
    private val store = mutableMapOf<String, JsonObject>()
    private val writes = mutableMapOf<String, Long>()
    private val pendingWrites = mutableSetOf<String>()
    private val storageLock = Any()
    private val contextLock = Any()
    private var seq = 0L
    private val availableTopics = config.topics.distinct().also { require(it.all { t -> t in PluginContract.topics }) { "Plugin topics must be registry relay topics" } }
    private val latest = availableTopics.associateWithTo(linkedMapOf()) { 0L to (JsonNull as JsonElement) }
    private val connections = ArrayDeque<ChannelPermit>()
    init {
        config.enabled.distinct().filter(PluginContract::id).forEach { id ->
            try {
                val path = PluginContract.file(config.root, "p/$id/oxp-plugin.json") ?: return@forEach
                val m = Json.parseToJsonElement(Files.readString(path)).jsonObject
                if (m["id"] != JsonPrimitive(id) || !PluginContract.manifest(m)) return@forEach
                manifests[id] = m
                if (config.stateDirectory != null && permission(m)) try {
                    val value = Json.parseToJsonElement(Files.readString(config.stateDirectory.resolve("plugins/storage/$id.json")))
                    if (value is JsonObject && value.toString().toByteArray(Charsets.UTF_8).size <= PluginContract.LIMIT) store[id] = value
                } catch (_: Exception) { }
            } catch (_: Exception) { }
        }
        require(welcome().toString().toByteArray(Charsets.UTF_8).size <= PluginContract.LIMIT) { "Plugin catalog exceeds channel cap" }
    }
    private fun permission(m: JsonObject): Boolean = JsonPrimitive("storage") in m.getValue("permissions").jsonArray
    fun index(): JsonObject = buildJsonObject { put("spec", PluginContract.SPEC); put("plugins", buildJsonArray { manifests.forEach { (id, m) -> add(buildJsonObject { put("id", id); put("version", m.getValue("version")); put("manifest", "/plugins/p/$id/oxp-plugin.json") }) } }) }
    fun welcome(): JsonObject = buildJsonObject { put("type", "welcome"); put("spec", PluginContract.SPEC); put("plugins", JsonArray(manifests.values.toList())); put("topics", JsonArray(availableTopics.map(::JsonPrimitive))); put("storage", buildJsonObject { put("quotaBytes", PluginContract.LIMIT); put("persistent", config.stateDirectory != null) }) }
    fun update(update: ContextUpdate) {
        if (update.topic.wireName !in availableTopics) return
        val m = ContextJson.encode(update)
        synchronized(contextLock) {
            val value = m.getValue("value").let { if (PluginContract.topic(update.topic.wireName, it)) it else JsonNull }
            val next = seq + 1
            val message = buildJsonObject { put("type", "state"); put("topic", update.topic.wireName); put("value", value); put("seq", next) }
            if (message.toString().toByteArray(Charsets.UTF_8).size <= PluginContract.LIMIT) { seq = next; latest[update.topic.wireName] = next to value }
        }
    }
    private fun storage(m: JsonObject): JsonObject {
        val plugin = m.getValue("plugin").jsonPrimitive.content
        fun error(code: String): JsonObject = buildJsonObject { put("type", "error"); put("id", m.getValue("id")); put("code", code) }
        val manifest = manifests[plugin] ?: return error("unknown-plugin")
        if (!permission(manifest)) return error("permission")
        if (m["type"] == JsonPrimitive("storage-get")) return synchronized(storageLock) {
            val reply = buildJsonObject { put("type", "storage"); put("id", m.getValue("id")); put("value", store[plugin] ?: JsonNull) }
            if (reply.toString().toByteArray(Charsets.UTF_8).size <= PluginContract.LIMIT) reply else error("quota")
        }
        val value = m.getValue("value").jsonObject; val bytes = value.toString().toByteArray(Charsets.UTF_8)
        if (bytes.size > PluginContract.LIMIT) return error("quota")
        synchronized(storageLock) {
            val now = System.nanoTime()
            if (plugin in pendingWrites || writes[plugin]?.let { now - it < TimeUnit.SECONDS.toNanos(2) } == true) return error("rate")
            pendingWrites.add(plugin)
        }
        // No shared lock is held during disk I/O, so other channel connections keep progressing.
        val persisted = try {
            if (config.stateDirectory != null) {
                val directory = config.stateDirectory.resolve("plugins/storage"); Files.createDirectories(directory)
                val tmp = Files.createTempFile(directory, "$plugin-", ".tmp")
                try { Files.write(tmp, bytes); Files.move(tmp, directory.resolve("$plugin.json"), StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING) }
                finally { Files.deleteIfExists(tmp) }
            }
            true
        } catch (_: Exception) { false }
        synchronized(storageLock) {
            pendingWrites.remove(plugin)
            if (!persisted) return error("protocol")
            writes[plugin] = System.nanoTime(); store[plugin] = value
        }
        return buildJsonObject { put("type", "storage-ok"); put("id", m.getValue("id")) }
    }

    internal inner class ChannelPermit : AutoCloseable {
        val replaced = AtomicBoolean()
        override fun close() { synchronized(connections) { connections.remove(this) } }
    }
    fun reserve(): ChannelPermit = synchronized(connections) {
        if (connections.size == 4) connections.removeFirst().replaced.set(true)
        ChannelPermit().also { connections.addLast(it) }
    }
    internal inner class Channel {
        private var ready = false
        private var topics = linkedSetOf<String>()
        private val pending = mutableMapOf<String, Long>()
        private val sent = mutableMapOf<String, Pair<Long, Long>>()
        @Synchronized fun receive(raw: String): List<JsonObject> {
            fun error(id: JsonElement? = null): List<JsonObject> = listOf(buildJsonObject { put("type", "error"); put("code", "protocol"); if (id is JsonPrimitive && id.isString && id.content.length in 1..256) put("id", id) })
            val m = try { Json.parseToJsonElement(raw).jsonObject } catch (_: Exception) { return error() }
            if (raw.toByteArray(Charsets.UTF_8).size > PluginContract.LIMIT || !PluginContract.channel(m)) return error()
            val type = m.getValue("type").jsonPrimitive.content
            if (type !in listOf("hello", "subscribe", "storage-get", "storage-set", "welcome", "state", "storage", "storage-ok", "error")) return emptyList()
            if (type == "hello" && !ready) { ready = true; return listOf(welcome()) }
            if (!ready) return error(m["id"])
            return when (type) {
                "subscribe" -> { topics = m.getValue("topics").jsonArray.map { it.jsonPrimitive.content }.filterTo(linkedSetOf()) { it in PluginContract.topics }; pending.clear(); val at = synchronized(contextLock) { seq }; topics.forEach { pending[it] = at }; flush() }
                "storage-get", "storage-set" -> listOf(storage(m))
                else -> error(m["id"])
            }
        }
        @Synchronized fun flush(now: Long = System.nanoTime()): List<JsonObject> = synchronized(contextLock) {
            topics.mapNotNull { topic ->
                val (topicSequence, value) = latest[topic] ?: return@mapNotNull null
                val sequence = maxOf(topicSequence, pending[topic] ?: 0L)
                val previous = sent[topic]
                if (previous != null && (previous.first >= sequence && topic !in pending || now - previous.second < TimeUnit.MILLISECONDS.toNanos(250))) null
                else { sent[topic] = sequence to now; pending.remove(topic); buildJsonObject { put("type", "state"); put("topic", topic); put("value", value); put("seq", sequence) } }
            }
        }
    }
    fun channel(socket: Socket, permit: ChannelPermit, timers: ScheduledExecutorService, active: () -> Boolean) {
        val channel = Channel(); val writeLock = Any(); val output = socket.getOutputStream()
        val writer = Executors.newSingleThreadScheduledExecutor { r -> Thread(r, "web-plugin-writer").apply { isDaemon = true } }
        val received = AtomicLong(System.nanoTime())
        val writing = AtomicLong()
        fun close() { try { socket.close() } catch (_: Exception) { } }
        fun write(opcode: Int, payload: ByteArray) { synchronized(writeLock) {
            writing.set(System.nanoTime())
            try { writeWebSocket(output, opcode, payload) } finally { writing.set(0) }
        } }
        fun send(m: JsonObject) { write(1, m.toString().toByteArray(Charsets.UTF_8)) }
        // Runs on the runtime timer, so even a blocked channel writer is interrupted by close().
        val watchdog = timers.scheduleWithFixedDelay({
            val now = System.nanoTime(); val start = writing.get()
            if (!active() || now - received.get() >= TimeUnit.MILLISECONDS.toNanos(timing.idleMillis) ||
                start != 0L && now - start >= TimeUnit.MILLISECONDS.toNanos(timing.writeMillis)) close()
        }, 10, 10, TimeUnit.MILLISECONDS)
        var ping = System.nanoTime()
        writer.scheduleWithFixedDelay({
            try {
                if (!active() || socket.isClosed) return@scheduleWithFixedDelay
                if (permit.replaced.get()) {
                    write(8, closePayload(1001) + "replaced".toByteArray(Charsets.UTF_8)); close()
                    return@scheduleWithFixedDelay
                }
                channel.flush().forEach(::send)
                if (System.nanoTime() - ping >= TimeUnit.MILLISECONDS.toNanos(timing.pingMillis)) {
                    write(9, byteArrayOf()); ping = System.nanoTime()
                }
            } catch (_: Exception) { close() }
        }, 10, 10, TimeUnit.MILLISECONDS)
        try {
            socket.soTimeout = timing.idleMillis.toInt()
            val reader = WebSocketReader(socket.getInputStream(), PluginContract.LIMIT) { received.set(System.nanoTime()) }
            while (active() && !socket.isClosed) {
                val m = reader.read()
                when (m.opcode) {
                    1 -> channel.receive(utf8(m.payload)).forEach(::send)
                    8 -> { write(8, m.payload); return }
                    9 -> write(10, m.payload)
                    2 -> send(buildJsonObject { put("type", "error"); put("code", "protocol") })
                }
            }
        } finally { watchdog.cancel(false); writer.shutdownNow(); close() }
    }

}

internal fun receiverWelcome(clientId: String, state: org.openxplay.stream.core.SessionState, sourceUrl: String, plugins: Boolean = false): JsonObject = buildJsonObject {
    put("type", "welcome"); put("version", 0); put("clientId", clientId)
    put("session", WebJsonCodec.session(state)); put("sourceUrl", sourceUrl)
    put("attribution", "Powered by OpenXPlay."); put("projectUrl", "https://github.com/OpenXPlay/OpenXPlay-Stream")
    if (plugins) put("plugins", buildJsonObject { put("host", "/plugins/host/index.html") })
}
