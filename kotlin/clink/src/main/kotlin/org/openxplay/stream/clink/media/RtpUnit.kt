// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

package org.openxplay.stream.clink.media

/**
 * The output of one muxer call as RFC 4571 framed RTP packets (two-byte length, RTP header, up to seven
 * TS packets) in one reusable buffer, so packetizing allocates nothing per packet.
 *
 * Bounded: a call producing more than [maxBytes] marks the unit [overflowed] and appends nothing more.
 * On 2026-10-15 one muxer call produced hundreds of thousands of TS packets (a timestamp jump of hours
 * filled by PCR packets); retaining all of them, then a second copy as RTP packets, exhausted the heap
 * in RtpPacketizer.packetize. Not thread-safe: [RtpSender] hands it between threads.
 */
internal class RtpUnit(val maxBytes: Int = MAX_BYTES) {
    var bytes: ByteArray = ByteArray(minOf(INITIAL_BYTES, maxBytes))
        private set
    var size: Int = 0
        private set
    var packets: Int = 0
        private set
    var overflowed: Boolean = false
        private set
    var firstSequence: Int = 0
        private set
    private var timestamp = 0
    private var ssrc = 0
    private var open = -1
    private var grouped = 0
    init { require(maxBytes >= FRAMED_PACKET_BYTES) }

    fun clear(timestamp: Int, firstSequence: Int, ssrc: Int) {
        // A rare huge unit (a large IDR) must not pin its buffer for the rest of the session.
        if (bytes.size > RETAINED_BYTES) bytes = ByteArray(INITIAL_BYTES)
        size = 0; packets = 0; overflowed = false; open = -1; grouped = 0
        this.timestamp = timestamp; this.firstSequence = firstSequence and 0xffff; this.ssrc = ssrc
    }

    fun append(ts: ByteArray) {
        if (overflowed) return
        val opening = open < 0 || grouped == 7
        val needed = size.toLong() + TS_BYTES + if (opening) HEADER_BYTES else 0
        if (needed > maxBytes) { overflowed = true; return }
        if (needed > bytes.size) grow(needed.toInt())
        val b = bytes
        if (opening) {
            open = size; grouped = 0
            val sequence = (firstSequence + packets) and 0xffff
            b[size + 2] = 0x80.toByte(); b[size + 3] = 33
            b[size + 4] = (sequence ushr 8).toByte(); b[size + 5] = sequence.toByte()
            putInt(b, size + 6, timestamp); putInt(b, size + 10, ssrc)
            size += HEADER_BYTES; packets++
        }
        ts.copyInto(b, size, 0, TS_BYTES)
        size += TS_BYTES; grouped++
        val length = 12 + grouped * TS_BYTES
        b[open] = (length ushr 8).toByte(); b[open + 1] = length.toByte()
    }

    /** Visits each framed packet (length prefix included) in order; stops when [action] returns false. */
    inline fun forEachPacket(action: (bytes: ByteArray, offset: Int, length: Int) -> Boolean) {
        val b = bytes
        var offset = 0
        while (offset < size) {
            val length = framedLength(b, offset)
            if (!action(b, offset, length)) return
            offset += length
        }
    }

    private fun grow(needed: Int) {
        var capacity = bytes.size.toLong()
        while (capacity < needed) capacity *= 2
        bytes = bytes.copyOf(minOf(capacity, maxBytes.toLong()).toInt())
    }

    companion object {
        const val TS_BYTES: Int = 188
        /** Two-byte RFC 4571 length plus the 12-byte RTP header. */
        const val HEADER_BYTES: Int = 14
        const val FRAMED_PACKET_BYTES: Int = HEADER_BYTES + 7 * TS_BYTES
        private const val INITIAL_BYTES = 64 * 1024
        private const val RETAINED_BYTES = 1024 * 1024
        /** Twice the largest legitimate unit: a 4 MiB video frame (VideoQueue's limit) is about 4.4 MB framed. */
        const val MAX_BYTES: Int = 8 * 1024 * 1024
        fun framedLength(bytes: ByteArray, offset: Int): Int =
            (((bytes[offset].toInt() and 255) shl 8) or (bytes[offset + 1].toInt() and 255)) + 2
        private fun putInt(b: ByteArray, offset: Int, value: Int) {
            b[offset] = (value ushr 24).toByte(); b[offset + 1] = (value ushr 16).toByte()
            b[offset + 2] = (value ushr 8).toByte(); b[offset + 3] = value.toByte()
        }
    }
}
