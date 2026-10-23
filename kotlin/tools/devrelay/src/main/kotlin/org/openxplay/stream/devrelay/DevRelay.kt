// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.devrelay

import java.io.PrintStream
import java.net.URI
import java.nio.file.Files
import java.security.KeyStore
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicBoolean
import javax.net.ssl.KeyManagerFactory
import javax.net.ssl.SSLContext
import org.openxplay.stream.cast.*
import org.openxplay.stream.clink.*
import org.openxplay.stream.clink.protocol.AuthIdentity
import org.openxplay.stream.core.*
import org.openxplay.stream.replay.AnnexBFileInput
import org.openxplay.stream.web.*

/** Owns component lifetimes; all component interaction uses the core interfaces. */
class DevRelay private constructor(private val options: RelayOptions, stdout: PrintStream) : AutoCloseable {
    private val log = EventLog(stdout)
    private val closed = AtomicBoolean(false)
    private val replay = AnnexBFileInput(requireNotNull(options.clip) { "Replay requires --clip" }, options.fps, audio = options.audio, micSummary = log::microphoneStats)
    private val input: ProjectionInput = LoggedInput(replay, log)
    private val hub = ProjectionHub(input, clock = MonotonicClock.system, listener = object : HubListener {
        override fun outputFailed(outputId: String, failure: Throwable) {
            log.event("output-failed", "output" to outputId, "message" to failure.message)
            log.error(outputId, failure)
        }
        override fun stats(outputId: String, stats: OutputStats) = log.stats(outputId, stats)
        override fun inputFailed(failure: Throwable) = log.error("input", failure)
    })
    var webPort: Int? = null
        private set
    var receiverUrl: String? = null
        private set

    private fun startComponents() {
        hub.start()
        options.webPort?.let { port ->
            val web = WebOutput(WebOutputConfig(
                bindAddress = options.webBind, port = port, sslContext = sslContext(options),
                staticAssetRoot = options.webRoot,
                sourceUrl = options.sourceUrl,
            ))
            hub.addOutput(LoggedOutput(web, log, reportOpened = false))
            try {
                webPort = web.awaitListening(3000)
                // A wildcard binding is reached locally through loopback.
                val address = when (options.webBind) {
                    "0.0.0.0" -> "127.0.0.1"
                    "::", "[::]" -> "::1"
                    else -> options.webBind
                }
                receiverUrl = URI(if (options.tlsKeystore == null) "http" else "https", null,
                    address, webPort!!, "/", null, null).toASCIIString()
                log.event("output-opened", "output" to web.id, "port" to webPort)
                log.event("receiver", "url" to receiverUrl)
            } catch (failure: Exception) {
                log.event("output-failed", "output" to web.id, "message" to failure.message)
                log.error(web.id, failure)
                throw failure
            }
        }
        options.castRenderer?.let { udn ->
            val cast = CastOutput(CastOutputConfig(udn, fps = if (options.fps >= 60) 60 else 30),
                listener = object : CastListener {
                    override fun state(state: CastState, renderer: CastRenderer?) = log.event("cast-state",
                        "state" to state, "renderer" to renderer?.friendlyName)
                    override fun failure(failure: CastFailure) = log.event("cast-failure", "code" to failure.code, "detail" to failure.detail)
                    override fun diagnostic(message: String) = log.event("cast-diagnostic", "message" to message)
                    override fun statistics(statistics: CastStatistics) = log.event("cast-stats",
                        "clients" to statistics.clients, "videoFrames" to statistics.videoFrames,
                        "audioFrames" to statistics.audioFrames, "silentFrames" to statistics.silentFrames,
                        "bytesSent" to statistics.bytesSent, "droppedFrames" to statistics.droppedFrames,
                        "overflows" to statistics.overflows, "rendererState" to statistics.rendererState)
                })
            hub.addOutput(LoggedOutput(cast, log))
        }
        options.clinkHeadUnit?.let { headUnit ->
            val clink = ClinkOutput(
                link = StaticClinkLink(ClinkAddresses(headUnit, options.clinkLocal!!)),
                pinProvider = PinProvider { options.clinkPin }, trustStore = MemoryTrustStore(),
                ports = ClinkPorts(), placeholder = BundledPlaceholderVideo(),
                listener = object : ClinkListener {
                    override fun state(state: ClinkState) = log.event("clink-state", "state" to state)
                    override fun error(error: Throwable) {
                        log.event("output-failed", "output" to "clink", "message" to error.message)
                        log.error("clink", error)
                    }
                    override fun statistics(stats: org.openxplay.stream.clink.ClinkStatistics) = log.event("clink-stats",
                        "liveFrames" to stats.liveFrames, "placeholderFrames" to stats.placeholderFrames, "droppedFrames" to stats.droppedFrames)
                    override fun profileMismatch(inputCodec: String, decoders: List<String>) = log.event("error",
                        "component" to "clink", "message" to "Decoder profile mismatch",
                        "inputCodec" to inputCodec, "decoders" to decoders)
                },
            )
            hub.addOutput(LoggedOutput(clink, log))
        }
    }

