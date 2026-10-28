// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-22

package org.openxplay.stream.cast

import java.net.Inet4Address
import java.net.InetSocketAddress
import java.util.concurrent.ThreadFactory
import java.util.concurrent.atomic.AtomicBoolean
import org.openxplay.stream.core.*

internal data class CastOptions(
    val discovery: DiscoveryOptions = DiscoveryOptions(),
    val now: () -> Long = System::nanoTime,
    val searchMs: Long = 3000,
    val searchBackoffMs: List<Long> = listOf(2000, 5000, 10_000),
    val stoppedBackoffMs: List<Long> = listOf(3000, 10_000, 30_000, 60_000),
    val retryMs: Long = 2000,
    val pollMs: Long = 2000,
    val playRetryMs: Long = 1000,
    val playDeadlineMs: Long = 20_000,
    val noClientTimeoutMs: Long = 15_000,
    val stablePlayingMs: Long = 60_000,
    val bindRetryMs: Long = 5000,
    val server: ServerOptions = ServerOptions(),
)
internal val CAST_THREADS = ThreadFactory { task -> Thread(task).apply { isDaemon = true } }

/** Buffered H.264/H.265 and AAC output. Reopen only after the preceding lifecycle reports ENDED. */
public class CastOutput internal constructor(
    private val config: CastOutputConfig,
    private val network: CastNetwork,
    private val listener: CastListener,
    private val threadFactory: ThreadFactory,
    override val id: String,
    private val options: CastOptions,
) : ProjectionOutput {
    public constructor(
        config: CastOutputConfig,
        network: CastNetwork = CastNetwork.SYSTEM,
        listener: CastListener = object : CastListener {},
        threadFactory: ThreadFactory = CAST_THREADS,
        id: String = "cast",
    ) : this(config, network, listener, threadFactory, id, CastOptions())

    private class GenerationChanged : Exception()
    private class Run(val host: OutputHost, val server: CastStreamServer) {
        val lock = Object()
        val active = AtomicBoolean(true)
        val io = SocketScope()
        val failures = mutableSetOf<String>()
        var session: SessionId? = null
        var format: VideoFormat? = null
        var generation = 0L
        var timeline: CastTimeline? = null
        var audioStream: Int? = null
        val audioFormats = linkedMapOf<Int, AudioFormat>()
        var dropped = 0L
        var videoFrames = 0L
        var audioFrames = 0L
        var silentFrames = 0L
        @Volatile var renderer: DiscoveredRenderer? = null
        @Volatile var lastRenderer: DiscoveredRenderer? = null
        @Volatile var rendererState: String? = null
        var state: CastState? = null
        var stateRenderer: CastRenderer? = null
        var media: Thread? = null
        var http: Thread? = null
    }
    private val opened = AtomicBoolean(false)
    @Volatile private var current: Run? = null

    override fun open(host: OutputHost) {
        check(opened.compareAndSet(false, true)) { "output_already_open" }
        val run = Run(host, CastStreamServer(config.port, "", threadFactory, options.server, options.now)); current = run
        try {
            host.capabilities(OutputCapabilities(LatencyClass.BUFFERED,
                listOf(DecoderSupport("avc1.64002a", true), DecoderSupport("hvc1.1.6.L150", true)),
                mapOf(ScreenId.MAIN to DisplayInfo(config.width, config.height, 160, config.fps.toDouble(),
                    frameRate = if (config.fps == 60) FrameRatePreference.FPS_60 else FrameRatePreference.FPS_30)),
                0, emptySet(), if (config.audio) setOf(AudioCodec.AAC_LC) else emptySet(), false, prefersH264 = true))
            run.http = worker("cast-http") { http(run) }
            run.media = worker("cast-media") { media(run) }
            worker("cast-control") { control(run) }
        } catch (failure: Exception) {
            run.active.set(false)
            worker("cast-close") { run.io.close(); run.server.close() }
            if (current === run) current = null
            opened.set(false); throw failure
        }
    }
    private fun worker(name: String, block: () -> Unit): Thread = threadFactory.newThread(block).apply { this.name = name; start() }
    private fun reset(run: Run, format: VideoFormat?) {
        run.dropped += run.timeline?.dropped ?: 0
        run.format = format; run.generation++
        run.timeline = format?.let { CastTimeline(options.now(), config.audio) }
        run.server.reset(run.generation); run.lock.notifyAll()
    }
    override fun onSession(state: SessionState, config: SessionConfig) {
        val run = current ?: return
        synchronized(run.lock) {
            if (!run.active.get()) return
            if (run.session != state.id || state.phase == SessionPhase.ENDED) {
                run.session = if (state.phase == SessionPhase.ENDED) null else state.id
                run.audioFormats.clear(); run.audioStream = null
                reset(run, if (state.phase == SessionPhase.ENDED) null else run.format)
            }
        }
    }
    override fun onVideoFormat(screen: ScreenId, format: VideoFormat?) {
        if (screen != ScreenId.MAIN) return
        val run = current ?: return
        synchronized(run.lock) {
            if (!run.active.get()) return
            if (!sameVideoFormat(run.format, format)) reset(run, format)
            run.lock.notifyAll()
        }
    }
    override fun onVideoFrame(frame: VideoFrame) {
        if (frame.screen != ScreenId.MAIN) return
        val run = current ?: return
        synchronized(run.lock) {
            if (run.active.get() && frame.session == run.session) { run.timeline?.video(frame); run.lock.notifyAll() }
        }
    }
    override fun onAudioFormat(stream: Int, format: AudioFormat?) {
        if (!config.audio) return
        val run = current ?: return
        synchronized(run.lock) {
            if (!run.active.get()) return
            val previous = run.audioStream
            run.audioFormats.remove(stream)
            if (format?.role == AudioRole.MEDIA && format.codec == AudioCodec.AAC_LC && format.sampleRate == 48_000 &&
                format.channels == 2 && format.config?.contentEquals(byteArrayOf(0x11, 0x90.toByte())) == true) run.audioFormats[stream] = format
            run.audioStream = run.audioFormats.keys.lastOrNull()
            if (previous != run.audioStream || previous == stream) run.timeline?.clearAudio()
        }
    }
    override fun onAudioPacket(packet: AudioPacket) {
        val run = current ?: return
        synchronized(run.lock) {
            if (run.active.get() && packet.session == run.session && packet.stream == run.audioStream) run.timeline?.audio(packet.data)
        }
    }
    override fun onMicrophone(stream: Int, request: MicRequest?): Unit = Unit
    override fun close() {
        val run = current ?: return
        if (!run.active.compareAndSet(true, false)) return
        synchronized(run.lock) { run.lock.notifyAll() }
        // Socket close can block in platform implementations, so the caller only signals shutdown.
        worker("cast-close") { run.io.close(); run.server.close() }
    }
    private fun wait(run: Run, ms: Long, generation: Long? = null): Boolean {
        val deadline = System.nanoTime() + ms * 1_000_000
        synchronized(run.lock) {
            while (run.active.get() && (generation == null || run.generation == generation)) {
                val remaining = (deadline - System.nanoTime()) / 1_000_000
                if (remaining <= 0) break
                run.lock.wait(remaining.coerceAtLeast(1))
            }
        }
        return run.active.get()
    }
    private fun notifyListener(call: () -> Unit) { runCatching(call) }
    private fun failure(run: Run, code: String, detail: String) {
        val report = synchronized(run.lock) { run.active.get() && run.failures.add(code) }
        if (report) notifyListener { listener.failure(CastFailure(code, detail)) }
    }
    private fun recovered(run: Run, code: String) { synchronized(run.lock) { run.failures.remove(code) } }
    private fun state(run: Run, state: CastState) {
        val renderer = run.renderer?.renderer
        if (run.state == state && run.stateRenderer == renderer) return
        run.state = state; run.stateRenderer = renderer
        notifyListener { listener.state(state, renderer) }
    }
    private fun host(run: Run, call: (OutputHost) -> Unit) {
        // The hub may hold the input lock while delivering media; never call back under our inbox lock.
        if (run.active.get()) call(run.host)
    }
    private fun http(run: Run) {
        while (run.active.get()) {
            try {
                run.server.bind(); recovered(run, "stream_server_failed")
                run.server.accept { run.active.get() }
                if (run.active.get()) throw java.io.IOException("HTTP accept ended")
            } catch (error: Exception) {
                if (run.active.get()) failure(run, "stream_server_failed", "HTTP bind or accept failed: ${error.javaClass.simpleName}")
            }
            if (!wait(run, options.bindRetryMs)) break
        }
    }
    private fun media(run: Run) {
        var revision = -1L
        var muxer: CastMediaMuxer? = null
        var reportedAt = options.now()
        val limiter = KeyframeLimiter(1_000_000_000)
        try {
            while (run.active.get()) {
                val now = options.now()
                val snapshot = synchronized(run.lock) {
                    val changed = revision != run.generation
                    if (changed) {
                        revision = run.generation
                        val generation = revision
                        muxer = run.format?.let { CastMediaMuxer(it, config.audio) { chunk -> run.server.publish(generation, chunk) } }
                    }
                    val timeline = run.timeline
                    val serverRequest = run.server.needsKeyframe.getAndSet(false)
                    val request = timeline != null && (timeline.requestKeyframe || serverRequest)
                    if (timeline != null) timeline.requestKeyframe = false
                    Pair(timeline?.take(now).orEmpty(), request)
                }
                val requests = if (snapshot.second) limiter.request(ControlEvent.KeyFrameRequest(ScreenId.MAIN, Origin(id, "renderer")), now) else limiter.poll(now)
                requests.forEach { event -> host(run) { it.control(event) } }
                for (event in snapshot.first) {
                    if (!run.active.get()) break
                    try {
                        muxer?.write(event, now)
                        synchronized(run.lock) {
                            when (event) {
                                is MediaEvent.Video -> run.videoFrames++
                                is MediaEvent.Audio -> { run.audioFrames++; if (event.silent) run.silentFrames++ }
                            }
                        }
                    } catch (_: IllegalArgumentException) {
                        synchronized(run.lock) { run.dropped++; run.timeline?.requestKeyframe = true }
                    }
                }
                if (now - reportedAt >= 1_000_000_000) {
                    val statistics = synchronized(run.lock) { CastStatistics(run.server.clients, run.videoFrames, run.audioFrames,
                        run.silentFrames, run.server.bytesSent.get(), run.dropped + (run.timeline?.dropped ?: 0),
                        run.server.overflows.get(), run.rendererState) }
                    notifyListener { listener.statistics(statistics) }
                    host(run) { it.stats(OutputStats(Origin(id, "renderer"), ScreenId.MAIN, null, 0, statistics.droppedFrames, 0, null)) }
                    reportedAt = now
                }
                synchronized(run.lock) { if (run.active.get()) run.lock.wait(10) }
            }
        } catch (error: Exception) {
            failure(run, "stream_server_failed", "Media worker failed: ${error.javaClass.simpleName}")
            close()
        }
    }
    private fun localAddress(run: Run, renderer: DiscoveredRenderer): String {
        val uri = renderer.transport.control
        val socket = run.io.add(network.socket())
        try {
            socket.use {
                socket.connect(InetSocketAddress(uri.host, if (uri.port < 0) 80 else uri.port), 2000)
                require(socket.localAddress is Inet4Address) { "Local route requires IPv4" }
                return socket.localAddress.hostAddress
            }
        } finally { run.io.remove(socket) }
    }
    private fun control(run: Run) {
        val soap = CastSoap(CastHttpClient(network, run.io))
        var searchRounds = 0
        var failures = 0
        var started: Long? = null
        var local = ""
        var playedAt = 0L
        var pollAt = 0L
        var hasPlayed = false
        var playingSince: Long? = null
        var missingClientSince: Long? = null
        var stoppedRound = 0
        val protocolChecked = mutableSetOf<String>()
        try {
            while (run.active.get()) {
                if (run.renderer == null) {
                    state(run, CastState.SEARCHING)
                    val result = runCatching { CastDiscovery(network, options.discovery, run.io) { run.active.get() }.scan(options.searchMs) }
                        .getOrElse { DiscoveryResult(emptyList(), emptySet()) }
                    if (!run.active.get()) break
                    val found = result.renderers.firstOrNull { it.renderer.udn == config.rendererUdn }
                    if (found == null) {
                        if (config.rendererUdn in result.withoutTransport) failure(run, "renderer_no_avtransport", "Selected renderer has no AVTransport service")
                        searchRounds++
                        if (searchRounds >= 3) failure(run, "renderer_not_found", "Selected UDN absent after three searches")
                        wait(run, options.searchBackoffMs[(searchRounds - 1).coerceAtMost(options.searchBackoffMs.lastIndex)])
                        continue
                    }
                    run.renderer = found; run.lastRenderer = found; searchRounds = 0; failures = 0; started = null
                    recovered(run, "renderer_not_found"); recovered(run, "renderer_no_avtransport")
                    state(run, CastState.WAITING_FOR_VIDEO)
                    try {
                        local = localAddress(run, found)
                        found.connection?.takeIf { protocolChecked.add(found.renderer.udn) }?.let {
                            runCatching { soap.action(it, "GetProtocolInfo", emptyMap()).arguments["Sink"] }.getOrNull()?.let { sink ->
                                if (sink.isNotBlank() && !sink.contains("video/mp2t", true) && '*' !in sink)
                                    notifyListener { listener.diagnostic("ConnectionManager Sink does not advertise video/mp2t; trying MPEG-TS") }
                            }
                        }
                    } catch (_: Exception) {
                        failure(run, "renderer_unreachable", "Cannot connect to renderer control endpoint")
                        run.renderer = null; wait(run, options.retryMs); continue
                    }
                }
                val renderer = run.renderer ?: continue
                val generation = synchronized(run.lock) { run.generation }
                if (started != null && started != generation) {
                    runCatching { soap.action(renderer.transport, "Stop") }; started = null
                }
                if (!run.server.ready(generation) || run.server.port == 0) {
                    state(run, CastState.WAITING_FOR_VIDEO)
                    synchronized(run.lock) { if (run.active.get()) run.lock.wait(20) }
                    continue
                }
                fun request(action: String, arguments: Map<String, String> = mapOf("InstanceID" to "0")): SoapResult {
                    fun valid(): Boolean = run.active.get() && synchronized(run.lock) { run.generation == generation }
                    if (!valid()) throw GenerationChanged()
                    val result = try { soap.action(renderer.transport, action, arguments) }
                    catch (fault: SoapFault) { failures = 0; recovered(run, "renderer_unreachable"); throw fault }
                    catch (error: Exception) { failures++; throw error }
                    failures = 0; recovered(run, "renderer_unreachable")
                    if (!valid()) throw GenerationChanged()
                    return result
                }
                try {
                    if (started != generation) {
                        state(run, CastState.STARTING)
                        try { request("Stop") } catch (_: SoapFault) { }
                        val url = "http://$local:${run.server.port}${run.server.path(generation)}"
                        val arguments = mapOf("InstanceID" to "0", "CurrentURI" to url, "CurrentURIMetaData" to CastSoap.metadata(config.title, url))
                        try { request("SetAVTransportURI", arguments) }
                        catch (_: SoapFault) { request("SetAVTransportURI", arguments + ("CurrentURIMetaData" to "")) }
                        var attempts = 0
                        while (run.active.get()) {
                            try { request("Play", mapOf("InstanceID" to "0", "Speed" to "1")); break }
                            catch (fault: SoapFault) {
                                if (fault.code != 701 || attempts++ >= 3) throw fault
                                if (!wait(run, options.playRetryMs, generation)) break
                            }
                        }
                        if (!run.active.get()) break
                        started = generation; playedAt = options.now(); pollAt = playedAt; hasPlayed = false; playingSince = null
                        missingClientSince = null
                        recovered(run, "renderer_rejected")
                    }
                    if (options.now() < pollAt) { wait(run, minOf(100, ((pollAt - options.now()) / 1_000_000).coerceAtLeast(1))); continue }
                    val transportState = request("GetTransportInfo").arguments["CurrentTransportState"].orEmpty()
                    run.rendererState = transportState; failures = 0
                    val now = options.now(); val clients = run.server.clients
                    pollAt = now + options.pollMs * 1_000_000
                    if (hasPlayed && clients == 0 && transportState in setOf("PLAYING", "TRANSITIONING")) {
                        if (missingClientSince == null) missingClientSince = now
                    } else missingClientSince = null
                    val stalled = missingClientSince?.let { now - it >= options.noClientTimeoutMs * 1_000_000 } == true
                    if (transportState == "PLAYING" && clients > 0) {
                        if (playingSince == null) playingSince = now
                        hasPlayed = true; state(run, CastState.PLAYING)
                        if (options.now() - playingSince >= options.stablePlayingMs * 1_000_000) {
                            stoppedRound = 0; recovered(run, "renderer_stopped")
                        }
                    } else {
                        playingSince = null
                        if (transportState == "PAUSED_PLAYBACK") { state(run, CastState.PAUSED); continue }
                        val stopped = hasPlayed && transportState in setOf("STOPPED", "NO_MEDIA_PRESENT") ||
                            !hasPlayed && now - playedAt >= options.playDeadlineMs * 1_000_000 && transportState != "PAUSED_PLAYBACK" || stalled
                        if (stopped) {
                            // Report the reason first so listeners already have it when the state changes.
                            failure(run, "renderer_stopped", if (stalled) "Renderer reports $transportState without streaming HTTP clients" else
                                "Renderer stopped or did not start within playback deadline")
                            state(run, CastState.STOPPED_BY_RENDERER)
                            val delay = options.stoppedBackoffMs[stoppedRound.coerceAtMost(options.stoppedBackoffMs.lastIndex)]; stoppedRound++
                            if (!wait(run, delay, generation)) break
                            synchronized(run.lock) { if (run.generation == generation) reset(run, run.format) }
                        }
                    }
                } catch (_: GenerationChanged) {
                    started = generation
                } catch (fault: SoapFault) {
                    failures = 0; started = null
                    failure(run, "renderer_rejected", fault.detail()); wait(run, options.retryMs, generation)
                } catch (_: Exception) {
                    if (failures >= 3) {
                        failure(run, "renderer_unreachable", "Three consecutive control requests failed")
                        run.renderer = null; started = null; failures = 0
                    }
                    wait(run, options.retryMs, generation)
                }
            }
        } finally {
            run.active.set(false); synchronized(run.lock) { run.lock.notifyAll() }
            run.io.close(); run.server.close()
            (run.renderer ?: run.lastRenderer)?.let { renderer ->
                val stopping = SocketScope()
                val stop = worker("cast-stop") {
                    runCatching { CastSoap(CastHttpClient(network, stopping)).action(renderer.transport, "Stop", budgetMs = 1000) }
                }
                stop.join(1000); stopping.close(); stop.join(100)
            }
            run.media?.join(1500); run.http?.join(1500); run.server.awaitWriters()
            if (current === run) current = null
            opened.set(false); state(run, CastState.ENDED)
        }
    }
}

internal fun sameVideoFormat(a: VideoFormat?, b: VideoFormat?): Boolean = a === b || a != null && b != null &&
    a.codec == b.codec && a.codecString == b.codecString && a.width == b.width && a.height == b.height && a.config.contentEquals(b.config)
