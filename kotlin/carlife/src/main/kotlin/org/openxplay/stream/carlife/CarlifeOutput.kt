// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-11-02

package org.openxplay.stream.carlife

import java.io.Closeable
import java.io.InputStream
import java.io.OutputStream
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.NetworkInterface
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.ScheduledThreadPoolExecutor
import java.util.concurrent.Semaphore
import java.util.concurrent.ThreadFactory
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import org.openxplay.stream.core.*
import org.openxplay.stream.core.codec.AvcConfig
import org.openxplay.stream.core.codec.Nal

/**
 * An independent CarLife sender. Only the core projection contract crosses this boundary.
 * Every caller-facing operation is nonblocking; protocol and media use separate bounded workers.
 */
public class CarlifeOutput(
    private val config: CarlifeConfig = CarlifeConfig(),
    private val deviceInfo: CarlifeDeviceInfo = CarlifeDeviceInfo(),
    private val listener: CarlifeListener = CarlifeListener {},
    override val id: String = "carlife",
) : ProjectionOutput {
    private data class PendingFrame(val frame: VideoFrame, val format: VideoFormat, val epoch: Long)
    private class Run(val host: OutputHost, val executor: ScheduledThreadPoolExecutor, val watchdog: ScheduledThreadPoolExecutor) {
        val active = AtomicBoolean(true)
        val servers = mutableListOf<ServerSocket>()
        val queue = ArrayBlockingQueue<PendingFrame>(3)
        val epoch = AtomicLong()
        val readers = Semaphore(14)
        val receivedFrames = AtomicLong()
        val droppedFrames = AtomicLong()
        val receivedKeyframes = AtomicLong()
        val keyRequestPending = AtomicBoolean()
        @Volatile var session: CarlifeSession? = null
        @Volatile var format: VideoFormat? = null
        @Volatile var inputSession: SessionId? = null
        @Volatile var awaitingKeyframe = true
    }

    private val threads = ThreadFactory { work -> Thread(work, "carlife").apply { isDaemon = true } }
    @Volatile private var run: Run? = null
    @Volatile private var currentStatus = CarlifeStatus(CarlifeState.CLOSED)
    @Volatile private var listeningPorts: List<Int> = emptyList()
    public val status: CarlifeStatus get() = currentStatus
    /** Actual listener ports, populated asynchronously; port zero selects an ephemeral test port. */
    public val boundPorts: List<Int> get() = listeningPorts

    override fun open(host: OutputHost) {
        check(run == null) { "CarLife output is already open" }
        fun worker(): ScheduledThreadPoolExecutor = ScheduledThreadPoolExecutor(1, threads).apply { removeOnCancelPolicy = true }
        val fresh = Run(host, worker(), worker())
        run = fresh
        fresh.executor.execute {
            try {
                if (!fresh.active.get()) return@execute
                if (config.tcp) listen(fresh)
                if (!fresh.active.get()) return@execute
                publish(fresh, CarlifeStatus(CarlifeState.LISTENING))
                fresh.executor.scheduleWithFixedDelay({ safely(fresh) { fresh.session?.tick() } }, 1, 1, TimeUnit.SECONDS)
                fresh.watchdog.scheduleWithFixedDelay({
                    fresh.session?.let { selected -> selected.transportFailure(System.nanoTime())?.let { (channel, reason) ->
                        terminate(fresh, selected, reason, channel)
                    } }
                }, 1, 1, TimeUnit.SECONDS)
                if (config.tcp && config.discovery) {
                    fresh.executor.scheduleWithFixedDelay({ discover(fresh) }, 0, 2, TimeUnit.SECONDS)
                }
            } catch (error: Exception) {
                publish(fresh, CarlifeStatus(CarlifeState.FAILED, reason = error.javaClass.simpleName))
                stop(fresh)
            }
        }
        threads.newThread { media(fresh) }.start()
    }

    private fun publish(current: Run, next: CarlifeStatus) {
        if (run !== current || !current.active.get()) return
        currentStatus = next
        runCatching { listener.onStatus(next) }
    }

    private fun session(current: Run, peer: String): CarlifeSession {
        lateinit var selected: CarlifeSession
        fun live(): Boolean = run === current && current.active.get() && current.session === selected
        val fenced = object : OutputHost {
            override fun capabilities(capabilities: OutputCapabilities) { if (live()) current.host.capabilities(capabilities) }
            override fun control(event: ControlEvent) { if (live()) current.host.control(event) }
            override fun stats(stats: OutputStats) { if (live()) current.host.stats(stats) }
        }
        selected = CarlifeSession(peer, config, deviceInfo, fenced, Origin(id, "carlife-${System.nanoTime()}"),
            { if (live()) publish(current, it) }, {
                // Terminal diagnostics survive close; host callbacks and a later lifecycle remain fenced.
                if (current.session === selected && (run === current || run == null)) runCatching { listener.diagnostic(it) }
            }, { current.format },
            { "sourceFrames=${current.receivedFrames.get()} sourceKeyframes=${current.receivedKeyframes.get()} " +
                "dropped=${current.droppedFrames.get()} queue=${current.queue.size}" })
        return selected
    }

    private fun listen(current: Run) {
        val servers = config.ports.map { port ->
            ServerSocket().apply {
                synchronized(current.servers) {
                    check(current.active.get()) { "Output closed" }
                    current.servers += this
                }
                reuseAddress = true
                bind(InetSocketAddress(config.bindAddress, port), 2)
            }
        }
        listeningPorts = servers.map { it.localPort }
        servers.forEachIndexed { index, server ->
            threads.newThread {
                while (current.active.get()) {
                    val socket = try { server.accept() } catch (_: Exception) { break }
                    if (!current.readers.tryAcquire()) { socket.close(); continue }
                    try {
                        socket.tcpNoDelay = true
                        socket.keepAlive = true
                        threads.newThread {
                            try { accept(current, socket, index + 1) } finally { current.readers.release() }
                        }.start()
                    } catch (_: Exception) {
                        runCatching { socket.close() }
                        current.readers.release()
                    }
                }
            }.start()
        }
    }

    private fun accept(current: Run, socket: Socket, channel: Int) {
        val peer = socket.inetAddress.hostAddress ?: "tcp"
        val connection = try {
            CarlifeConnection(socket.getInputStream(), socket.getOutputStream(), socket, false, channel)
        } catch (_: Exception) {
            runCatching { socket.close() }
            return
        }
        var owner: CarlifeSession? = null
        try {
            owner = current.executor.submit<CarlifeSession?> {
                if (!current.active.get() || !connection.active.get()) return@submit null
                val existing = current.session
                if (existing != null && existing.peer != peer) return@submit null
                // A replacement control socket is a new lifetime, never a continuation of old authentication.
                val selected = if (existing == null || (channel == 1 && existing.ownsChannel(1))) {
                    existing?.close("control_replaced")
                    resetMedia(current)
                    session(current, peer).also { current.session = it }
                } else existing
                selected.attach(connection)
                selected
            }.get(10, TimeUnit.SECONDS)
            if (owner != null) read(current, owner, connection)
        } catch (_: Exception) {
            owner?.let { ended(current, it, connection, "transport_failed") }
        } finally { connection.close() }
    }

    /** Attaches the Android accessory's duplex streams; closing the handle also cancels a pending attach. */
    public fun attachAccessory(input: InputStream, output: OutputStream, closeable: Closeable): Closeable {
        val current = checkNotNull(run) { "Open the output before attaching an accessory" }
        val attached = AtomicBoolean(true)
        val connection = CarlifeConnection(input, output, closeable, true, 1)
        current.executor.execute {
            if (!current.active.get() || !attached.get()) { connection.close(); return@execute }
            current.session?.close("accessory_replaced")
            resetMedia(current)
            val selected = session(current, "usb").also { current.session = it }
            selected.attach(connection)
            threads.newThread { read(current, selected, connection) }.start()
        }
        return Closeable {
            if (attached.compareAndSet(true, false)) threads.newThread {
                connection.close()
                current.session?.takeIf { it.owns(connection) }?.let { ended(current, it, connection, "accessory_detached") }
            }.start()
        }
    }

    private fun read(current: Run, owner: CarlifeSession, connection: CarlifeConnection) {
        var reason = "peer_closed"
        var messageId: Int? = null
        var messageChannel = connection.channel
        try {
            while (current.active.get() && owner.active.get() && connection.active.get()) {
                messageId = null
                messageChannel = connection.channel
                val message = if (connection.usb) CarlifeFraming.readUsb(connection.input)
                    else CarlifeFraming.read(connection.input, connection.channel)
                if (message == null) break
                messageId = message.id
                messageChannel = message.channel
                current.executor.submit {
                    if (current.active.get() && connection.active.get() && current.session === owner && owner.owns(connection)) {
                        owner.receive(message)
                        current.format?.let { owner.format(it) }
                    }
                }.get(10, TimeUnit.SECONDS)
            }
        } catch (error: Exception) {
            val cause = error.cause ?: error
            reason = failureCode(cause)
            owner.failure((cause as? CarlifeProtocolException)?.channel ?: messageChannel,
                (cause as? CarlifeProtocolException)?.messageId ?: messageId,
                reason, cause.javaClass.simpleName)
        }
        finally { ended(current, owner, connection, reason) }
    }

    private fun ended(current: Run, owner: CarlifeSession, connection: CarlifeConnection, reason: String) {
        connection.close()
        if (!current.active.get()) return
        runCatching { current.executor.execute {
            if (current.session === owner && owner.owns(connection)) {
                owner.failure(connection.channel, null, reason, "none")
                owner.close(reason)
                current.session = null
                resetMedia(current)
                publish(current, CarlifeStatus(CarlifeState.LISTENING, reason = reason))
            }
        } }
    }

    private fun safely(current: Run, work: () -> Unit) {
        try { work() } catch (error: Exception) {
            val selected = current.session ?: return
            val reason = failureCode(error)
            selected.failure(0, null, reason, error.javaClass.simpleName)
            selected.close(reason)
            current.session = null
            resetMedia(current)
            publish(current, CarlifeStatus(CarlifeState.LISTENING, reason = reason))
        }
    }

    private fun terminate(current: Run, selected: CarlifeSession, reason: String, channel: Int = 0) {
        // Closing the stream unblocks both a stalled write and any exact-read loop. This watchdog
        // runs independently of the protocol worker, which itself may be waiting on that write.
        selected.failure(channel, if (reason == "video_write_failed") 131073 else null, reason, "none")
        selected.close(reason)
        runCatching { current.executor.execute {
            if (current.session === selected) {
                current.session = null
                resetMedia(current)
                publish(current, CarlifeStatus(CarlifeState.LISTENING, reason = reason))
            }
        } }
    }

    private fun discover(current: Run) {
        if (!current.active.get() || current.session?.authenticated == true) return
        val addresses = linkedSetOf<String>()
        config.targetAddress?.takeIf { it.isNotBlank() }?.let { addresses += it }
        if (config.targetAddress == null) {
            addresses += "255.255.255.255"
            runCatching { NetworkInterface.getNetworkInterfaces()?.toList().orEmpty().forEach { network ->
                if (network.isUp && !network.isLoopback) network.interfaceAddresses.mapNotNullTo(addresses) {
                    it.broadcast?.hostAddress
                }
            } }
        }
        runCatching {
            DatagramSocket().use { socket ->
                socket.broadcast = true
                val bytes = byteArrayOf(1)
                addresses.forEach { target -> runCatching {
                    socket.send(DatagramPacket(bytes, bytes.size, InetAddress.getByName(target), 7999))
                } }
            }
        }
    }

    override fun onSession(state: SessionState, config: SessionConfig) {
        val current = run ?: return
        if (current.inputSession != state.id || state.phase == SessionPhase.ENDED) resetMedia(current)
        current.inputSession = if (state.phase == SessionPhase.ENDED) null else state.id
    }

    override fun onVideoFormat(screen: ScreenId, format: VideoFormat?) {
        if (screen != ScreenId.MAIN) return
        val current = run ?: return
        current.format = format
        resetMedia(current)
        if (format != null) runCatching {
            current.executor.execute { safely(current) { current.session?.format(format) } }
        }
    }

    override fun onVideoFrame(frame: VideoFrame) {
        val current = run ?: return
        if (!current.active.get() || frame.screen != ScreenId.MAIN || frame.session != current.inputSession) return
        val format = current.format ?: return
        if (format.codec != VideoCodec.H264 || current.session?.streaming != true) return
        current.receivedFrames.incrementAndGet()
        if (frame.keyFrame) current.receivedKeyframes.incrementAndGet()
        if (current.awaitingKeyframe && !frame.keyFrame) { current.droppedFrames.incrementAndGet(); return }
        if (frame.keyFrame) current.awaitingKeyframe = false
        if (!current.queue.offer(PendingFrame(frame, format, current.epoch.get()))) {
            current.droppedFrames.addAndGet(current.queue.size.toLong() + if (frame.keyFrame) 0 else 1)
            current.queue.clear()
            current.awaitingKeyframe = true
            if (frame.keyFrame) {
                current.awaitingKeyframe = false
                current.queue.offer(PendingFrame(frame, format, current.epoch.get()))
            }
            if (current.keyRequestPending.compareAndSet(false, true)) runCatching {
                current.executor.execute {
                    current.keyRequestPending.set(false)
                    current.session?.requestKeyframe()
                }
            }
        }
    }

    private fun media(current: Run) {
        var nextSend = 0L
        var previous: CarlifeSession? = null
        var videoFormat: VideoFormat? = null
        var parameterSetsSent = false
        while (current.active.get()) {
            val pending = try { current.queue.poll(250, TimeUnit.MILLISECONDS) } catch (_: InterruptedException) { null }
                ?: continue
            val selected = current.session ?: continue
            if (selected !== previous) {
                nextSend = 0; previous = selected; videoFormat = null; parameterSetsSent = false
            }
            if (pending.epoch != current.epoch.get() || pending.format !== current.format || !selected.streaming) continue
            if (pending.format !== videoFormat) { videoFormat = pending.format; parameterSetsSent = false }
            try {
                val period = 1_000_000_000L / selected.fps
                val wait = nextSend - System.nanoTime()
                if (wait > 0) TimeUnit.NANOSECONDS.sleep(wait)
                if (pending.epoch != current.epoch.get() || current.session !== selected) continue
                val annex = Nal.avccToAnnexB(pending.frame.data)
                val bytes = when {
                    selected.parameterSetsInEveryKeyframe -> if (pending.frame.keyFrame) {
                        val parsed = AvcConfig.parse(pending.format.config)
                        val sets = (parsed.sps + parsed.pps).fold(byteArrayOf()) { all, nal ->
                            all + byteArrayOf(0, 0, 0, 1) + nal
                        }
                        sets + annex
                    } else annex
                    // A 3.x receiver keeps the first frame's parameter sets and supplies them
                    // itself after a decoder reset, but only to a keyframe whose first NAL is IDR.
                    pending.frame.keyFrame -> {
                        val idr = Nal.units(pending.frame.data, Nal.Format.AVCC)
                            .dropWhile { it[0].toInt() and 0x1f != 5 }.toList()
                        if (idr.isEmpty()) annex else {
                            val sets = if (parameterSetsSent) byteArrayOf() else AvcConfig.parse(pending.format.config)
                                .let { parsed -> (parsed.sps + parsed.pps).fold(byteArrayOf()) { all, nal ->
                                    all + byteArrayOf(0, 0, 0, 1) + nal
                                } }
                            parameterSetsSent = true
                            sets + idr.fold(byteArrayOf()) { all, nal -> all + byteArrayOf(0, 0, 0, 1) + nal }
                        }
                    }
                    else -> annex
                }
                try {
                    if (selected.video(bytes)) {
                        val now = System.nanoTime()
                        nextSend = if (nextSend == 0L) now + period else maxOf(nextSend + period, now)
                    }
                } catch (_: Exception) { terminate(current, selected, "video_write_failed", 2) }
            } catch (_: Exception) {
                current.awaitingKeyframe = true
                current.queue.clear()
                runCatching { current.executor.execute { safely(current) { selected.requestKeyframe() } } }
            }
        }
    }

    private fun resetMedia(current: Run) {
        current.epoch.incrementAndGet()
        current.droppedFrames.addAndGet(current.queue.size.toLong())
        current.queue.clear()
        current.awaitingKeyframe = true
    }

    private fun failureCode(error: Throwable): String = when (error) {
        is CarlifeProtocolException -> error.code
        is java.util.concurrent.TimeoutException -> "protocol_worker_timeout"
        is java.io.IOException -> "transport_io"
        is java.security.GeneralSecurityException -> "content_encryption_failed"
        else -> "protocol_error"
    }

    override fun onAudioFormat(stream: Int, format: AudioFormat?): Unit = Unit
    override fun onAudioPacket(packet: AudioPacket): Unit = Unit
    override fun onMicrophone(stream: Int, request: MicRequest?): Unit = Unit

    override fun close() {
        val current = run ?: return
        run = null
        currentStatus = CarlifeStatus(CarlifeState.CLOSED)
        listeningPorts = emptyList()
        stop(current)
    }

    private fun stop(current: Run) {
        if (!current.active.compareAndSet(true, false)) return
        threads.newThread {
            synchronized(current.servers) { current.servers.forEach { runCatching { it.close() } }; current.servers.clear() }
            current.session?.close("output_closed")
            current.queue.clear()
            current.executor.shutdownNow()
            current.watchdog.shutdownNow()
            if (run == null) runCatching { listener.onStatus(CarlifeStatus(CarlifeState.CLOSED)) }
        }.start()
    }
}
