// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.clink.protocol

import java.nio.ByteBuffer

/** Immutable wire frame. Time and sequence are unsigned 32-bit values represented as Long. */
public class Frame(
    public val sequence: Long,
    public val unixSeconds: Long,
    public val source: Int,
    public val format: Int,
    public val kind: Int,
    public val category: Int,
    public val method: Int,
    body: ByteArray = byteArrayOf(),
) {
    private val payload: ByteArray = body.copyOf()
    public val body: ByteArray get() = payload.copyOf()
    init {
        require(sequence in 0..0xffffffffL && unixSeconds in 0..0xffffffffL)
        require(source in 0..1 && format in 0..3 && kind in 0..3)
        require(category in 0..255 && method in 0..65535)
        require(payload.size <= MAX_BYTES - HEADER_BYTES)
    }
    public fun encode(): ByteArray {
        val out = ByteArray(HEADER_BYTES + payload.size)
        ByteBuffer.wrap(out).putInt(out.size).putInt(sequence.toInt()).putInt(unixSeconds.toInt())
            .put((source or (format shl 1) or (kind shl 3)).toByte()).put(category.toByte())
            .putShort(method.toShort()).putShort(0).putShort(crc16(out.copyOfRange(0, 18)).toShort())
            .put(payload)
        return out
    }
    public companion object {
        public const val HEADER_BYTES: Int = 20
        public const val MAX_BYTES: Int = 4 * 1024 * 1024
        /** CRC-16/ARC: initial zero, reflected 0xA001, no final XOR. */
        public fun crc16(bytes: ByteArray): Int {
            var crc = 0
            for (b in bytes) {
                crc = crc xor (b.toInt() and 255)
                repeat(8) { crc = if (crc and 1 != 0) (crc ushr 1) xor 0xa001 else crc ushr 1 }
            }
            return crc
        }
        public fun decode(bytes: ByteArray): Frame {
            require(bytes.size in HEADER_BYTES..MAX_BYTES) { "invalid_packet_length" }
            val b = ByteBuffer.wrap(bytes)
            require(b.int.toLong() and 0xffffffffL == bytes.size.toLong()) { "invalid_packet_length" }
            val sequence = b.int.toLong() and 0xffffffffL
            val time = b.int.toLong() and 0xffffffffL
            val flags = b.get().toInt() and 255
            val category = b.get().toInt() and 255
            val method = b.short.toInt() and 65535
            require(flags and 0xe0 == 0 && b.short.toInt() == 0) { "invalid_packet_header" }
            require((b.short.toInt() and 65535) == crc16(bytes.copyOfRange(0, 18))) { "invalid_packet_crc" }
            return Frame(sequence, time, flags and 1, (flags ushr 1) and 3, (flags ushr 3) and 3,
                category, method, bytes.copyOfRange(HEADER_BYTES, bytes.size))
        }
    }
}

/** One stream, confined to its transport thread. Reset after a decoding failure. */
public class FrameReader {
    private var buffer: ByteArray = byteArrayOf()
    public val pendingBytes: Int get() = buffer.size
    public fun reset() { buffer = byteArrayOf() }
    public fun feed(bytes: ByteArray): List<Frame> = feed(bytes, null)
    /** Skips invalid complete frames when supplied; invalid lengths still require channel closure. */
    internal fun feed(bytes: ByteArray, malformed: ((Exception) -> Unit)?): List<Frame> {
        require(bytes.size <= Frame.MAX_BYTES && buffer.size.toLong() + bytes.size <= Frame.MAX_BYTES + 65536L)
        buffer += bytes
        val out = mutableListOf<Frame>()
        var offset = 0
        while (buffer.size - offset >= 4) {
            val size = ByteBuffer.wrap(buffer, offset, 4).int
            require(size in Frame.HEADER_BYTES..Frame.MAX_BYTES) { "invalid_packet_length" }
            if (buffer.size - offset < size) break
            try { out += Frame.decode(buffer.copyOfRange(offset, offset + size)) }
            catch (failure: Exception) { if (malformed == null) throw failure else malformed(failure) }
            offset += size
        }
        buffer = buffer.copyOfRange(offset, buffer.size)
        return out
    }
}
