// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.clink.protocol

import java.io.ByteArrayOutputStream

/** Supported protobuf wire values; byte values are defensively copied. */
public sealed interface FieldValue {
    public data class Varint(public val value: ULong) : FieldValue
    public class Bytes(bytes: ByteArray, public val wireType: Int = 2) : FieldValue {
        private val data: ByteArray = bytes.copyOf()
        public val value: ByteArray get() = data.copyOf()
        init { require(wireType == 2 || wireType == 1 && data.size == 8 || wireType == 5 && data.size == 4) }
    }
}

/** Singular fields, sorted on encoding. Duplicate fields are rejected, as in the reference. */
public class Fields(values: Map<Int, FieldValue> = emptyMap()) {
    private val entries: Map<Int, FieldValue> = values.toSortedMap()
    init { require(entries.keys.all { it in 1 until (1 shl 29) }) }
    public val numbers: Set<Int> get() = entries.keys.toSet()
    public operator fun get(number: Int): FieldValue? = entries[number]
    public fun number(number: Int): ULong = when (val v = entries[number]) {
        null -> 0uL
        is FieldValue.Varint -> v.value
        else -> throw IllegalArgumentException("invalid_fields")
    }
    public fun bytes(number: Int, size: Int? = null): ByteArray {
        val v = (entries[number] as? FieldValue.Bytes)?.value
            ?: throw IllegalArgumentException("missing_byte_field")
        require(size == null || v.size == size) { "invalid_field_size" }
        return v
    }
    public fun encode(): ByteArray {
        val out = ByteArrayOutputStream()
        fun varint(value: ULong) {
            var n = value
            while (n >= 128uL) { out.write((n.toInt() and 127) or 128); n = n shr 7 }
            out.write(n.toInt())
        }
        for ((id, v) in entries) when (v) {
            is FieldValue.Varint -> { varint(id.toULong() shl 3); varint(v.value) }
            is FieldValue.Bytes -> {
                varint((id.toULong() shl 3) or v.wireType.toULong())
                val data = v.value
                if (v.wireType == 2) varint(data.size.toULong())
                out.write(data)
            }
        }
        return out.toByteArray()
    }
    public companion object {
        public fun decode(bytes: ByteArray): Fields {
            require(bytes.size <= Frame.MAX_BYTES)
            var pos = 0
            fun varint(): ULong {
                var value = 0uL
                for (i in 0..9) {
                    require(pos < bytes.size) { "truncated_varint" }
                    val b = bytes[pos++].toInt() and 255
                    require(i < 9 || b <= 1) { "varint_overflow" }
                    value = value or ((b and 127).toULong() shl (i * 7))
                    if (b and 128 == 0) return value
                }
                throw IllegalArgumentException("varint_overflow")
            }
            val result = mutableMapOf<Int, FieldValue>()
            while (pos < bytes.size) {
                val key = varint()
                val id = key shr 3
                require(id > 0uL && id < (1uL shl 29)) { "invalid_field_number" }
                require(id.toInt() !in result) { "duplicate_field" }
                val wire = (key and 7uL).toInt()
                val value = if (wire == 0) FieldValue.Varint(varint()) else {
                    val size = when (wire) { 1 -> 8uL; 5 -> 4uL; 2 -> varint(); else -> throw IllegalArgumentException("unsupported_wire_type") }
                    require(size <= (bytes.size - pos).toULong()) { "truncated_field" }
                    FieldValue.Bytes(bytes.copyOfRange(pos, pos + size.toInt()), wire).also { pos += size.toInt() }
                }
                result[id.toInt()] = value
            }
            return Fields(result)
        }
    }
}
