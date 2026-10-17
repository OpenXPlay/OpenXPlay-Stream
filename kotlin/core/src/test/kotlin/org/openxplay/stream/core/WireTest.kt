// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core

import org.openxplay.stream.core.wire.BinaryHeader
import org.openxplay.stream.core.wire.BinaryKind
import org.openxplay.stream.core.wire.BinaryMessage
import org.openxplay.stream.core.wire.KeyPayload
import org.openxplay.stream.core.wire.OxpWebCodec
import org.openxplay.stream.core.wire.WireFormatException
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

class WireTest {
    @Test
    fun videoHeaderGoldenVector() {
        // 01 video, 01 cluster, 03 keyframe+gap, 00 reserved. All multi-byte fields are little-endian.
        val header = BinaryHeader(
            BinaryKind.VIDEO, 1, OxpWebCodec.KEYFRAME or OxpWebCodec.GAP, 0x12345678u,
            0x0102030405060708L, 0x1112131415161718L,
        )
        assertContentEquals(vector("video-header"), OxpWebCodec.encodeHeader(header))
        assertEquals(header, OxpWebCodec.decodeHeader(vector("video-header")))
    }

    @Test
    fun headerAndOpaqueMediaRoundTrips() {
        for (kind in BinaryKind.entries) {
            for (sequence in listOf(0u, 1u, UInt.MAX_VALUE)) {
                val header = BinaryHeader(kind, 255, 0, sequence, Long.MIN_VALUE, Long.MAX_VALUE)
                assertEquals(header, OxpWebCodec.decodeHeader(OxpWebCodec.encodeHeader(header)))
            }
        }
        for (kind in listOf(BinaryKind.VIDEO, BinaryKind.AUDIO, BinaryKind.MICROPHONE)) {
            val header = BinaryHeader(kind, 0, 0, UInt.MAX_VALUE, -1, 1234)
            val payload = hex("00 00 00 02 65 88")
            val decoded = OxpWebCodec.decode(OxpWebCodec.encode(BinaryMessage(header, payload)))
            assertEquals(header, decoded.header)
            assertContentEquals(payload, decoded.payload)
        }
    }

    @Test
    fun touchMessageGoldenVector() {
        val contacts = listOf(TouchContact(2, 0.5, 1.0, true), TouchContact(7, 0.0, 0.25, false))
        val header = BinaryHeader(BinaryKind.TOUCH, 0, 0, 42u, 0, 1_000_000)
        // Count 02, contact 2 pressed: x 00 80, y ff ff; contact 7 released: x 00 00, y 00 40.
        val bytes = OxpWebCodec.encode(BinaryMessage(header, OxpWebCodec.encodeTouch(contacts)))
        assertContentEquals(vector("touch-message"), bytes)
        val decoded = OxpWebCodec.decode(vector("touch-message"))
        assertEquals(header, decoded.header)
        val roundTrip = OxpWebCodec.decodeTouch(decoded.payload)
        for ((expected, actual) in contacts.zip(roundTrip)) {
            assertEquals(expected.id, actual.id)
            assertEquals(expected.pressed, actual.pressed)
            assertEquals(expected.x, actual.x, 0.5 / 65535)
            assertEquals(expected.y, actual.y, 0.5 / 65535)
        }
        assertContentEquals(bytes, OxpWebCodec.encode(BinaryMessage(header, OxpWebCodec.encodeTouch(roundTrip))))
        assertContentEquals(hex("00"), OxpWebCodec.encodeTouch(emptyList()))
        assertTrue(OxpWebCodec.decodeTouch(hex("00")).isEmpty())
    }

    @Test
    fun everyKeyAndBooleanRoundTrips() {
        for (code in KeyCode.entries) {
            for (down in listOf(false, true)) {
                val key = KeyPayload(code, down)
                val payload = OxpWebCodec.encodeKey(key)
                assertEquals(key, OxpWebCodec.decodeKey(payload))
                val header = BinaryHeader(BinaryKind.KEY, 0, 0, 1u, 0, 0)
                val decoded = OxpWebCodec.decode(OxpWebCodec.encode(BinaryMessage(header, payload)))
                assertEquals(key, OxpWebCodec.decodeKey(decoded.payload))
            }
        }
        assertContentEquals(hex("0a 00 01"), OxpWebCodec.encodeKey(KeyPayload(KeyCode.VOICE, true)))
    }

    @Test
    fun malformedHeadersUseOneExceptionType() {
        val valid = vector("video-header")
        for (size in 0 until OxpWebCodec.HEADER_SIZE) {
            assertFailsWith<WireFormatException> { OxpWebCodec.decode(valid.copyOf(size)) }
        }
        assertFailsWith<WireFormatException> { OxpWebCodec.decodeHeader(valid + 0) }
        for ((offset, value) in listOf(0 to 0xff, 2 to 4, 3 to 1)) {
            assertFailsWith<WireFormatException> {
                OxpWebCodec.decode(valid.copyOf().also { it[offset] = value.toByte() })
            }
        }
        val header = OxpWebCodec.decodeHeader(valid)
        assertFailsWith<WireFormatException> { OxpWebCodec.encodeHeader(header.copy(stream = -1)) }
        assertFailsWith<WireFormatException> { OxpWebCodec.encodeHeader(header.copy(stream = 256)) }
        assertFailsWith<WireFormatException> { OxpWebCodec.encodeHeader(header.copy(flags = 0x80)) }
    }

    @Test
    fun malformedPayloadsUseOneExceptionType() {
        val touchHeader = BinaryHeader(BinaryKind.TOUCH, 0, 0, 0u, 0, 0)
        for (bytes in listOf(
            byteArrayOf(), hex("00 00"), hex("01"), hex("0b"), hex("01 0a 01 00 00 00 00"),
            hex("01 00 02 00 00 00 00"), hex("02 00 01 00 00 00 00 00 00 00 00 00 00"),
        )) {
            assertFailsWith<WireFormatException> { OxpWebCodec.decodeTouch(bytes) }
            assertFailsWith<WireFormatException> { OxpWebCodec.encode(BinaryMessage(touchHeader, bytes)) }
            assertFailsWith<WireFormatException> { OxpWebCodec.decode(OxpWebCodec.encodeHeader(touchHeader) + bytes) }
        }
        val duplicate = TouchContact(0, 0.0, 0.0, true)
        assertFailsWith<WireFormatException> { OxpWebCodec.encodeTouch(listOf(duplicate, duplicate)) }
        assertFailsWith<WireFormatException> { OxpWebCodec.encodeTouch(List(11) { duplicate }) }
        val keyHeader = touchHeader.copy(kind = BinaryKind.KEY)
        for (bytes in listOf(byteArrayOf(), hex("00 00"), hex("00 00 01 00"), hex("ff ff 01"), hex("00 00 02"))) {
            assertFailsWith<WireFormatException> { OxpWebCodec.decodeKey(bytes) }
            assertFailsWith<WireFormatException> { OxpWebCodec.encode(BinaryMessage(keyHeader, bytes)) }
            assertFailsWith<WireFormatException> { OxpWebCodec.decode(OxpWebCodec.encodeHeader(keyHeader) + bytes) }
        }
    }
}
