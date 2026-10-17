// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.wire

import java.util.concurrent.Semaphore
import org.openxplay.stream.core.*
import org.openxplay.stream.core.wire.*

/** One reader and writer, bounded control messages and borrowed media queues. */
internal class WireConnection(
    transport: WireTransport,
    private val hello: WireMessage.Hello,
    private val initial: List<WireMessage>,
    private val maxMessages: Int,
    private val maxBytes: Long,
    private val maxDurationUs: Long,
    private val receive: (WireMessage, Origin) -> Unit,
    private val recovery: (ScreenId, Origin) -> Unit,
    private val failure: (Throwable) -> Unit,
    private val expectedPeerName: String? = null,
) {
    private val pending = transport.prepare()
    private val lock = Any()
    private val gate = Any()
    private val wake = Semaphore(0)
    private val messages = ArrayDeque<WireMessage>()
    private val queues = linkedMapOf<ScreenId, FrameQueue>()
    private sealed interface AudioEvent {
        data class Format(val message: WireMessage.AudioFormatMessage) : AudioEvent
        data class Packet(val packet: AudioPacket) : AudioEvent
    }
    private val audio = ArrayDeque<AudioEvent>()
    private val audioFormats = mutableSetOf<Int>()
    private val audioSequences = mutableMapOf<Int, UInt>()
    private var audioBytes = 0L
    private val requests = linkedSetOf<ScreenId>()
    private val sequences = mutableMapOf<ScreenId, UInt>()
    private var rejecting = false
    private var state: SessionState? = null
    private val formats = mutableSetOf<ScreenId>()
    @Volatile private var peerContext = false
    private val context = linkedMapOf<ContextTopic, ContextUpdate>()
    @Volatile private var active = true
    @Volatile private var origin = Origin(hello.name, hello.client)

    init { require(maxMessages > 0 && maxBytes > 0 && maxDurationUs >= 0) }

    fun start() {
        thread("reader") {
            try {
                val streams = pending.open()
                StreamWireCodec.write(streams.output, hello)
                streams.output.flush()
                val remote = StreamWireCodec.read(streams.input) as? WireMessage.Hello
                    ?: throw WireFormatException("Hello required")
                if (remote.role == hello.role) throw WireFormatException("Wrong peer role")
                if (expectedPeerName != null && remote.name != expectedPeerName)
                    throw WireFormatException("Consumer hello name must match WireOutput.id")
                peerContext = ContextNegotiation.supports(remote)
                origin = Origin(remote.name, ContextNegotiation.clientId(remote))
                thread("writer") { write(streams) }
                while (active) {
                    val message = StreamWireCodec.read(streams.input) ?: error("Peer closed")
                    if (message is WireMessage.Unknown) continue
                    if (message is WireMessage.Hello) throw WireFormatException("Duplicate hello")
                    if (message is WireMessage.Error) error("Peer error ${message.code}: ${message.message}")
                    if (message is WireMessage.Client) origin = origin.copy(clientId = message.client)
                    synchronized(gate) { if (active) receive(message, origin) }
                }
            } catch (e: Exception) { fail(e) }
        }
    }

    // The writer owns I/O; repeated context updates replace four bounded slots.
    fun context(update: ContextUpdate): Unit = synchronized(lock) {
        if (!active || rejecting) return
        context[update.topic] = update
        wake.release()
    }

    fun enqueue(message: WireMessage): Boolean = enqueue(listOf(message))

    fun enqueue(batch: List<WireMessage>): Boolean = synchronized(lock) {
        if (!active || rejecting) return false
        if (messages.size + batch.size > maxMessages) { rejecting = true; failAsync(IllegalStateException("Control queue overflow")); return false }
        messages.addAll(batch); wake.release(); true
    }

    fun session(next: SessionState): Unit = synchronized(lock) {
        if (!active || rejecting || state?.let { next.id.value < it.id.value } == true) return
        if (state?.id != next.id || next.phase == SessionPhase.ENDED) {
            queues.values.forEach(FrameQueue::clear); formats.clear()
            audio.clear(); audioBytes = 0; audioFormats.clear()
        }
        state = next; enqueue(WireMessage.Session(next)); Unit
    }
    fun format(screen: ScreenId, format: VideoFormat?): Unit = synchronized(lock) {
        if (!active || rejecting) return
        queues.remove(screen)?.clear()
        if (format == null) formats.remove(screen) else formats.add(screen)
        enqueue(WireMessage.VideoFormatMessage(screen, format)); Unit
    }
    fun frame(frame: VideoFrame) = synchronized(lock) {
        if (!active || rejecting || state?.id != frame.session || state?.phase == SessionPhase.ENDED || frame.screen !in formats) return
        val queue = queues.getOrPut(frame.screen) {
            FrameQueue(maxDurationUs, maxBytes) { requests.add(frame.screen); wake.release() }
        }
        queue.offer(frame)
        if (queues.values.sumOf { it.queuedBytes } > maxBytes) {
            queues.forEach { (screen, q) ->
                q.clear()
                requests.add(screen)
            }
            queue.offer(frame)
        }
        wake.release()
    }
    fun audioFormat(stream: Int, format: AudioFormat?): Unit = synchronized(lock) {
        if (!active || rejecting) return
        if (format == null) audioFormats.remove(stream) else audioFormats.add(stream)
        if (audio.count { it is AudioEvent.Format } >= maxMessages) {
            rejecting = true; failAsync(IllegalStateException("Audio format queue overflow")); return
        }
        // Format markers share the packet FIFO and cannot overtake accepted audio.
        audio.addLast(AudioEvent.Format(WireMessage.AudioFormatMessage(stream, format))); wake.release(); Unit
    }
    fun audioPacket(packet: AudioPacket): Unit = synchronized(lock) {
        if (!active || rejecting || state?.id != packet.session || state?.phase == SessionPhase.ENDED || packet.stream !in audioFormats) return
        audio.addLast(AudioEvent.Packet(packet)); audioBytes += packet.data.size
        // Audio drops oldest packets independently of video keyframe recovery.
        val oldestPtsUs = if (packet.ptsUs < Long.MIN_VALUE + 500_000) Long.MIN_VALUE else packet.ptsUs - 500_000
        while (audio.isNotEmpty() && (audioBytes > maxBytes || audio.count { it is AudioEvent.Packet } > maxMessages ||
            audio.any { it is AudioEvent.Packet && it.packet.stream == packet.stream && it.packet.ptsUs < oldestPtsUs })) {
            val oldest = audio.first { it is AudioEvent.Packet } as AudioEvent.Packet
            audio.remove(oldest); audioBytes -= oldest.packet.data.size
        }
        wake.release()
    }
    fun close() {
        synchronized(gate) { active = false }
        synchronized(lock) { messages.clear(); context.clear(); queues.clear(); requests.clear(); audio.clear(); audioBytes = 0 }
        wake.release()
        thread("close") { pending.close() }
    }
    private fun failAsync(e: Exception) { thread("failure") { fail(e) } }
    private fun fail(e: Throwable) {
        synchronized(gate) {
            if (!active) return
            active = false
            runCatching { failure(e) }
        }
        synchronized(lock) { messages.clear(); context.clear(); queues.clear(); requests.clear(); audio.clear(); audioBytes = 0 }
        wake.release(); pending.close()
    }
    private fun write(streams: WireStreams) {
        try {
            initial.forEach { StreamWireCodec.write(streams.output, it) }
            streams.output.flush()
            while (active) {
                wake.acquire(); wake.drainPermits()
                val recover = synchronized(lock) { requests.toList().also { requests.clear() } }
                recover.forEach { screen -> synchronized(gate) { if (active) recovery(screen, origin) } }
                while (active) {
                    val message = synchronized(lock) {
                        if (!peerContext) context.clear()
                        if (messages.isNotEmpty()) messages.removeFirst()
                        else if (context.isNotEmpty()) {
                            if (peerContext) context.entries.first().let { entry ->
                                WireMessage.Context(entry.value).also { context.remove(entry.key) }
                            } else { context.clear(); null }
                        }
                        else if (audio.isNotEmpty()) {
                            when (val item = audio.removeFirst()) {
                                is AudioEvent.Format -> item.message
                                is AudioEvent.Packet -> {
                                    val packet = item.packet; audioBytes -= packet.data.size
                                    val seq = audioSequences[packet.stream] ?: 0u; audioSequences[packet.stream] = seq + 1u
                                    WireMessage.Binary(BinaryMessage(BinaryHeader(BinaryKind.AUDIO, packet.stream, 0,
                                        seq, packet.ptsUs, System.nanoTime() / 1000), packet.data))
                                }
                            }
                        } else queues.values.firstNotNullOfOrNull { it.poll() }?.let { queued ->
                            val f = queued.frame
                            val seq = sequences[f.screen] ?: 0u; sequences[f.screen] = seq + 1u
                            WireMessage.Binary(BinaryMessage(BinaryHeader(BinaryKind.VIDEO, f.screen.value,
                                (if (f.keyFrame) OxpWebCodec.KEYFRAME else 0) or (if (queued.gap) OxpWebCodec.GAP else 0),
                                seq, f.ptsUs, f.receivedAtNanos / 1000), f.data))
                        }
                    } ?: break
                    StreamWireCodec.write(streams.output, message); streams.output.flush()
                }
            }
        } catch (e: Exception) { fail(e) }
    }
    private fun thread(role: String, action: () -> Unit) {
        Thread(action, "oxp-wire-${hello.name}-$role").apply { isDaemon = true; start() }
    }
}
