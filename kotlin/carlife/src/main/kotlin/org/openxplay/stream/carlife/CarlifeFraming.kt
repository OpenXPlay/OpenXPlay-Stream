// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-11-02

package org.openxplay.stream.carlife

import java.io.InputStream
import java.io.OutputStream
import java.nio.ByteBuffer

public data class CarlifeMessage(public val channel: Int, public val id: Int, public val payload: ByteArray = byteArrayOf())

/** CarLife framing, independent of USB, TCP and the projection model. */
public object CarlifeFraming {
    public const val MAX_MESSAGE_BYTES: Int = 67_108_864

    public fun headerSize(channel: Int): Int {
        protocolRequire(channel in 1..7, "invalid_channel", channel = channel)
        return if (channel == 1 || channel == 6) 8 else 12
    }

    public fun encode(message: CarlifeMessage): ByteArray {
        val header = headerSize(message.channel)
        protocolRequire(message.payload.size <= MAX_MESSAGE_BYTES - header, "invalid_payload_length")
        if (header == 8) protocolRequire(message.payload.size <= 65535, "invalid_payload_length")
        return ByteBuffer.allocate(header + message.payload.size).apply {
            if (header == 8) { putShort(message.payload.size.toShort()); putShort(0) }
            else { putInt(message.payload.size); putInt(0) }
            putInt(message.id)
            put(message.payload)
        }.array()
    }

    public fun read(input: InputStream, channel: Int): CarlifeMessage? {
        val size = headerSize(channel)
        val bytes = exact(input, size, true, "truncated_header", channel = channel) ?: return null
        val header = ByteBuffer.wrap(bytes)
        val length = if (size == 8) header.short.toInt() and 65535 else header.int
        if (size == 8) header.short else header.int
        val id = header.int
        protocolRequire(length in 0..MAX_MESSAGE_BYTES - size, "invalid_payload_length", id, channel)
        return CarlifeMessage(channel, id, exact(input, length, false, "truncated_payload", id, channel)!!)
    }

    public fun readUsb(input: InputStream): CarlifeMessage? {
        val outer = ByteBuffer.wrap(exact(input, 8, true, "truncated_header") ?: return null)
        val channel = outer.int
        val total = outer.int
        val size = headerSize(channel)
        protocolRequire(total in size..MAX_MESSAGE_BYTES, "invalid_payload_length", channel = channel)
        // Read and validate the inner length before allocating a potentially large payload.
        val header = exact(input, size, false, "truncated_header", channel = channel)!!
        val buffer = ByteBuffer.wrap(header)
        val payloadLength = if (size == 8) buffer.short.toInt() and 65535 else buffer.int
        if (size == 8) buffer.short else buffer.int
        val id = buffer.int
        protocolRequire(payloadLength == total - size, "usb_length_mismatch", id, channel)
        return CarlifeMessage(channel, id, exact(input, payloadLength, false, "truncated_payload", id, channel)!!)
    }

    public fun write(output: OutputStream, message: CarlifeMessage, usb: Boolean = false): Unit {
        val body = encode(message)
        if (usb) {
            output.write(ByteBuffer.allocate(8).putInt(message.channel).putInt(body.size).array())
            var offset = 0
            while (offset < body.size) {
                val count = minOf(16_384, body.size - offset)
                output.write(body, offset, count)
                offset += count
            }
        } else output.write(body)
        output.flush()
    }

    private fun exact(input: InputStream, count: Int, eofAllowed: Boolean, code: String, id: Int? = null, channel: Int? = null): ByteArray? {
        val bytes = ByteArray(count)
        var offset = 0
        while (offset < count) {
            val read = input.read(bytes, offset, count - offset)
            if (read < 0) {
                if (offset == 0 && eofAllowed) return null
                throw CarlifeProtocolException(code, id, channel)
            }
            if (read == 0) {
                val one = input.read()
                if (one < 0) throw CarlifeProtocolException(code, id, channel)
                bytes[offset++] = one.toByte()
            } else offset += read
        }
        return bytes
    }
}
