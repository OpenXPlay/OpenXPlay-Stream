// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-22

package org.openxplay.stream.cast

import java.net.Inet4Address
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.NetworkInterface
import java.net.StandardProtocolFamily
import java.net.StandardSocketOptions
import java.net.URI
import java.nio.ByteBuffer
import java.nio.channels.ClosedSelectorException
import java.nio.channels.DatagramChannel
import java.nio.channels.SelectionKey
import java.nio.channels.Selector
import java.util.concurrent.Callable
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

internal const val RENDERER_TYPE = "urn:schemas-upnp-org:device:MediaRenderer:"
internal const val AVTRANSPORT_TYPE = "urn:schemas-upnp-org:service:AVTransport:"
internal const val CONNECTION_TYPE = "urn:schemas-upnp-org:service:ConnectionManager:"
internal data class CastService(val type: String, val control: URI)
internal data class DiscoveredRenderer(val renderer: CastRenderer, val transport: CastService, val connection: CastService?)
internal data class DiscoveryResult(val renderers: List<DiscoveredRenderer>, val withoutTransport: Set<String>)
internal data class DiscoveryOptions(
    val target: InetSocketAddress = InetSocketAddress("239.255.255.250", 1900),
    val repeatMs: Long = 500,
    val descriptionMs: Int = 2000,
)
internal data class SsdpReply(val location: String, val usn: String, val st: String)
internal object Ssdp {
    private val responseStatus = Regex("HTTP/1\\.[01][ \\t]+200(?:[ \\t].*)?")
    fun search(target: InetSocketAddress, st: String): ByteArray = ("M-SEARCH * HTTP/1.1\r\n" +
        "HOST: ${target.address.hostAddress}:${target.port}\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\n" +
        "USER-AGENT: $USER_AGENT\r\nST: $st\r\n\r\n").toByteArray(Charsets.US_ASCII)
    fun reply(bytes: ByteArray): SsdpReply? = runCatching {
        val wire = String(bytes, Charsets.ISO_8859_1).replace("\r\n", "\n")
        require(wire.contains("\n\n"))
        val lines = wire.split('\n')
        require(responseStatus.matches(lines.first()))
        val headers = mutableMapOf<String, String>()
        for (line in lines.drop(1)) {
            if (line.isEmpty()) break
            val colon = line.indexOf(':'); require(colon > 0)
            val key = line.substring(0, colon).lowercase(java.util.Locale.ROOT)
            require(key.all { it in 'a'..'z' || it in '0'..'9' || it in "!#$%&'*+-.^_`|~" } && line.all { it.code >= 32 || it == '\t' })
            if (key !in headers) headers[key] = line.substring(colon + 1).trim()
        }
        val location = headers["location"]!!; httpUri(URI(location))
        val usn = headers["usn"]!!; val st = headers["st"]!!
        require(usn.isNotBlank() && st.isNotBlank()); SsdpReply(location, usn, st)
    }.getOrNull()
}

internal fun parseDescription(location: URI, xml: String): DiscoveryResult {
    httpUri(location)
    val root = XmlReader(xml).read()
    val base = root.value("URLBase").takeIf { it.isNotBlank() }?.let { httpUri(URI(it)) } ?: location
    require(base.host.equals(location.host, true)) { "Foreign URLBase host" }
    val address = InetAddress.getByName(location.host)
    require(address is Inet4Address) { "Renderer requires IPv4" }
    val found = mutableListOf<DiscoveredRenderer>(); val missing = mutableSetOf<String>()
    fun visit(device: XmlNode) {
        if (device.value("deviceType").startsWith(RENDERER_TYPE)) {
            val udn = device.value("UDN")
            val services = device.child("serviceList")?.children.orEmpty().filter { it.name == "service" }
            fun service(prefix: String): CastService? = services.firstOrNull { it.value("serviceType").startsWith(prefix) }?.let {
                val control = it.value("controlURL"); require(control.isNotBlank()) { "Missing control URL" }
                val uri = httpUri(base.resolve(control))
                require(uri.host.equals(location.host, true)) { "Foreign control host" }
                CastService(it.value("serviceType"), uri)
            }
            if (udn.isNotBlank()) {
                val transport = service(AVTRANSPORT_TYPE)
                if (transport == null) missing += udn
                else found += DiscoveredRenderer(CastRenderer(udn, device.value("friendlyName"), device.value("manufacturer"),
                    device.value("modelName"), address.hostAddress, location.toASCIIString()), transport, service(CONNECTION_TYPE))
            }
        }
        device.child("deviceList")?.children?.filter { it.name == "device" }?.forEach(::visit)
    }
    if (root.name == "device") visit(root) else root.children.filter { it.name == "device" }.forEach(::visit)
    return DiscoveryResult(found, missing)
}

