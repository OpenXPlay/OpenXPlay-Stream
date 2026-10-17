// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.devrelay

import java.io.PrintStream
import java.util.concurrent.LinkedBlockingQueue
import org.openxplay.stream.core.*
import org.openxplay.stream.replay.MicSummary

/** Only the writer performs I/O; interface callbacks enqueue model values. */
internal class EventLog(private val stdout: PrintStream) : AutoCloseable {
    private data class Entry(val fields: Map<String, Any?>)
    private val queue = LinkedBlockingQueue<Entry>()
    private val end = Entry(emptyMap())
    private var closed = false
    private val writer = Thread({
        while (true) {
            val entry = queue.take()
            if (entry === end) break
            stdout.println(json(entry.fields))
            stdout.flush()
        }
    }, "devrelay-log").apply { isDaemon = true; start() }

    @Synchronized
    fun event(type: String, vararg fields: Pair<String, Any?>) {
        if (!closed) queue.add(Entry(linkedMapOf("event" to type, *fields)))
    }

    fun error(component: String, failure: Throwable) = event("error", "component" to component,
        "class" to failure.javaClass.name, "message" to failure.message)

    fun session(state: SessionState) {
        event("session", "id" to state.id.value, "phase" to state.phase, "reason" to state.reason)
        if (state.phase == SessionPhase.ENDED && state.reason != null && state.reason != "End of file") {
            event("error", "component" to "input", "message" to state.reason)
        }
    }

    fun audioFormat(stream: Int, format: AudioFormat?) = event("audio-format", "stream" to stream,
        "format" to format?.let { mapOf("role" to it.role, "codec" to it.codec,
            "sampleRate" to it.sampleRate, "channels" to it.channels,
            "config" to it.config?.map { byte -> byte.toInt() and 255 }) })

    fun audioStats(stream: Int, packets: Long, bytes: Long) = event("audio-stats",
        "stream" to stream, "packets" to packets, "bytes" to bytes)

    fun microphone(stream: Int, request: MicRequest?) = event("microphone", "stream" to stream,
        "request" to request?.let { mapOf("codec" to it.codec, "sampleRate" to it.sampleRate,
            "channels" to it.channels, "frameMs" to it.frameMs) })

    fun microphoneStats(stats: MicSummary) = event("mic-stats", "stream" to 1, "packets" to stats.packets,
        "origin" to origin(stats.origin), "rmsDbfs" to stats.rmsDbfs)

    fun control(event: ControlEvent) {
        if (event is ControlEvent.MicPacket) return
        val fields = when (event) {
            is ControlEvent.Touch -> arrayOf("type" to "touch", "screen" to event.screen.value,
                "contacts" to event.contacts.map { mapOf("id" to it.id, "x" to it.x, "y" to it.y, "pressed" to it.pressed) })
            is ControlEvent.Key -> arrayOf("type" to "key", "code" to event.code, "down" to event.down)
            is ControlEvent.KeyFrameRequest -> arrayOf("type" to "keyframe-request", "screen" to event.screen.value)
            is ControlEvent.MicPacket -> arrayOf("type" to "microphone", "stream" to event.stream,
                "bytes" to event.data.size, "ptsUs" to event.ptsUs)
        }
        event("control", "origin" to origin(event.origin), *fields)
    }

    fun capabilities(id: String, caps: OutputCapabilities) = event("capabilities", "output" to id,
        "latency" to caps.latency,
        "decoders" to caps.decoders.map { mapOf("codecString" to it.codecString, "hardware" to it.hardware) },
        "displays" to caps.displays.map { (screen, display) -> mapOf(
            "screen" to screen.value, "widthPx" to display.widthPx, "heightPx" to display.heightPx,
            "densityDpi" to display.densityDpi, "refreshHz" to display.refreshHz,
            "safeArea" to mapOf("left" to display.safeArea.left, "top" to display.safeArea.top,
                "right" to display.safeArea.right, "bottom" to display.safeArea.bottom)) },
        "touch" to caps.touch, "keys" to caps.keys, "audio" to caps.audio, "microphone" to caps.microphone)

    fun stats(id: String, stats: OutputStats) = event("stats", "output" to id, "origin" to origin(stats.origin),
        "screen" to stats.screen.value, "decodeTimeUs" to stats.decodeTimeUs, "decodeQueue" to stats.decodeQueue,
        "dropped" to stats.dropped, "rendered" to stats.rendered, "endToEndLatencyUs" to stats.endToEndLatencyUs,
        "audioBufferedUs" to stats.audioBufferedUs, "audioUnderruns" to stats.audioUnderruns)

    override fun close() {
        synchronized(this) {
            if (closed) return
            event("shutdown")
            closed = true
            queue.add(end)
        }
        writer.join()
    }
}

private fun origin(value: Origin): Map<String, String> = mapOf("outputId" to value.outputId, "clientId" to value.clientId)

// Serialization JSON is an implementation dependency of web-server, not a compile API.
private fun json(value: Any?): String = when (value) {
    null -> "null"
    is String -> buildString {
        append('"')
        for (c in value) when (c) {
            '"' -> append("\\\"")
            '\\' -> append("\\\\")
            '\n' -> append("\\n")
            '\r' -> append("\\r")
            '\t' -> append("\\t")
            else -> if (c.code < 32 || c.isSurrogate()) append("\\u%04x".format(c.code)) else append(c)
        }
        append('"')
    }
    is Boolean, is Number -> value.toString()
    is Enum<*> -> json(value.name)
    is Map<*, *> -> value.entries.joinToString(",", "{", "}") { json(it.key.toString()) + ":" + json(it.value) }
    is Iterable<*> -> value.joinToString(",", "[", "]") { json(it) }
    else -> error("Unsupported log value: ${value.javaClass.name}")
}
