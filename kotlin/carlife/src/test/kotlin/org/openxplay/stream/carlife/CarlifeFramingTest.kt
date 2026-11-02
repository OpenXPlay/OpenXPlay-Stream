// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-11-02

package org.openxplay.stream.carlife

import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.InputStream
import java.nio.ByteBuffer
import kotlin.test.*
import org.junit.jupiter.api.Test

class CarlifeFramingTest {
    @Test fun controlUsesEightBigEndianBytes() {
        val encoded = CarlifeFraming.encode(CarlifeMessage(1, 65538, byteArrayOf(8, 1)))
        assertContentEquals(byteArrayOf(0, 2, 0, 0, 0, 1, 0, 2, 8, 1), encoded)
        val parsed = CarlifeFraming.read(ByteArrayInputStream(encoded), 1)!!
        assertEquals(65538, parsed.id)
        assertContentEquals(byteArrayOf(8, 1), parsed.payload)
    }

    @Test fun everyChannelUsesItsOwnHeader() {
        for (channel in 1..7) {
            val message = CarlifeMessage(channel, 131073, byteArrayOf(1, 2, 3))
            val bytes = CarlifeFraming.encode(message)
            assertEquals(if (channel in listOf(1, 6)) 11 else 15, bytes.size)
            val read = CarlifeFraming.read(ByteArrayInputStream(bytes), channel)!!
            assertEquals(channel, read.channel)
            assertContentEquals(message.payload, read.payload)
        }
    }

    @Test fun usbOuterHeaderIsASeparateWriteAndBodiesUse16KiBChunks() {
        val writes = mutableListOf<ByteArray>()
        val output = object : ByteArrayOutputStream() {
            override fun write(bytes: ByteArray, offset: Int, size: Int) {
                writes += bytes.copyOfRange(offset, offset + size)
                super.write(bytes, offset, size)
            }
        }
        val payload = ByteArray(40_000) { it.toByte() }
        CarlifeFraming.write(output, CarlifeMessage(2, 131073, payload), usb = true)
        assertEquals(listOf(8, 16384, 16384, 7244), writes.map { it.size })
        assertContentEquals(ByteBuffer.allocate(8).putInt(2).putInt(40012).array(), writes.first())
        val read = CarlifeFraming.readUsb(ByteArrayInputStream(output.toByteArray()))!!
        assertContentEquals(payload, read.payload)
    }

    @Test fun partialReadsAreCompleted() {
        val bytes = CarlifeFraming.encode(CarlifeMessage(6, 425985, byteArrayOf(1, 2)))
        val source = object : ByteArrayInputStream(bytes) {
            override fun read(bytes: ByteArray, offset: Int, length: Int): Int = super.read(bytes, offset, minOf(1, length))
        }
        assertContentEquals(byteArrayOf(1, 2), CarlifeFraming.read(source, 6)!!.payload)
    }

    @Test fun cleanEofAndTruncatedFramesAreDifferent() {
        assertNull(CarlifeFraming.read(ByteArrayInputStream(byteArrayOf()), 2))
        assertEquals("truncated_header", assertFailsWith<CarlifeProtocolException> {
            CarlifeFraming.read(ByteArrayInputStream(byteArrayOf(0)), 2)
        }.code)
        val frame = CarlifeFraming.encode(CarlifeMessage(1, 1, byteArrayOf(1)))
        val truncated = assertFailsWith<CarlifeProtocolException> {
            CarlifeFraming.read(ByteArrayInputStream(frame.copyOf(8)), 1)
        }
        assertEquals("truncated_payload", truncated.code)
        assertEquals(1, truncated.messageId)
        assertEquals(1, truncated.channel)
    }

    @Test fun invalidLengthsAndChannelsAreRejectedBeforePayloadRead() {
        for (length in listOf(-1, Int.MAX_VALUE, 67_108_864)) {
            val header = ByteBuffer.allocate(12).putInt(length).putInt(0).putInt(131073).array()
            val error = assertFailsWith<CarlifeProtocolException> { CarlifeFraming.read(ByteArrayInputStream(header), 2) }
            assertEquals("invalid_payload_length", error.code)
            assertEquals(2, error.channel)
            assertEquals(131073, error.messageId)
        }
        assertFailsWith<IllegalArgumentException> { CarlifeFraming.headerSize(0) }
        assertFailsWith<IllegalArgumentException> { CarlifeFraming.headerSize(8) }
        assertFailsWith<IllegalArgumentException> { CarlifeFraming.encode(CarlifeMessage(6, 1, ByteArray(65536))) }
        val bad = ByteBuffer.allocate(16).putInt(1).putInt(10).putShort(0).putShort(0).putInt(1).array()
        assertEquals("usb_length_mismatch", assertFailsWith<CarlifeProtocolException> {
            CarlifeFraming.readUsb(ByteArrayInputStream(bad))
        }.code)
    }

    @Test fun reservedBytesDoNotAffectDecoding() {
        val frame = CarlifeFraming.encode(CarlifeMessage(2, 131074))
        for (index in 4..7) frame[index] = 127
        assertEquals(131074, CarlifeFraming.read(ByteArrayInputStream(frame), 2)!!.id)
    }
}