public class CastDiscovery internal constructor(
    private val network: CastNetwork,
    private val options: DiscoveryOptions,
    private val scope: SocketScope,
    private val active: () -> Boolean,
) {
    public constructor(network: CastNetwork = CastNetwork.SYSTEM) : this(network, DiscoveryOptions(), SocketScope(), { true })

    /** Blocking search plus parallel description fetches; never call on a UI or media thread. */
    public fun search(timeoutMs: Long = 3_000): List<CastRenderer> = scan(timeoutMs).renderers.map { it.renderer }

    internal fun scan(timeoutMs: Long): DiscoveryResult {
        require(timeoutMs >= 0)
        network.multicastLock().use {
            val selector = scope.add(Selector.open())
            val channels = mutableListOf<DatagramChannel>()
            val locations = linkedSetOf<String>()
            try {
                for (address in network.localAddresses().distinct()) {
                    if (!active()) break
                    var channel: DatagramChannel? = null
                    try {
                        // Dual-stack datagram sockets can miss IPv4 multicast replies.
                        channel = scope.add(DatagramChannel.open(StandardProtocolFamily.INET))
                        channel.setOption(StandardSocketOptions.IP_MULTICAST_IF, checkNotNull(NetworkInterface.getByInetAddress(address)))
                        channel.setOption(StandardSocketOptions.IP_MULTICAST_TTL, 2)
                        channel.bind(InetSocketAddress(address, 0)); channel.configureBlocking(false)
                        channel.register(selector, SelectionKey.OP_READ); channels += channel
                    } catch (_: Exception) {
                        channel?.let { runCatching { it.close() }; scope.remove(it) }
                    }
                }
                val start = System.nanoTime(); var round = 0
                val bytes = ByteBuffer.allocate(16 * 1024)
                while (active() && selector.isOpen && (System.nanoTime() - start) / 1_000_000 < timeoutMs) {
                    val elapsed = (System.nanoTime() - start) / 1_000_000
                    if (round < 3 && elapsed >= round * options.repeatMs) {
                        for (channel in channels) {
                            if (!active()) break
                            listOf("${RENDERER_TYPE}1", "${AVTRANSPORT_TYPE}1").forEach { st ->
                                val request = Ssdp.search(options.target, st)
                                if (active()) runCatching { channel.send(ByteBuffer.wrap(request), options.target) }
                            }
                        }
                        round++
                    }
                    val remaining = timeoutMs - (System.nanoTime() - start) / 1_000_000
                    if (remaining <= 0 || !active()) break
                    val nextRound = if (round < 3) round * options.repeatMs - (System.nanoTime() - start) / 1_000_000 else remaining
                    if (nextRound <= 0) continue
                    try {
                        // Bound the wait so an active() change also works without closing the scope.
                        selector.select(minOf(remaining, nextRound, 100).coerceAtLeast(1))
                        if (!active() || !selector.isOpen) break
                        val selected = selector.selectedKeys()
                        val ready = synchronized(selected) { selected.toList().also { selected.clear() } }
                        for (key in ready) {
                            if (!active() || (System.nanoTime() - start) / 1_000_000 >= timeoutMs) break
                            if (!key.isValid) continue
                            val channel = key.channel() as DatagramChannel
                            try {
                                bytes.clear()
                                if (channel.receive(bytes) != null) {
                                    val reply = Ssdp.reply(bytes.array().copyOf(bytes.position()))
                                    if (reply != null && locations.size < 256) locations += reply.location
                                }
                            } catch (_: java.io.IOException) {
                                key.cancel(); runCatching { channel.close() }; scope.remove(channel); channels.remove(channel)
                            }
                        }
                    } catch (_: ClosedSelectorException) { break }
                }
            } finally {
                channels.forEach { runCatching { it.close() }; scope.remove(it) }
                try { selector.close() } finally { scope.remove(selector) }
            }
            if (!active() || locations.isEmpty()) return DiscoveryResult(emptyList(), emptySet())
            val descriptions = scope.add(SocketScope())
            val deadline = System.nanoTime() + options.descriptionMs * 1_000_000L
            val executor = Executors.newFixedThreadPool(minOf(8, locations.size)) { task ->
                Thread(task, "cast-description").apply { isDaemon = true }
            }
            try {
                val futures = locations.map { location -> executor.submit(Callable {
                    val remaining = ((deadline - System.nanoTime()) / 1_000_000).toInt()
                    if (!active() || remaining <= 0) return@Callable null
                    runCatching {
                        val uri = URI(location)
                        val response = CastHttpClient(network, descriptions).request(uri, limit = 256 * 1024,
                            connectMs = options.descriptionMs, readMs = options.descriptionMs, budgetMs = remaining)
                        require(response.status == 200)
                        parseDescription(uri, String(response.body, Charsets.UTF_8))
                    }.getOrNull()
                }) }
                val results = futures.mapNotNull { future -> runCatching {
                    val remaining = deadline - System.nanoTime()
                    if (future.isDone) future.get() else if (remaining > 0) future.get(remaining, TimeUnit.NANOSECONDS) else null
                }.getOrNull() }
                return DiscoveryResult(results.flatMap { it.renderers }.distinctBy { it.renderer.udn }
                    .sortedBy { it.renderer.friendlyName }, results.flatMap { it.withoutTransport }.toSet())
            } finally {
                descriptions.close(); scope.remove(descriptions); executor.shutdownNow()
                executor.awaitTermination(100, TimeUnit.MILLISECONDS)
            }
        }
    }
}