    override fun close() {
        if (!closed.compareAndSet(false, true)) return
        try {
            // Hub stop detaches and closes outputs and stops its input.
            hub.stop()
        } catch (failure: Exception) {
            log.error("shutdown", failure)
        } finally {
            try { input.stop() } catch (failure: Exception) { log.error("input", failure) }
            replay.awaitStopped()
            log.close()
        }
    }

    companion object {
        fun start(options: RelayOptions, stdout: PrintStream = System.out): DevRelay {
            val relay = DevRelay(options, stdout)
            try { relay.startComponents() } catch (failure: Exception) {
                relay.log.error("startup", failure)
                relay.close()
                throw failure
            }
            return relay
        }
    }
}

private class LoggedInput(private val input: ProjectionInput, private val log: EventLog) : ProjectionInput by input {
    override fun start(config: SessionConfig, sink: InputSink) = input.start(config, object : InputSink by sink {
        override fun session(state: SessionState) { log.session(state); sink.session(state) }
        private val audioStats = mutableMapOf<Int, LongArray>()
        override fun audioFormat(stream: Int, format: AudioFormat?) {
            audioStats.remove(stream)?.let { log.audioStats(stream, it[1], it[2]) }
            if (format != null) audioStats[stream] = longArrayOf(System.nanoTime(), 0, 0)
            log.audioFormat(stream, format); sink.audioFormat(stream, format)
        }
        override fun audioPacket(packet: AudioPacket) {
            audioStats[packet.stream]?.let {
                it[1]++; it[2] += packet.data.size
                val now = System.nanoTime()
                if (now - it[0] >= 1_000_000_000L) {
                    log.audioStats(packet.stream, it[1], it[2])
                    it[0] = now; it[1] = 0; it[2] = 0
                }
            }
            sink.audioPacket(packet)
        }
        override fun microphone(stream: Int, request: MicRequest?) {
            log.microphone(stream, request); sink.microphone(stream, request)
        }
    })
    override fun control(event: ControlEvent): Boolean {
        log.control(event)
        return input.control(event)
    }
}

private class LoggedOutput(
    private val output: ProjectionOutput,
    private val log: EventLog,
    private val reportOpened: Boolean = true,
) : ProjectionOutput by output {
    override fun open(host: OutputHost) {
        output.open(object : OutputHost by host {
            override fun capabilities(capabilities: OutputCapabilities) {
                log.capabilities(id, capabilities)
                host.capabilities(capabilities)
            }
        })
        if (reportOpened) log.event("output-opened", "output" to id)
    }
}

private class MemoryTrustStore : TrustStore {
    @Volatile private var identity: AuthIdentity? = null
    private val peers = ConcurrentHashMap<String, ByteArray>()
    override fun loadIdentity(): AuthIdentity? = identity
    override fun saveIdentity(identity: AuthIdentity) { this.identity = identity }
    override fun loadPeerKey(headUnitId: String): ByteArray? = peers[headUnitId]?.copyOf()
    override fun savePeerKey(headUnitId: String, publicKey: ByteArray) { peers[headUnitId] = publicKey.copyOf() }
    override fun forgetPeer(headUnitId: String) { peers.remove(headUnitId) }
}

private fun sslContext(options: RelayOptions): SSLContext? {
    val file = options.tlsKeystore ?: return null
    val password = options.tlsPassword!!.toCharArray()
    try {
        val store = KeyStore.getInstance("PKCS12")
        Files.newInputStream(file).use { store.load(it, password) }
        val keys = KeyManagerFactory.getInstance(KeyManagerFactory.getDefaultAlgorithm())
        keys.init(store, password)
        return SSLContext.getInstance("TLS").apply { init(keys.keyManagers, null, null) }
    } finally { password.fill('\u0000') }
}
