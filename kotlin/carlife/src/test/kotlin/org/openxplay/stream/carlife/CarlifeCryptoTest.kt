// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-11-02

package org.openxplay.stream.carlife

import java.security.KeyPairGenerator
import java.util.Base64
import javax.crypto.Cipher
import javax.crypto.spec.SecretKeySpec
import kotlin.test.*
import org.junit.jupiter.api.Test

class CarlifeCryptoTest {
    @Test fun digestMatchesIndependentMd5VectorAndExactDeviceFields() {
        assertEquals("abbc46749f4bce7b06c1b88be4f613ac", CarlifeCrypto.authenticate(
            CarlifeDeviceInfo(), "2.0;2026-11-02-10-20-30"))
        assertNotEquals(CarlifeCrypto.authenticate(CarlifeDeviceInfo(), "2.0;2026-11-02-10-20-30"),
            CarlifeCrypto.authenticate(CarlifeDeviceInfo(sdk = "2"), "2.0;2026-11-02-10-20-30"))
        assertFailsWith<IllegalArgumentException> { CarlifeCrypto.authenticate(CarlifeDeviceInfo(), "unsupported") }
    }

    @Test fun rsaExchangeAndAesPayloadRoundTripLeaveHeaderAndEmptyFramesClear() {
        val keys = KeyPairGenerator.getInstance("RSA").apply { initialize(2048) }.generateKeyPair()
        val crypto = CarlifeCrypto()
        val encryptedKey = crypto.negotiate(Base64.getEncoder().encodeToString(keys.public.encoded))
        val rsa = Cipher.getInstance("RSA/ECB/PKCS1Padding")
        rsa.init(Cipher.DECRYPT_MODE, keys.private)
        val key = rsa.doFinal(Base64.getDecoder().decode(encryptedKey))
        assertEquals(16, key.size)
        val message = CarlifeMessage(1, 65544, byteArrayOf(8, 1, 16, 2))
        assertSame(message, crypto.transform(message, true))
        crypto.activate()
        val encrypted = crypto.transform(message, true)
        assertEquals(1, encrypted.channel)
        assertEquals(message.id, encrypted.id)
        assertEquals(16, encrypted.payload.size)
        val aes = Cipher.getInstance("AES/ECB/PKCS5Padding")
        aes.init(Cipher.DECRYPT_MODE, SecretKeySpec(key, "AES"))
        assertContentEquals(message.payload, aes.doFinal(encrypted.payload))
        assertContentEquals(message.payload, crypto.transform(encrypted, false).payload)
        val version = CarlifeMessage(1, 98305, byteArrayOf(1))
        assertSame(version, crypto.transform(version, true))
        val empty = CarlifeMessage(2, 131074)
        assertSame(empty, crypto.transform(empty, true))
        val encoded = CarlifeFraming.encode(encrypted)
        assertEquals(16, encoded[1].toInt())
    }

    @Test fun encryptionCannotActivateWithoutNegotiation() {
        assertEquals("unexpected_phase", assertFailsWith<CarlifeProtocolException> { CarlifeCrypto().activate() }.code)
    }

    @Test fun protoPresenceZeroRepeatedMessagesAndUnknownFields() {
        val encoded = ProtoWriter().number(1, 0).number(9, -1).text(2, "hello")
            .data(3, ProtoWriter().text(1, "a").build()).data(3, ProtoWriter().text(1, "b").build())
            .build() + byteArrayOf(0x25, 1, 2, 3, 4, 0x29, 1, 2, 3, 4, 5, 6, 7, 8)
        val parsed = Proto.parse(encoded)
        assertEquals(0L, parsed.number(1))
        assertNull(parsed.number(8))
        assertEquals(-1L, parsed.number(9))
        assertEquals("hello", parsed.text(2))
        assertEquals(listOf("a", "b"), parsed.messages(3).map { it.text(1) })
    }

    @Test fun malformedProtoFailsWithoutReadingPastBounds() {
        for (bytes in listOf(byteArrayOf(0), byteArrayOf(8, -128), byteArrayOf(10, 127), byteArrayOf(13, 1),
            ByteArray(12) { -128 })) assertFails { Proto.parse(bytes) }
    }

    @Test fun unknownGroupsNeverOverwriteRootFields() {
        val root = ProtoWriter().number(1, 4).text(2, "root").build()
        // Unknown group field 3 contains fields 1 and 2 with conflicting values.
        val nested = byteArrayOf(27) + ProtoWriter().number(1, 999).text(2, "nested").build() + byteArrayOf(28)
        val decoded = Proto.parse(root + nested)
        assertEquals(4L, decoded.number(1))
        assertEquals("root", decoded.text(2))
    }
}
