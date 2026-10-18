// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-18

package org.openxplay.stream.clink.trace

import org.openxplay.stream.clink.protocol.*

public sealed interface ClinkTraceEvent {
    public val type: String get() = when (this) {
        is Rtsp -> "Rtsp"; is Frame -> "Frame"; is Socket -> "Socket"; is Negotiation -> "Negotiation"
        is Media -> "Media"; is Video -> "Video"; is Input -> "Input"; is Error -> "Error"; is Link -> "Link"
    }
    public val atNanos: Long
    public val session: Long
    public data class Rtsp(override val atNanos: Long, override val session: Long, val direction: String,
        val startLine: String, val headers: Map<String, String>, val body: String) : ClinkTraceEvent
    public data class Frame(override val atNanos: Long, override val session: Long, val direction: String,
        val port: Int, val service: String, val category: Int, val method: Int, val kind: Int,
        val sequence: Long, val bodySize: Int, val fields: Map<Int, ClinkTraceField>) : ClinkTraceEvent
    public data class Socket(override val atNanos: Long, override val session: Long, val service: String,
        val event: String, val remoteAddress: String?, val port: Int, val errorClass: String? = null) : ClinkTraceEvent
    public data class Negotiation(override val atNanos: Long, override val session: Long, val mode: String,
        val decoders: List<String>, val audio: Boolean, val rtpPort: Int, val display: String) : ClinkTraceEvent
    public data class Media(override val atNanos: Long, override val session: Long, val liveFrames: Long,
        val placeholderFrames: Long, val droppedFrames: Long, val keyframes: Long, val videoBytes: Long,
        val rtpPackets: Long, val rtpBytes: Long, val sendStalls: Long, val audioFrames: Long,
        val queueOverflows: Long) : ClinkTraceEvent
    public data class Video(override val atNanos: Long, override val session: Long, val event: String,
        val source: String = "", val codec: String? = null, val width: Int? = null, val height: Int? = null,
        val sps: List<String> = emptyList(), val pps: List<String> = emptyList()) : ClinkTraceEvent
    public data class Input(override val atNanos: Long, override val session: Long, val kind: String,
        val coordinates: List<Pair<Double, Double>>) : ClinkTraceEvent
    public data class Error(override val atNanos: Long, override val session: Long, val code: String,
        val exceptionClass: String, val location: String) : ClinkTraceEvent
    public data class Link(override val atNanos: Long, override val session: Long, val stage: String,
        val fields: Map<String, String> = emptyMap()) : ClinkTraceEvent
}

public sealed interface ClinkTraceField {
    public data class Number(public val value: ULong) : ClinkTraceField
    public data class Bytes(public val length: Int, public val preview: String) : ClinkTraceField
}

public object ClinkTraceFields {
    /** Runs on session workers before the event reaches [emit]; any failure yields no fields, never an exception. */
    public fun decode(service: String, body: ByteArray, decrypted: Boolean = true): Map<Int, ClinkTraceField> {
        if (!decrypted || service == "auth" || service == "cert") return emptyMap()
        return try {
            val fields = Fields.decode(body)
            fields.numbers.associateWith { id -> when (val value = fields[id]) {
                is FieldValue.Varint -> ClinkTraceField.Number(value.value)
                is FieldValue.Bytes -> {
                    val bytes = value.value; val preview = bytes.take(64)
                    val text = if (preview.all { it.toInt() in 32..126 }) preview.map { it.toInt().toChar() }.joinToString("")
                        else preview.joinToString("") { "%02x".format(it.toInt() and 255) }
                    ClinkTraceField.Bytes(bytes.size, text)
                }
                null -> ClinkTraceField.Bytes(0, "")
            } }
        } catch (_: Exception) { emptyMap() }
    }
}
