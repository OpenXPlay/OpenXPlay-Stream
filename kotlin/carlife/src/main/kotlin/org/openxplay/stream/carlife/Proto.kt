// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-11-02

package org.openxplay.stream.carlife

import java.io.ByteArrayOutputStream

/** Presence-aware subset of proto2; unknown fields are skipped, never interpreted. */
internal class Proto private constructor(
    private val integers: Map<Int, Long>,
    private val strings: Map<Int, List<ByteArray>>,
) {
    fun number(field: Int): Long? = integers[field]
    fun text(field: Int): String? = strings[field]?.lastOrNull()?.toString(Charsets.UTF_8)
    fun messages(field: Int): List<Proto> = strings[field].orEmpty().map { parse(it) }

    companion object {
        fun parse(bytes: ByteArray): Proto {
            var offset = 0
            val integers = mutableMapOf<Int, Long>()
            val strings = mutableMapOf<Int, MutableList<ByteArray>>()
            fun varint(): Long {
                var value = 0L
                for (index in 0..9) {
                    protocolRequire(offset < bytes.size, "malformed_protobuf")
                    val next = bytes[offset++].toInt() and 255
                    if (index == 9) protocolRequire(next <= 1, "malformed_protobuf")
                    value = value or ((next and 127).toLong() shl (index * 7))
                    if (next < 128) return value
                }
                throw CarlifeProtocolException("malformed_protobuf")
            }
            fun skip(count: Int) {
                protocolRequire(count >= 0 && count <= bytes.size - offset, "malformed_protobuf")
                offset += count
            }
            fun field(tag: Long, depth: Int, collect: Boolean): Unit {
                protocolRequire(depth < 32 && tag ushr 3 in 1..536_870_911, "malformed_protobuf")
                val number = (tag ushr 3).toInt()
                when ((tag and 7).toInt()) {
                    0 -> { val value = varint(); if (collect) integers[number] = value }
                    1 -> skip(8)
                    2 -> {
                        val length = varint()
                        protocolRequire(length in 0..(bytes.size - offset).toLong(), "malformed_protobuf")
                        val end = offset + length.toInt()
                        if (collect) strings.getOrPut(number) { mutableListOf() }.add(bytes.copyOfRange(offset, end))
                        offset = end
                    }
                    3 -> {
                        while (true) {
                            val nested = varint()
                            if ((nested and 7) == 4L) {
                                protocolRequire(nested ushr 3 == number.toLong(), "malformed_protobuf")
                                break
                            }
                            field(nested, depth + 1, false)
                        }
                    }
                    5 -> skip(4)
                    else -> throw CarlifeProtocolException("malformed_protobuf")
                }
            }
            while (offset < bytes.size) field(varint(), 0, true)
            return Proto(integers, strings)
        }
    }
}

internal class ProtoWriter {
    private val bytes = ByteArrayOutputStream()
    private fun varint(number: Long) {
        var value = number
        while (value and -128L != 0L) {
            bytes.write((value.toInt() and 127) or 128)
            value = value ushr 7
        }
        bytes.write(value.toInt())
    }
    fun number(field: Int, number: Long): ProtoWriter = apply { varint(field.toLong() shl 3); varint(number) }
    fun text(field: Int, text: String): ProtoWriter = data(field, text.toByteArray(Charsets.UTF_8))
    fun data(field: Int, data: ByteArray): ProtoWriter = apply {
        varint((field.toLong() shl 3) or 2)
        varint(data.size.toLong())
        bytes.write(data)
    }
    fun build(): ByteArray = bytes.toByteArray()
}
