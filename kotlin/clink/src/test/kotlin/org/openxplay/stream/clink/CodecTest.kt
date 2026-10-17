// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.clink

import java.security.SecureRandom
import kotlin.test.*
import org.openxplay.stream.clink.protocol.*
import org.openxplay.stream.clink.ble.*

internal fun hex(value: String): ByteArray = java.util.HexFormat.of().parseHex(value)
internal fun ByteArray.hex(): String = java.util.HexFormat.of().formatHex(this)
internal class FixedRandom(private val bytes: ByteArray) : SecureRandom() {
    override fun nextBytes(target: ByteArray) { require(target.size == bytes.size); bytes.copyInto(target) }
}

class CodecTest {
    @Test fun frameReferenceVectorsAndFragmentation() {
        // Vectors computed with an independent implementation; frozen timestamp and sequence.
        // Initial CRC is zero; the header stores it big-endian.
        assertEquals(0, Frame.crc16(byteArrayOf()))
        assertEquals(0xbb3d, Frame.crc16("123456789".toByteArray()))
        val bytes = hex("0000001701020304650000010b03000100006c56080138")
        val frame = Frame(0x01020304, 0x65000001, 1, 1, 1, 3, 1, hex("080138"))
        assertContentEquals(bytes, frame.encode())
        val decoded = Frame.decode(bytes)
        assertEquals(frame.sequence, decoded.sequence)
        assertEquals(frame.unixSeconds, decoded.unixSeconds)
        assertEquals(1, decoded.source); assertEquals(1, decoded.format); assertEquals(1, decoded.kind)
        assertEquals(3, decoded.category); assertEquals(1, decoded.method)
        assertContentEquals(frame.body, decoded.body)
        for (split in 0..bytes.size) {
            val reader = FrameReader()
            val frames = reader.feed(bytes.copyOfRange(0, split)) + reader.feed(bytes.copyOfRange(split, bytes.size) + bytes)
            assertEquals(2, frames.size); assertEquals(0, reader.pendingBytes)
        }
        val corrupt = bytes.copyOf().also { it[4] = 0 }
        assertFailsWith<IllegalArgumentException> { Frame.decode(corrupt) }
        assertFailsWith<IllegalArgumentException> { FrameReader().feed(hex("ffffffff")) }
        for (i in listOf(12, 16)) {
            val bad = bytes.copyOf().also { it[i] = (it[i].toInt() or 0x80).toByte() }
            val crc = Frame.crc16(bad.copyOfRange(0, 18))
            bad[18] = (crc shr 8).toByte(); bad[19] = crc.toByte()
            assertFailsWith<IllegalArgumentException> { Frame.decode(bad) }
        }
        val source = byteArrayOf(1)
        val immutable = Frame(0, 0, 0, 0, 0, 0, 0, source)
        source[0] = 2; immutable.body[0] = 3
        assertEquals(1, immutable.body[0].toInt())
    }
    @Test fun fieldsReferenceVectors() {
        // Unsigned LEB128 varints, fields encoded in ascending ID order.
        val body = Fields(mapOf(3 to FieldValue.Bytes("abc".toByteArray()),
            1 to FieldValue.Varint(150uL), 2 to FieldValue.Varint(0uL)))
        assertEquals("08960110001a03616263", body.encode().hex())
        assertEquals(150uL, Fields.decode(body.encode()).number(1))
        assertContentEquals("abc".toByteArray(), Fields.decode(body.encode()).bytes(3))
        val max = Fields(mapOf(1 to FieldValue.Varint(ULong.MAX_VALUE)))
        assertEquals("08ffffffffffffffffff01", max.encode().hex())
        assertEquals(ULong.MAX_VALUE, Fields.decode(max.encode()).number(1))
        val fixed = hex("0900010203040506071500010203")
        assertContentEquals(fixed, Fields.decode(fixed).encode())
        for (invalid in listOf("00", "0880", "08010802", "1203ff", "08ffffffffffffffffff02")) {
            assertFailsWith<IllegalArgumentException> { Fields.decode(hex(invalid)) }
        }
        val raw = byteArrayOf(1)
        val value = FieldValue.Bytes(raw)
        raw[0] = 9; value.value[0] = 8
        assertEquals(1, value.value[0].toInt())
    }
    @Test fun gcmKnownAnswerReplayTamperingAndFragments() {
        // NIST AES-128-GCM: zero key, 96-bit zero IV, 16-byte zero plaintext, no AAD.
        val record = hex("0000000c00000000000000000000000000000020" +
            "0388dace60b6a392f328c2b971b2fe78ab6e47d42cec13bdf53a67b21257bddf")
        assertContentEquals(record, SecureRecord(ByteArray(16), FixedRandom(ByteArray(12))).seal(ByteArray(16)))
        val inbound = SecureRecord(ByteArray(16))
        assertContentEquals(ByteArray(16), inbound.open(record))
        assertFailsWith<IllegalArgumentException> { inbound.open(record) }
        val corrupted = record.copyOf().also { it[it.lastIndex] = (it.last().toInt() xor 1).toByte() }
        assertFails { SecureRecord(ByteArray(16)).open(corrupted) }
        for (split in 0..record.size) {
            val reader = SecureRecord(ByteArray(16))
            val decoded = reader.feed(record.copyOfRange(0, split)) + reader.feed(record.copyOfRange(split, record.size))
            assertEquals(1, decoded.size); assertContentEquals(ByteArray(16), decoded.single())
            assertEquals(0, reader.pendingBytes)
        }
        val key = ByteArray(16) { it.toByte() }
        val plain = "projection control".toByteArray()
        assertContentEquals(plain, SecureRecord(key).open(SecureRecord(key).seal(plain)))
        assertFails { SecureRecord(key).open(record + record) }
        assertFails { SecureRecord(key).feed(hex("0000000b")) }
    }
    @Test fun bleReferenceVectors() {
        // 15-byte identity, plus the 1.9 extension with a vendor field.
        val ad = BleCodec.advertisement(hex("010200020000000000000100010000"), "示例车机".toByteArray())
        assertEquals("020000000000", ad.carId); assertEquals("00010001", ad.modelId)
        assertEquals("0000", ad.vendorData); assertEquals("示例车机", ad.name)
        assertEquals(1, ad.major); assertEquals(2, ad.minor); assertEquals(0, ad.serial)
        assertEquals("000102030405", BleCodec.advertisement(hex("010900000102030405060708091234ff"),
            byteArrayOf(67, 97, 114, 0, 0)).carId)
        val request = BleCodec.phoneRequest(PhoneRequest("client-id", "Pi source", "Linux", 1, "aa:bb:cc:dd:ee:ff"))
        assertEquals("""{"id":"client-id","name":"Pi source","model":"Linux","band":1,"mac":"aa:bb:cc:dd:ee:ff","type":1001}""", request.toString(Charsets.UTF_8))
        val json = """{"name":"car short name","ssid":"DIRECT-vs-XXXXXX","psk":"temporary-wifi-secret","mac":"02:00:00:00:00:00","freq":5200,"port":0,"type":1001}"""
        val reply = BleCodec.headUnitReply(json.toByteArray())
        assertEquals("DIRECT-vs-XXXXXX", reply.ssid); assertEquals(5200, reply.freq); assertEquals(0, reply.port)
        assertEquals("02:00:00:00:00:00", reply.mac)
        assertEquals(0, BleCodec.headUnitReply(json.replace("5200", "0").toByteArray()).freq)
        assertFails { BleCodec.headUnitReply(json.replace("5200", "\"5200\"").toByteArray()) }
        assertFails { BleCodec.headUnitReply(json.replace("{", "{\"ssid\":\"duplicate\",").toByteArray()) }
        assertFails { BleCodec.headUnitReply(byteArrayOf(0xc0.toByte(), 0x80.toByte())) }
        assertFails { BleCodec.advertisement(ByteArray(14)) }
    }
}
