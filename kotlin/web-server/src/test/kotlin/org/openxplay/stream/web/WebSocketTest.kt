// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.web

import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import org.junit.jupiter.api.Test
import kotlin.test.*

internal fun masked(opcode: Int, bytes: ByteArray, fin: Boolean = true): ByteArray {
    val out = ByteArrayOutputStream()
    out.write((if (fin) 128 else 0) or opcode)
    when {
        bytes.size < 126 -> out.write(128 or bytes.size)
        bytes.size <= 65535 -> { out.write(128 or 126); out.write(bytes.size ushr 8); out.write(bytes.size) }
        else -> { out.write(128 or 127); for (shift in 56 downTo 0 step 8) out.write((bytes.size.toLong() ushr shift).toInt()) }
    }
    val mask = byteArrayOf(7, 19, 31, 43)
    out.write(mask)
    bytes.forEachIndexed { i, b -> out.write(b.toInt() xor mask[i % 4].toInt()) }
    return out.toByteArray()
}

class WebSocketTest {
    @Test fun maskingAndFragmentationWithInterleavedPing() {
        val bytes = masked(1, "hel".toByteArray(), false) + masked(9, byteArrayOf(42)) + masked(0, "lo".toByteArray())
        val reader = WebSocketReader(ByteArrayInputStream(bytes), 64)
        assertEquals(9, reader.read().opcode)
        assertEquals("hello", utf8(reader.read().payload))
        val unmasked = byteArrayOf(0x81.toByte(), 0)
        assertEquals(1002, assertFailsWith<WebSocketFailure> { WebSocketReader(ByteArrayInputStream(unmasked), 64).read() }.code)
    }

    @Test fun oversizeIncludesReassembledLength() {
        val bytes = masked(2, ByteArray(10), false) + masked(0, ByteArray(10))
        assertEquals(1009, assertFailsWith<WebSocketFailure> { WebSocketReader(ByteArrayInputStream(bytes), 16).read() }.code)
        assertEquals(1009, assertFailsWith<WebSocketFailure> { WebSocketReader(ByteArrayInputStream(masked(2, ByteArray(65536))), 16).read() }.code)
    }

    @Test fun closeAndMalformedFrames() {
        assertContentEquals(closePayload(1000), WebSocketReader(ByteArrayInputStream(masked(8, closePayload(1000))), 64).read().payload)
        for (bytes in listOf(masked(8, byteArrayOf(1)), masked(8, closePayload(1006)), masked(9, byteArrayOf(1), false), masked(0, byteArrayOf(1)))) {
            assertEquals(1002, assertFailsWith<WebSocketFailure> { WebSocketReader(ByteArrayInputStream(bytes), 64).read() }.code)
        }
        assertEquals(1007, assertFailsWith<WebSocketFailure> { WebSocketReader(ByteArrayInputStream(masked(1, byteArrayOf(0xff.toByte()))), 64).read() }.code)
    }

    @Test fun outgoingHeaderAndSeparatePayloads() {
        for (size in listOf(0, 125, 126, 65535, 65536)) {
            val out = ByteArrayOutputStream()
            writeWebSocket(out, 2, ByteArray(24), ByteArray(size))
            val wire = out.toByteArray()
            assertEquals(0x82, wire[0].toInt() and 255)
            assertEquals(0, wire[1].toInt() and 128)
            val headerSize = when (wire[1].toInt() and 127) { 126 -> 4; 127 -> 10; else -> 2 }
            assertEquals(headerSize + 24 + size, wire.size)
        }
    }
}
