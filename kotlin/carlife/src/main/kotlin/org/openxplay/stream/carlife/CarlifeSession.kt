// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-11-02

package org.openxplay.stream.carlife

import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.atomic.AtomicReference
import org.openxplay.stream.core.*

/** One head unit and one authenticated lifetime; a reconnect always receives a new instance. */
internal class CarlifeSession(
    val peer: String,
    private val config: CarlifeConfig,
    private val info: CarlifeDeviceInfo,
    private val host: OutputHost,
    private val origin: Origin,
    private val status: (CarlifeStatus) -> Unit,
    private val diagnosticSink: (String) -> Unit,
    private val sourceFormat: () -> VideoFormat? = { null },
    private val mediaCounters: () -> String = { "" },
) {
    val active = AtomicBoolean(true)
    private val channels = ConcurrentHashMap<Int, CarlifeConnection>()
    private val crypto = CarlifeCrypto()
    private var version = false
    private var peerMajor = 0L
    private var profile: ReceiverProfile? = null
    private var deviceSent = false
    private var authAnswered = false
    private var authAccepted = false
    @Volatile var authenticated = false
        private set
    @Volatile var streaming = false
        private set
    @Volatile var width = 0
        private set
    @Volatile var height = 0
        private set
    @Volatile var fps = config.fps
        private set
    /** The selected receiver generation; available after 98305. */
    internal val receiver: ReceiverProfile get() = checkNotNull(profile)
    internal val parameterSetsInEveryKeyframe: Boolean get() = receiver.parameterSetsInEveryKeyframe
    private var encoderChannel = 1
    private var encryptionRequested = false
    private var lastKeyframeRequest = 0L
    private val lastSent = AtomicLong(System.nanoTime())
    private val sent = AtomicLong()
    private val createdAt = System.nanoTime()
    @Volatile private var phase = "awaiting_version"
    private val lastReceived = AtomicLong(createdAt)
    private val received = AtomicLong()
    private val keyframeRequests = AtomicLong()
    private val inputDown = AtomicLong()
    private val inputUp = AtomicLong()
    private val inputMove = AtomicLong()
    private var previousTouchCount = 0
    private var activeContacts: List<TouchContact> = emptyList()
    private var lastSummary = createdAt
    private var knownFormat: VideoFormat? = null
    private data class Failure(val channel: Int, val id: Int?, val cause: String)
    private val rootFailure = AtomicReference<Failure?>()
    private val wireLogAt = mutableMapOf<Int, Long>()

    private fun diagnostic(event: String) {
        diagnosticSink("session=${origin.clientId} phase=$phase elapsedMs=${(System.nanoTime() - createdAt) / 1_000_000} $event")
    }

    private fun diagnosticVersion(value: Long?): String = when (value) {
        null -> "absent"
        in 0L..999L -> value.toString()
        else -> "out_of_range"
    }

    private fun stage(next: String) {
        phase = next
        diagnostic("phase_changed")
    }

    fun attach(connection: CarlifeConnection) {
        if (!active.get()) { connection.close(); return }
        if (connection.usb) {
            channels.values.toSet().forEach { it.close() }
            channels.clear()
            for (channel in 1..7) channels[channel] = connection
        } else channels.put(connection.channel, connection)?.close()
        diagnostic("transport_opened peer=$peer channel=${connection.channel} transport=${if (connection.usb) "usb" else "tcp"}")
    }

    fun owns(connection: CarlifeConnection): Boolean = channels[connection.channel] === connection

    fun ownsChannel(channel: Int): Boolean = channels[channel] != null

    fun receive(wire: CarlifeMessage) {
        if (!active.get()) return
        val message = crypto.transform(wire, false)
        lastReceived.set(System.nanoTime())
        received.incrementAndGet()
        if (message.id !in setOf(131074, 425985, 425998)) {
            val now = System.nanoTime()
            val previous = wireLogAt[message.id]
            if ((previous != null && now - previous >= 10_000_000_000L) || (previous == null && wireLogAt.size < 64)) {
                wireLogAt[message.id] = now
                diagnostic("rx channel=${message.channel} id=${message.id} bytes=${message.payload.size}")
            }
        }
        if (!version && message.id != 98305) return
        val payload by lazy { Proto.parse(message.payload) }
        when (message.id) {
            98305 -> {
                protocolRequire(!version, "unexpected_phase")
                val announced = payload.number(1) ?: run {
                    diagnostic("version_rejected major=absent minor=${diagnosticVersion(payload.number(2))}")
                    throw CarlifeProtocolException("version_mismatch")
                }
                val selected = ReceiverProfile.of(announced) ?: run {
                    diagnostic("version_rejected major=${diagnosticVersion(announced)} minor=${diagnosticVersion(payload.number(2))}")
                    throw CarlifeProtocolException("version_mismatch")
                }
                peerMajor = announced
                profile = selected
                version = true
                stage("version_matched")
                send(1, 65538, ProtoWriter().number(1, 1).number(2, announced).build())
                report(CarlifeState.CONNECTED)
            }
            98343, 98307 -> sendDevice()
            98376 -> {
                protocolRequire(deviceSent, "unexpected_phase")
                val answer = CarlifeCrypto.authenticate(info, payload.text(1) ?: throw CarlifeProtocolException("malformed_protobuf"))
                send(1, 65609, ProtoWriter().text(1, answer).build())
                authAnswered = true
                stage("auth_answered")
            }
            98378 -> {
                protocolRequire(authAnswered, "unexpected_phase")
                protocolRequire(payload.number(1) == 1L, "auth_rejected")
                authAccepted = true
                stage("auth_accepted")
                send(1, 65611, ProtoWriter().number(1, 1).build())
            }
            98380 -> {
                protocolRequire(authAccepted, "unexpected_phase")
                authenticated = true
                stage("established")
                report(CarlifeState.AUTHENTICATED)
                val flags = listOf("AAC_SUPPORT" to 0, "CONTENT_ENCRYPTION" to 0,
                    "CONNECT_TYPE" to if (channels[1]?.usb == true) 2 else 5)
                val writer = ProtoWriter().number(1, flags.size.toLong())
                for ((key, value) in flags) writer.data(2, ProtoWriter().text(1, key).number(2, value.toLong()).build())
                send(1, 65617, writer.build())
            }
            98386 -> {
                if (payload.messages(2).any { it.text(1) == "CONTENT_ENCRYPTION" && it.number(2) == 1L } &&
                    !encryptionRequested && !crypto.active) {
                    encryptionRequested = true
                    diagnostic("encryption requested=true active=false")
                    send(1, 65642)
                }
            }
            98411 -> {
                protocolRequire(authenticated && !crypto.active, "unexpected_phase")
                val key = crypto.negotiate(payload.text(1) ?: throw CarlifeProtocolException("malformed_protobuf"))
                send(1, 65644, ProtoWriter().text(1, key).build())
            }
            98413 -> { protocolRequire(!crypto.active, "unexpected_phase"); send(1, 65646) }
            98415 -> { crypto.activate(); diagnostic("encryption active=true") }
            98311 -> {
                // A receiver that does not challenge the sender establishes the session at video
                // negotiation, as a 4.x receiver does at 98380.
                if (!authenticated && !receiver.challenges) {
                    authenticated = true
                    stage("established")
                    diagnostic("authentication absent peerMajor=$peerMajor")
                    report(CarlifeState.AUTHENTICATED)
                }
                protocolRequire(authenticated, "unexpected_phase")
                val rawWidth = payload.number(1)
                val rawHeight = payload.number(2)
                protocolRequire(rawWidth in 16L..16384L && rawHeight in 16L..16384L, "invalid_display")
                val requestedWidth = rawWidth!!.toInt()
                val requestedHeight = rawHeight!!.toInt()
                val scale = minOf(1.0, config.maxWidth.toDouble() / requestedWidth,
                    config.maxHeight.toDouble() / requestedHeight)
                width = ((requestedWidth * scale).toInt() / 2 * 2).coerceAtLeast(16)
                height = ((requestedHeight * scale).toInt() / 2 * 2).coerceAtLeast(16)
                sourceFormat()?.takeIf { it.codec == VideoCodec.H264 && it.width <= 1920 && it.height <= 1080 }?.let {
                    width = it.width
                    height = it.height
                }
                val requestedFps = payload.number(3)?.toInt() ?: 0
                fps = if (requestedFps > 0) requestedFps.coerceIn(1, config.fps) else config.fps
                encoderChannel = message.channel
                diagnostic("video_negotiated requested=${requestedWidth}x$requestedHeight selected=${width}x$height fps=$fps codec=H264")
                stage("video_negotiated")
                publishCapabilities(requestedWidth, requestedHeight)
                encoderInfo()
                report(CarlifeState.AUTHENTICATED)
            }
            98313 -> {
                protocolRequire(authenticated && width > 0, "unexpected_phase")
                streaming = true
                stage("video_requested")
                requestKeyframe()
            }
            98389 -> runCatching { Proto.parse(message.payload).text(1) }.getOrNull()
                ?.let { diagnostic("peer_code code=$it") }
            98314 -> {
                streaming = false
                stage("video_stopped")
                releaseTouches()
                report(CarlifeState.AUTHENTICATED)
            }
            425985, 425998 -> if (authenticated && width > 0) {
                runCatching {
                    val touch = CarlifeInput.touch(message.payload, message.id == 425998, width, height, origin, activeContacts)
                    val count = touch.contacts.count { it.pressed }
                    when {
                        count > previousTouchCount -> { inputDown.incrementAndGet(); diagnostic("input_down contacts=$count points=${points(touch.contacts)}") }
                        count < previousTouchCount -> { inputUp.incrementAndGet(); diagnostic("input_up contacts=$count points=${points(touch.contacts)}") }
                        else -> inputMove.incrementAndGet()
                    }
                    previousTouchCount = count
                    activeContacts = touch.contacts.filter { it.pressed }
                    host.control(touch)
                }.onFailure { diagnostic("input_ignored id=${message.id} cause=invalid_touch") }
            }
            425992 -> if (authenticated) runCatching { CarlifeInput.key(message.payload) }.getOrNull()?.let {
                diagnostic("input_key code=${it.name}")
                host.control(ControlEvent.Key(it, true, origin))
                host.control(ControlEvent.Key(it, false, origin))
            }
            // Empty keepalives and unknown extension messages are valid traffic, with no mandatory reply.
        }
    }

    private fun sendDevice() {
        if (deviceSent) return
        send(1, 65540, ProtoWriter().text(1, "Android").text(4, info.brand).text(14, info.model)
            .text(20, info.sdk).text(24, "4.0.0").build())
        deviceSent = true
        stage("device_sent")
    }

    private fun publishCapabilities(displayWidth: Int, displayHeight: Int) {
        host.capabilities(OutputCapabilities(LatencyClass.INTERACTIVE,
            listOf(DecoderSupport("avc1.64002a", true), DecoderSupport("avc1.42e01f", true)),
            mapOf(ScreenId.MAIN to DisplayInfo(displayWidth, displayHeight, 160, fps.toDouble(),
                frameSize = Viewport(width, height))), touch = 10, keys = KeyCode.entries.toSet(),
            audio = emptySet(), microphone = false, prefersH264 = true))
    }

    fun format(format: VideoFormat) {
        if (!authenticated || width == 0) return
        protocolRequire(format.codec == VideoCodec.H264, "unsupported_source_codec")
        protocolRequire(format.width <= 1920 && format.height <= 1080, "unsupported_source_size")
        if (knownFormat !== format) {
            knownFormat = format
            diagnostic("video_source codec=${format.codec} size=${format.width}x${format.height} fpsHint=${format.fpsHint ?: 0.0}")
        }
        if (width != format.width || height != format.height) {
            width = format.width
            height = format.height
            streaming = false
            stage("video_reconfiguring")
            encoderInfo()
            requestKeyframe()
        }
    }

    private fun encoderInfo() {
        send(encoderChannel, 65544, ProtoWriter().number(1, width.toLong()).number(2, height.toLong())
            .number(3, fps.toLong()).build())
    }

    fun video(payload: ByteArray): Boolean {
        if (!active.get() || !authenticated || !streaming) return false
        if (!send(2, 131073, payload)) return false
        if (sent.incrementAndGet() == 1L) {
            stage("streaming")
            report(CarlifeState.STREAMING)
        }
        return true
    }

    fun tick() {
        if (!version || !active.get()) return
        if (System.nanoTime() - lastSent.get() > 1_000_000_000L) {
            send(if (channels[2] != null) 2 else 1, 131074)
        }
        if (streaming) {
            if (System.nanoTime() - lastKeyframeRequest >= 1_500_000_000L) requestKeyframe()
            if (sent.get() > 0) report(CarlifeState.STREAMING)
        }
        val now = System.nanoTime()
        if (now - lastSummary >= 10_000_000_000L) {
            lastSummary = now
            diagnostic("summary rx=${received.get()} sent=${sent.get()} keyframeRequests=${keyframeRequests.get()} " +
                "inputDown=${inputDown.get()} inputUp=${inputUp.get()} inputMove=${inputMove.get()} " +
                "lastRxMs=${(now - lastReceived.get()) / 1_000_000} lastTxMs=${(now - lastSent.get()) / 1_000_000} " +
                "encryption=${crypto.active} ${mediaCounters()}")
        }
    }

    fun requestKeyframe() {
        lastKeyframeRequest = System.nanoTime()
        keyframeRequests.incrementAndGet()
        host.control(ControlEvent.KeyFrameRequest(ScreenId.MAIN, origin))
    }

    private fun send(channel: Int, id: Int, payload: ByteArray = byteArrayOf()): Boolean {
        if (!active.get()) return false
        val connection = channels[channel] ?: return false
        val message = crypto.transform(CarlifeMessage(channel, id, payload), true)
        if (!connection.write(message)) return false
        lastSent.set(System.nanoTime())
        if (id != 131073 && id != 131074) diagnostic("tx channel=$channel id=$id bytes=${message.payload.size}")
        return true
    }

    fun report(state: CarlifeState, reason: String? = null) {
        status(CarlifeStatus(state, peer, width, height, fps, sent.get(), reason))
    }

    fun releaseTouches() {
        if (previousTouchCount > 0) { inputUp.incrementAndGet(); previousTouchCount = 0 }
        val released = activeContacts.map { it.copy(pressed = false) }
        activeContacts = emptyList()
        if (released.isNotEmpty()) diagnostic("input_up contacts=0 points=${points(released)} cause=session_release")
        host.control(ControlEvent.Touch(ScreenId.MAIN, released, origin))
    }

    private fun points(contacts: List<TouchContact>): String = contacts.joinToString(";") {
        java.lang.String.format(java.util.Locale.ROOT, "%d:%.4f,%.4f,%s", it.id, it.x, it.y, if (it.pressed) "down" else "up")
    }

    fun transportFailure(now: Long): Pair<Int, String>? {
        channels.values.firstNotNullOfOrNull { it.stalledChannel(now) }?.let { return it to "write_timeout" }
        return if (!authenticated && now - createdAt >= 30_000_000_000L) 1 to "handshake_timeout" else null
    }

    fun close(cause: String = "local_close") {
        if (active.compareAndSet(true, false)) {
            val root = rootFailure.get()
            val endedCause = root?.cause ?: cause
            val rootChannel = root?.channel ?: 0
            val affected = channels.keys.sorted().joinToString(",")
            diagnostic("session_closed cause=$endedCause originChannel=$rootChannel affected=$affected sent=${sent.get()} ${mediaCounters()}")
            channels.values.toSet().forEach {
                diagnostic("transport_closed peer=$peer channel=${it.channel} transport=${if (it.usb) "usb" else "tcp"} cause=$endedCause originChannel=$rootChannel")
                it.close()
            }
            channels.clear()
            releaseTouches()
            host.capabilities(OutputCapabilities(LatencyClass.INTERACTIVE, emptyList(), emptyMap(), 0,
                emptySet(), emptySet(), false))
        }
    }

    fun failure(channel: Int, id: Int?, cause: String, errorClass: String) {
        if (!active.get()) return
        if (rootFailure.compareAndSet(null, Failure(channel, id, cause))) {
            val transport = if (channels[channel]?.usb == true) "usb" else "tcp"
            diagnostic("error transport=$transport channel=$channel id=${id ?: 0} cause=$cause class=$errorClass " +
                "affected=${channels.keys.sorted().joinToString(",")}")
        }
    }
}
