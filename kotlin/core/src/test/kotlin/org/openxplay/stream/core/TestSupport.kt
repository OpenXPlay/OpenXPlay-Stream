// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core

internal fun hex(value: String): ByteArray = value.trim().split(Regex("\\s+"))
    .filter { it.isNotEmpty() }.map { it.toInt(16).toByte() }.toByteArray()

internal fun vector(name: String): ByteArray {
    val resource = checkNotNull(object {}.javaClass.getResource("/vectors/$name.hex"))
    return hex(resource.readText())
}

/** Constructs synthetic parameter sets from explicitly chosen fields. */
internal class TestBits {
    private val bits = StringBuilder()

    fun bits(value: Long, count: Int): TestBits = apply {
        for (shift in count - 1 downTo 0) bits.append(if ((value ushr shift) and 1L == 0L) '0' else '1')
    }

    fun ue(value: Int): TestBits = apply {
        val code = (value.toLong() + 1).toString(2)
        bits.append("0".repeat(code.length - 1)).append(code)
    }

    fun se(value: Int): TestBits = ue(if (value > 0) 2 * value - 1 else -2 * value)

    fun nal(header: ByteArray): ByteArray {
        bits.append('1') // rbsp_stop_one_bit.
        while (bits.length % 8 != 0) bits.append('0')
        val bytes = mutableListOf<Byte>()
        var zeros = 0
        for (index in 0 until bits.length step 8) {
            val byte = bits.substring(index, index + 8).toInt(2)
            if (zeros >= 2 && byte <= 3) {
                bytes.add(3.toByte())
                zeros = 0
            }
            bytes.add(byte.toByte())
            zeros = if (byte == 0) zeros + 1 else 0
        }
        return header + bytes.toByteArray()
    }
}
