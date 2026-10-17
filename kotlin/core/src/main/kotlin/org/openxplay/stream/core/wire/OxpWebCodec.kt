// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core.wire

import org.openxplay.stream.core.KeyCode
import org.openxplay.stream.core.TouchContact
import kotlin.math.roundToInt

/** The single exception raised by OXP-Web encoders and decoders for malformed values or bytes. */
public class WireFormatException(message: String) : IllegalArgumentException(message)

public enum class BinaryKind(public val wireCode: Int) {
    VIDEO(0x01), AUDIO(0x02), MICROPHONE(0x03), TOUCH(0x10), KEY(0x11),
}

/** Sequence is an unsigned wrapping 32-bit counter. Both clocks are signed 64-bit microseconds. */
public data class BinaryHeader(
    public val kind: BinaryKind,
    public val stream: Int,
    public val flags: Int,
    public val sequence: UInt,
    public val ptsUs: Long,
    public val senderClockUs: Long,
)

/** [payload] must not be mutated after hand-off. Uses identity equality. */
public class BinaryMessage(public val header: BinaryHeader, public val payload: ByteArray)

public data class KeyPayload(public val code: KeyCode, public val down: Boolean)

/** OXP-Web v0 framing. Origins are supplied by the connection, not transmitted in these payloads. */
public object OxpWebCodec {
    public const val HEADER_SIZE: Int = 24
    public const val KEYFRAME: Int = 1
    public const val GAP: Int = 2

    public fun encodeHeader(header: BinaryHeader): ByteArray {
        validateHeader(header)
        return ByteArray(HEADER_SIZE).also { bytes ->
            bytes[0] = header.kind.wireCode.toByte()
            bytes[1] = header.stream.toByte()
            bytes[2] = header.flags.toByte()
            putLittleEndian(bytes, 4, header.sequence.toLong(), 4)
            putLittleEndian(bytes, 8, header.ptsUs, 8)
            putLittleEndian(bytes, 16, header.senderClockUs, 8)
        }
    }

    /** Requires exactly 24 bytes, including a zero reserved byte and only defined flag bits. */
    public fun decodeHeader(bytes: ByteArray): BinaryHeader {
        checkWire(bytes.size == HEADER_SIZE, "Header must be 24 bytes")
        val kind = BinaryKind.entries.firstOrNull { it.wireCode == u8(bytes, 0) }
            ?: throw WireFormatException("Unknown message kind")
        checkWire(u8(bytes, 3) == 0, "Nonzero reserved header byte")
        return BinaryHeader(
            kind, u8(bytes, 1), u8(bytes, 2), littleEndian(bytes, 4, 4).toUInt(),
            littleEndian(bytes, 8, 8), littleEndian(bytes, 16, 8),
        ).also { validateHeader(it) }
    }

    /** Materializes wire bytes. Outputs can send [encodeHeader] and the original media array separately to avoid copying. */
    public fun encode(message: BinaryMessage): ByteArray {
        validatePayload(message.header.kind, message.payload)
        checkWire(message.payload.size <= Int.MAX_VALUE - HEADER_SIZE, "Message is too large")
        val header = encodeHeader(message.header)
        return ByteArray(HEADER_SIZE + message.payload.size).also {
            header.copyInto(it)
            message.payload.copyInto(it, HEADER_SIZE)
        }
    }

    /** Media payloads are opaque; touch and key payloads are validated structurally and semantically. */
    public fun decode(bytes: ByteArray): BinaryMessage {
        checkWire(bytes.size >= HEADER_SIZE, "Truncated message header")
        val header = decodeHeader(bytes.copyOfRange(0, HEADER_SIZE))
        val payload = bytes.copyOfRange(HEADER_SIZE, bytes.size)
        validatePayload(header.kind, payload)
        return BinaryMessage(header, payload)
    }

    /** Coordinates are rounded to the closest unsigned 16-bit value. Empty lists release all contacts. */
    public fun encodeTouch(contacts: List<TouchContact>): ByteArray {
        validateContacts(contacts)
        return ByteArray(1 + contacts.size * 6).also { bytes ->
            bytes[0] = contacts.size.toByte()
            contacts.forEachIndexed { index, contact ->
                val offset = 1 + index * 6
                bytes[offset] = contact.id.toByte()
                bytes[offset + 1] = if (contact.pressed) 1 else 0
                putLittleEndian(bytes, offset + 2, (contact.x * 65535).roundToInt().toLong(), 2)
                putLittleEndian(bytes, offset + 4, (contact.y * 65535).roundToInt().toLong(), 2)
            }
        }
    }

    public fun decodeTouch(payload: ByteArray): List<TouchContact> {
        checkWire(payload.isNotEmpty(), "Missing touch count")
        val count = u8(payload, 0)
        checkWire(count <= 10 && payload.size == 1 + count * 6, "Invalid touch payload length or count")
        val seen = mutableSetOf<Int>()
        return List(count) { index ->
            val offset = 1 + index * 6
            val id = u8(payload, offset)
            checkWire(id in 0..9 && seen.add(id), "Invalid or duplicate contact id")
            val pressed = u8(payload, offset + 1)
            checkWire(pressed in 0..1, "Invalid pressed value")
            TouchContact(
                id, littleEndian(payload, offset + 2, 2) / 65535.0,
                littleEndian(payload, offset + 4, 2) / 65535.0, pressed == 1,
            )
        }
    }

    public fun encodeKey(key: KeyPayload): ByteArray = byteArrayOf(
        key.code.wireCode.toByte(), (key.code.wireCode ushr 8).toByte(), if (key.down) 1 else 0,
    )

    public fun decodeKey(payload: ByteArray): KeyPayload {
        checkWire(payload.size == 3, "Key payload must be three bytes")
        val code = littleEndian(payload, 0, 2).toInt()
        val key = KeyCode.entries.firstOrNull { it.wireCode == code }
            ?: throw WireFormatException("Unknown key code")
        val down = u8(payload, 2)
        checkWire(down in 0..1, "Invalid key down value")
        return KeyPayload(key, down == 1)
    }

    private fun validateHeader(header: BinaryHeader) {
        checkWire(header.stream in 0..255, "Stream must fit u8")
        checkWire(header.flags in 0..3, "Unknown header flags")
    }

    private fun validatePayload(kind: BinaryKind, payload: ByteArray) {
        when (kind) {
            BinaryKind.TOUCH -> decodeTouch(payload)
            BinaryKind.KEY -> decodeKey(payload)
            else -> Unit
        }
    }

    private fun validateContacts(contacts: List<TouchContact>) {
        checkWire(contacts.size <= 10, "Too many contacts")
        checkWire(contacts.map { it.id }.toSet().size == contacts.size, "Duplicate contact id")
    }

    private fun checkWire(condition: Boolean, message: String) {
        if (!condition) throw WireFormatException(message)
    }

    private fun u8(bytes: ByteArray, offset: Int): Int = bytes[offset].toInt() and 0xff

    private fun littleEndian(bytes: ByteArray, offset: Int, size: Int): Long {
        var value = 0L
        repeat(size) { index -> value = value or (u8(bytes, offset + index).toLong() shl (8 * index)) }
        return value
    }

    private fun putLittleEndian(bytes: ByteArray, offset: Int, value: Long, size: Int) {
        repeat(size) { index -> bytes[offset + index] = (value ushr (8 * index)).toByte() }
    }
}
