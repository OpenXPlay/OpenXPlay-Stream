// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core.audio

/** ADTS framing for one raw AAC access unit per frame, without CRC or program configuration elements. */
public object Adts {
    /** Payload bytes are copied by the parser and belong to the caller. */
    public class AccessUnit(public val config: AudioSpecificConfig, public val data: ByteArray)

    public data class Header(public val config: AudioSpecificConfig, public val frameLength: Int)

    /** Builds a seven-byte MPEG-4 ADTS header; explicit ASC rates must have a standard ADTS index. */
    public fun buildHeader(config: AudioSpecificConfig, accessUnitSize: Int): ByteArray {
        require(accessUnitSize in 1..8184) { "Invalid ADTS access-unit size" }
        val index = AudioSpecificConfig.frequencyIndex(config.sampleRate)
        require(index != 15) { "ADTS cannot encode an explicit sample rate" }
        val channels = config.channelConfiguration
        val length = accessUnitSize + 7
        return byteArrayOf(
            0xff.toByte(), 0xf1.toByte(),
            (((config.objectType - 1) shl 6) or (index shl 2) or (channels ushr 2)).toByte(),
            (((channels and 3) shl 6) or (length ushr 11)).toByte(),
            (length ushr 3).toByte(), (((length and 7) shl 5) or 0x1f).toByte(), 0xfc.toByte(),
        )
    }

    /** Validates a complete unprotected header and declared frame length at the given offset. */
    public fun parseHeader(bytes: ByteArray, offset: Int = 0): Header {
        require(offset >= 0 && offset <= bytes.size && bytes.size - offset >= 7) { "Truncated ADTS header" }
        fun byte(index: Int): Int = bytes[offset + index].toInt() and 0xff
        require(byte(0) == 0xff && byte(1) and 0xf6 == 0xf0) { "Invalid ADTS sync or layer" }
        require(byte(1) and 1 == 1) { "CRC-protected ADTS is unsupported" }
        require(byte(6) and 3 == 0) { "Multiple ADTS raw data blocks are unsupported" }
        val index = (byte(2) ushr 2) and 15
        val type = (byte(2) ushr 6) + 1
        val channels = ((byte(2) and 1) shl 2) or (byte(3) ushr 6)
        val config = AudioSpecificConfig(type, AudioSpecificConfig.rateForIndex(index), channels, index)
        require(byte(1) and 8 == 0 || type != 4) { "AAC LTP requires MPEG-4 ADTS" }
        val length = ((byte(3) and 3) shl 11) or (byte(4) shl 3) or (byte(5) ushr 5)
        require(length > 7 && length <= bytes.size - offset) { "Invalid or truncated ADTS frame length" }
        return Header(config, length)
    }

    /** Strictly splits a complete .aac byte stream; config is attached to each AU, including changes. */
    public fun parse(bytes: ByteArray): List<AccessUnit> {
        val result = mutableListOf<AccessUnit>()
        var offset = 0
        while (offset < bytes.size) {
            val header = parseHeader(bytes, offset)
            result.add(AccessUnit(header.config, bytes.copyOfRange(offset + 7, offset + header.frameLength)))
            offset += header.frameLength
        }
        return result
    }

    /** Used by muxers to reject concatenated frames before emitting output. */
    public fun validateFrame(bytes: ByteArray): Header = parseHeader(bytes).also {
        require(it.frameLength == bytes.size) { "Expected exactly one ADTS frame" }
    }
}
