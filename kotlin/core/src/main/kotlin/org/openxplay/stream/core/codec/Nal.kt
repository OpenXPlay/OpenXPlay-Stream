// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core.codec

import org.openxplay.stream.core.VideoCodec

/** NAL framing utilities. Conversion preserves emulation-prevention bytes. */
public object Nal {
    public enum class Format { ANNEX_B, AVCC }

    /** Iterates copied NAL payloads without framing; malformed framing throws IllegalArgumentException on iteration. */
    public fun units(data: ByteArray, format: Format = Format.AVCC): Sequence<ByteArray> =
        ranges(data, format).map { data.copyOfRange(it.first, it.last + 1) }

    /** Accepts three/four-byte start codes and Annex B leading/trailing zero bytes. */
    public fun annexBToAvcc(data: ByteArray): ByteArray = convert(data, Format.ANNEX_B)

    /** Accepts four-byte unsigned big-endian lengths and emits four-byte start codes. */
    public fun avccToAnnexB(data: ByteArray): ByteArray = convert(data, Format.AVCC)

    public fun isKeyFrame(data: ByteArray, codec: VideoCodec, format: Format = Format.AVCC): Boolean {
        var keyFrame = false
        // Consume every NAL so a keyframe cannot conceal malformed trailing framing.
        for (range in ranges(data, format)) {
            val header = data[range.first].toInt() and 0xff
            when (codec) {
                VideoCodec.H264 -> if (header and 0x1f == 5) keyFrame = true
                VideoCodec.H265 -> {
                    require(range.last - range.first + 1 >= 2) { "Truncated HEVC NAL header" }
                    if ((header ushr 1) and 0x3f in 16..23) keyFrame = true
                }
            }
        }
        return keyFrame
    }

    private fun convert(data: ByteArray, source: Format): ByteArray {
        val ranges = ranges(data, source).toList()
        val size = ranges.sumOf { it.last.toLong() - it.first + 5 }
        require(size <= Int.MAX_VALUE) { "NAL output is too large" }
        val output = ByteArray(size.toInt())
        var offset = 0
        for (range in ranges) {
            val length = range.last - range.first + 1
            val prefix = if (source == Format.ANNEX_B) length else 1
            for (shift in 24 downTo 0 step 8) output[offset++] = (prefix ushr shift).toByte()
            data.copyInto(output, offset, range.first, range.last + 1)
            offset += length
        }
        return output
    }

    private fun ranges(data: ByteArray, format: Format): Sequence<IntRange> = when (format) {
        Format.AVCC -> avccRanges(data)
        Format.ANNEX_B -> annexBRanges(data)
    }

    private fun avccRanges(data: ByteArray): Sequence<IntRange> = sequence {
        var offset = 0
        while (offset < data.size) {
            require(data.size - offset >= 4) { "Truncated NAL length" }
            var length = 0L
            repeat(4) { length = (length shl 8) or (data[offset++].toLong() and 0xff) }
            require(length > 0 && length <= data.size.toLong() - offset) { "Invalid NAL length" }
            val end = offset + length.toInt()
            yield(offset until end)
            offset = end
        }
    }

    private fun annexBRanges(data: ByteArray): Sequence<IntRange> = sequence {
        if (data.isEmpty()) return@sequence
        var payloadStart = -1
        var zeros = 0
        for (index in data.indices) {
            val byte = data[index].toInt() and 0xff
            if (byte == 1 && zeros >= 2) {
                val prefixStart = index - zeros
                if (payloadStart >= 0) {
                    require(prefixStart > payloadStart) { "Empty NAL unit" }
                    yield(payloadStart until prefixStart)
                } else {
                    require(prefixStart == 0) { "Unexpected bytes before start code" }
                }
                payloadStart = index + 1
            } else if (payloadStart < 0 && byte != 0) {
                throw IllegalArgumentException("Missing Annex B start code")
            }
            zeros = if (byte == 0) zeros + 1 else 0
        }
        require(payloadStart >= 0) { "Missing Annex B start code" }
        val end = data.size - zeros
        require(end > payloadStart) { "Empty NAL unit" }
        yield(payloadStart until end)
    }
}
