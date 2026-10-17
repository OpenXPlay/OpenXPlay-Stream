// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core.codec

internal class ByteCursor(private val data: ByteArray) {
    private var position: Int = 0
    val remaining: Int get() = data.size - position

    fun u8(): Int {
        require(remaining >= 1) { "Truncated configuration record" }
        return data[position++].toInt() and 0xff
    }

    fun u16(): Int = (u8() shl 8) or u8()
    fun u32(): Long = (u16().toLong() shl 16) or u16().toLong()

    fun bytes(size: Int): ByteArray {
        require(size >= 0 && size <= remaining) { "Truncated configuration record" }
        return data.copyOfRange(position, position + size).also { position += size }
    }

    fun end() {
        require(remaining == 0) { "Unexpected configuration bytes" }
    }
}

internal class BitReader(data: ByteArray, headerSize: Int) {
    private val rbsp: ByteArray
    private var position: Long = 0

    init {
        require(data.size > headerSize) { "Truncated parameter set" }
        val output = ByteArray(data.size - headerSize)
        var size = 0
        var zeros = 0
        var index = headerSize
        while (index < data.size) {
            val byte = data[index].toInt() and 0xff
            if (zeros >= 2 && byte == 3) {
                require(index + 1 < data.size && (data[index + 1].toInt() and 0xff) <= 3) {
                    "Invalid emulation-prevention byte"
                }
                zeros = 0
            } else {
                output[size++] = data[index]
                zeros = if (byte == 0) zeros + 1 else 0
            }
            index++
        }
        rbsp = output.copyOf(size)
    }

    fun bits(count: Int): Long {
        require(count in 0..32 && position + count <= rbsp.size.toLong() * 8) { "Truncated bitstream" }
        var value = 0L
        repeat(count) {
            val bit = (rbsp[(position / 8).toInt()].toInt() ushr (7 - (position % 8).toInt())) and 1
            value = (value shl 1) or bit.toLong()
            position++
        }
        return value
    }

    fun flag(): Boolean = bits(1) != 0L

    fun skip(count: Int) {
        require(count >= 0 && position + count <= rbsp.size.toLong() * 8) { "Truncated bitstream" }
        position += count
    }

    fun ue(): Int {
        var zeros = 0
        while (!flag()) {
            zeros++
            require(zeros <= 30) { "Exp-Golomb value exceeds supported range" }
        }
        return ((1L shl zeros) - 1 + bits(zeros)).toInt()
    }

    fun se(): Int {
        val value = ue()
        return if (value and 1 == 0) -(value / 2) else value / 2 + 1
    }
}

internal fun dimensions(width: Long, height: Long): Pair<Int, Int> {
    require(width in 1..Int.MAX_VALUE.toLong() && height in 1..Int.MAX_VALUE.toLong()) {
        "Invalid picture dimensions or cropping"
    }
    return width.toInt() to height.toInt()
}

internal fun hexByte(value: Int): String = value.toString(16).uppercase().padStart(2, '0')
