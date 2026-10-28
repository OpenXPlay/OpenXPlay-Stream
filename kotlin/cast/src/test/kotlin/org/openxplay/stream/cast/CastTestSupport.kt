// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-22

package org.openxplay.stream.cast

import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.Inet4Address
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.net.URI
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.ThreadFactory
import java.util.concurrent.atomic.AtomicBoolean
import org.openxplay.stream.core.*
import kotlin.test.assertTrue

internal val LOOPBACK: Inet4Address = InetAddress.getByName("127.0.0.1") as Inet4Address
internal val LOOPBACK_NETWORK = object : CastNetwork { override fun localAddresses(): List<Inet4Address> = listOf(LOOPBACK) }
internal fun vector(name: String): ByteArray = checkNotNull(object {}.javaClass.getResource("/vectors/$name.hex"))
    .readText().trim().split(Regex("\\s+")).map { it.toInt(16).toByte() }.toByteArray()
internal fun videoFormat(): VideoFormat = VideoFormat.create(ScreenId.MAIN, VideoCodec.H264, vector("avc-baseline-640x480"))
internal fun frame(key: Boolean = true, received: Long = System.nanoTime(), session: Long = 1): VideoFrame =
    VideoFrame(ScreenId.MAIN, SessionId(session), byteArrayOf(0, 0, 0, 2, (if (key) 0x65 else 0x41).toByte(), 0), 0, key, received)
internal val SESSION_CONFIG = SessionConfig(mapOf(ScreenId.MAIN to ScreenConfig(VideoCodec.H264, 640, 480, 30.0)))
internal val NO_HOST = object : OutputHost {
    override fun capabilities(capabilities: OutputCapabilities): Unit = Unit
    override fun control(event: ControlEvent): Unit = Unit
    override fun stats(stats: OutputStats): Unit = Unit
}
internal fun await(message: String, timeoutMs: Long = 4000, test: () -> Boolean) {
    val deadline = System.nanoTime() + timeoutMs * 1_000_000
    while (!test() && System.nanoTime() < deadline) Thread.sleep(5)
    assertTrue(test(), message)
}
internal class TrackedThreads : ThreadFactory {
    val threads = CopyOnWriteArrayList<Thread>()
    override fun newThread(r: Runnable): Thread = Thread(r).apply { isDaemon = true; threads += this }
    fun awaitEnded() { await("Workers still alive: ${threads.filter { it.isAlive }.map { it.name }}") { threads.none { it.isAlive } } }
}
internal fun description(device: String, base: String = ""): String = "<?xml version=\"1.0\"?><root xmlns=\"urn:schemas-upnp-org:device-1-0\">$base$device</root>"
internal fun device(udn: String = "uuid:test", services: String = service(), embedded: String = "", name: String = "Test &amp; Renderer"): String =
    "<device><deviceType>${RENDERER_TYPE}1</deviceType><UDN>$udn</UDN><friendlyName>$name</friendlyName><manufacturer>OXP</manufacturer>" +
        "<modelName>Loopback</modelName><serviceList>$services</serviceList>$embedded</device>"
internal fun service(type: String = "${AVTRANSPORT_TYPE}1", control: String = "/control"): String =
    "<service><serviceType>$type</serviceType><serviceId>urn:upnp-org:serviceId:AVTransport</serviceId><controlURL>$control</controlURL></service>"
internal fun soapResponse(action: String, values: Map<String, String> = emptyMap()): String =
    "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body><u:${action}Response xmlns:u=\"${AVTRANSPORT_TYPE}1\">" +
        values.entries.joinToString("") { (k, v) -> "<$k>${xmlEscape(v)}</$k>" } + "</u:${action}Response></s:Body></s:Envelope>"
internal fun fault(code: Int = 714): String = "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body><s:Fault><detail>" +
    "<UPnPError xmlns=\"urn:schemas-upnp-org:control-1-0\"><errorCode>$code</errorCode><errorDescription>Test rejected</errorDescription></UPnPError>" +
    "</detail></s:Fault></s:Body></s:Envelope>"
internal fun respond(socket: Socket, body: String, status: Int = 200) {
    val bytes = body.toByteArray(Charsets.UTF_8)
    socket.getOutputStream().apply {
        write("HTTP/1.1 $status ${if (status == 200) "OK" else "Internal Server Error"}\r\nContent-Length: ${bytes.size}\r\nConnection: close\r\n\r\n".toByteArray())
        write(bytes); flush()
    }
}

