// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.web

import java.io.ByteArrayOutputStream
import java.io.EOFException
import java.io.InputStream
import java.io.OutputStream
import java.nio.ByteBuffer
import java.nio.charset.CodingErrorAction

internal class WebSocketFailure(val code: Int, message: String) : Exception(message)
internal data class WebSocketMessage(val opcode: Int, val payload: ByteArray, val stampPong: Boolean = false)

/** Stateful RFC 6455 reader; control frames may interrupt a fragmented message. */
internal class WebSocketReader(private val input: InputStream, private val maxMessageBytes: Int, private val onFrame: () -> Unit = {}) {
    private var fragmentedOpcode = 0
    private var fragments: ByteArrayOutputStream? = null

    fun read(): WebSocketMessage {
        while (true) {
            val first = byte(); val second = byte()
            val fin = first and 128 != 0; val opcode = first and 15
            check(first and 112 == 0 && opcode in setOf(0, 1, 2, 8, 9, 10), 1002, "Invalid opcode or RSV")
            check(second and 128 != 0, 1002, "Client frames must be masked")
            var length = (second and 127).toLong()
            when (length) {
                126L -> { length = (byte().toLong() shl 8) or byte().toLong(); check(length >= 126, 1002, "Noncanonical length") }
                127L -> {
                    val high = byte(); check(high and 128 == 0, 1002, "Invalid length")
                    length = high.toLong()
                    repeat(7) { length = (length shl 8) or byte().toLong() }
                    check(length >= 65536, 1002, "Noncanonical length")
                }
            }
            val control = opcode >= 8
            check(!control || fin && length <= 125, 1002, "Invalid control frame")
            check(length <= maxMessageBytes && (control || length + (fragments?.size() ?: 0) <= maxMessageBytes), 1009, "Message too large")
            if (!control) {
                check(if (opcode == 0) fragmentedOpcode != 0 else fragmentedOpcode == 0, 1002, "Invalid continuation")
            }
            val mask = ByteArray(4) { byte().toByte() }
            val payload = ByteArray(length.toInt())
            var offset = 0
            while (offset < payload.size) {
                val n = input.read(payload, offset, payload.size - offset)
                if (n < 0) throw EOFException()
                offset += n
            }
            payload.indices.forEach { payload[it] = (payload[it].toInt() xor mask[it % 4].toInt()).toByte() }
            onFrame()
            if (control) {
                if (opcode == 8) validateClose(payload)
                return WebSocketMessage(opcode, payload)
            }
            if (opcode != 0 && fin) {
                if (opcode == 1) utf8(payload)
                return WebSocketMessage(opcode, payload)
            }
            if (opcode != 0) { fragmentedOpcode = opcode; fragments = ByteArrayOutputStream() }
            fragments!!.write(payload)
            if (fin) {
                val data = fragments!!.toByteArray(); val kind = fragmentedOpcode
                fragments = null; fragmentedOpcode = 0
                if (kind == 1) utf8(data)
                return WebSocketMessage(kind, data)
            }
        }
    }

    private fun byte(): Int = input.read().also { if (it < 0) throw EOFException() }
    private fun check(value: Boolean, code: Int, message: String) { if (!value) throw WebSocketFailure(code, message) }
    private fun validateClose(data: ByteArray) {
        check(data.size != 1, 1002, "Invalid close payload")
        if (data.size >= 2) {
            val code = ((data[0].toInt() and 255) shl 8) or (data[1].toInt() and 255)
            check(code in 1000..1014 && code !in setOf(1004, 1005, 1006) || code in 3000..4999, 1002, "Invalid close code")
            utf8(data.copyOfRange(2, data.size))
        }
    }
}

internal fun utf8(data: ByteArray): String = try {
    Charsets.UTF_8.newDecoder().onMalformedInput(CodingErrorAction.REPORT)
        .onUnmappableCharacter(CodingErrorAction.REPORT).decode(ByteBuffer.wrap(data)).toString()
} catch (_: java.nio.charset.CharacterCodingException) { throw WebSocketFailure(1007, "Invalid UTF-8") }

/** Writes separate buffers so the media payload is borrowed directly from core. */
internal fun writeWebSocket(output: OutputStream, opcode: Int, vararg parts: ByteArray) {
    val size = parts.sumOf { it.size.toLong() }
    output.write(128 or opcode)
    when {
        size < 126 -> output.write(size.toInt())
        size <= 65535 -> { output.write(126); output.write((size ushr 8).toInt()); output.write(size.toInt()) }
        else -> { output.write(127); for (shift in 56 downTo 0 step 8) output.write((size ushr shift).toInt()) }
    }
    parts.forEach { output.write(it) }
    output.flush()
}

internal fun closePayload(code: Int): ByteArray = byteArrayOf((code ushr 8).toByte(), code.toByte())