/** Public-protocol fixture; discovery, control and the renderer's player use loopback sockets. */
internal class FakeRenderer(private val transport: Boolean = true) : AutoCloseable {
    private val active = AtomicBoolean(true)
    private val tcp = ServerSocket(0, 32, LOOPBACK)
    private val udp = DatagramSocket(InetSocketAddress(LOOPBACK, 0))
    private val sockets = ConcurrentHashMap.newKeySet<Socket>()
    private val players = ConcurrentHashMap.newKeySet<Socket>()
    val threads = TrackedThreads()
    val actions = CopyOnWriteArrayList<String>()
    val uris = CopyOnWriteArrayList<String>()
    val metadata = CopyOnWriteArrayList<String>()
    val errors = CopyOnWriteArrayList<Throwable>()
    val packets = CopyOnWriteArrayList<ByteArray>()
    val streams = ConcurrentHashMap<String, CopyOnWriteArrayList<ByteArray>>()
    val rejections = ConcurrentHashMap<String, Int>()
    @Volatile var transportState = "STOPPED"
    @Volatile var unavailable = false
    @Volatile var sink = "http-get:*:video/mp2t:*"
    val location: String = "http://127.0.0.1:${tcp.localPort}/description.xml"
    val options = CastOptions(discovery = DiscoveryOptions(InetSocketAddress(LOOPBACK, udp.localPort), 10), searchMs = 70,
        searchBackoffMs = listOf(10), stoppedBackoffMs = listOf(30), retryMs = 30, pollMs = 30, playRetryMs = 20,
        playDeadlineMs = 1000, stablePlayingMs = 100, bindRetryMs = 20)
    init {
        start("fixture-ssdp") {
            while (active.get()) {
                val packet = DatagramPacket(ByteArray(4096), 4096); udp.receive(packet)
                val reply = "HTTP/1.1 200 OK\r\nLOCATION: $location\r\nUSN: uuid:test::${RENDERER_TYPE}1\r\nST: ${RENDERER_TYPE}1\r\n\r\n".toByteArray()
                udp.send(DatagramPacket(reply, reply.size, packet.socketAddress))
            }
        }
        start("fixture-http") { while (active.get()) { val socket = tcp.accept(); sockets += socket; start("fixture-request") { handle(socket) } } }
    }
    private fun start(name: String, block: () -> Unit) {
        threads.newThread {
            try { block() } catch (error: Exception) { if (active.get() && error !is java.io.IOException) errors += error }
        }.apply { this.name = name; start() }
    }
    private fun handle(socket: Socket) {
        try {
            socket.use {
                val input = HttpInput(socket, 2000)
                val head = input.head()
                if (head.start.startsWith("GET /description.xml ")) {
                    respond(socket, description(device(services = if (transport) service() + service("${CONNECTION_TYPE}1", "/connection") else ""))); return
                }
                if (unavailable) return
                val xml = String(input.body(head.headers, 65536), Charsets.UTF_8)
                val body = XmlReader(xml).read().child("Body")!!.children.single()
                val action = body.name; actions += action
                val remaining = rejections[action] ?: 0
                if (remaining > 0) {
                    rejections[action] = remaining - 1; respond(socket, fault(if (action == "Play") 701 else 714), 500); return
                }
                val values = when (action) {
                    "Stop" -> { transportState = "STOPPED"; players.forEach { runCatching { it.close() } }; emptyMap() }
                    "SetAVTransportURI" -> { uris += body.value("CurrentURI"); metadata += body.value("CurrentURIMetaData"); emptyMap() }
                    "Play" -> { transportState = "PLAYING"; startPlayer(uris.last()); emptyMap() }
                    "GetTransportInfo" -> mapOf("CurrentTransportState" to transportState)
                    "GetProtocolInfo" -> mapOf("Sink" to sink)
                    else -> error("Unexpected action $action")
                }
                respond(socket, soapResponse(action, values))
            }
        } finally { sockets.remove(socket) }
    }
    private fun startPlayer(url: String) {
        start("fixture-player") {
            val stream = streams.computeIfAbsent(url) { CopyOnWriteArrayList() }
            val uri = URI(url); val socket = Socket(); sockets += socket; players += socket
            try {
                socket.use {
                    socket.connect(InetSocketAddress(uri.host, uri.port), 1000)
                    socket.getOutputStream().apply { write("GET ${uri.rawPath} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n".toByteArray()); flush() }
                    val input = HttpInput(socket, 2000, 10_000)
                    check(input.head().start == "HTTP/1.1 200 OK")
                    while (active.get()) {
                        val packet = input.exact(188); packets += packet; stream += packet
                    }
                }
            } finally { sockets.remove(socket); players.remove(socket) }
        }
    }
    fun disconnectPlayers() { players.forEach { runCatching { it.close() } } }
    fun output(listener: CastListener = object : CastListener {}, threads: ThreadFactory = CAST_THREADS,
        audio: Boolean = true, network: CastNetwork = LOOPBACK_NETWORK, options: CastOptions = this.options): CastOutput =
        CastOutput(CastOutputConfig("uuid:test", audio = audio), network, listener, threads, "cast", options)
    override fun close() {
        active.set(false); udp.close(); tcp.close(); sockets.forEach { runCatching { it.close() } }; threads.awaitEnded()
    }
}
