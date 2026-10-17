// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.clink

import java.security.KeyFactory
import java.security.KeyPair
import java.security.Signature
import java.security.spec.PKCS8EncodedKeySpec
import java.util.Properties
import kotlin.test.*
import org.openxplay.stream.clink.protocol.*

class AuthTest {
    private val vectors = Properties().apply {
        AuthTest::class.java.getResourceAsStream("/vectors/auth.properties")!!.use { load(it) }
    }
    private fun vector(name: String): ByteArray = hex(vectors.getProperty(name))
    private fun identity(): AuthIdentity = AuthIdentity(vector("identity_private"), vector("identity_public"))
    private fun ephemeral(): KeyPair = KeyPair(AuthCrypto.decode(vector("agreement_public")),
        KeyFactory.getInstance("EC").generatePrivate(PKCS8EncodedKeySpec(vector("agreement_private"))))
    private fun client(peer: ByteArray? = null): Auth = Auth(identity(), "phone", "relay", "192.168.49.108", peer)
    private fun start(auth: Auth, full: Boolean = false, confirmed: Boolean = false): Frame =
        auth.start(123, 7, vectors.getProperty("pin"), full, confirmed, ephemeral(), vector("phone_nonce")).frames.single()
    private fun response(body: ByteArray = vector("m2")): Frame = Frame(7, 124, 0, 1, 2, 3, 2, body)
    private fun replace(body: ByteArray, id: Int, value: FieldValue): ByteArray {
        val old = Fields.decode(body)
        return Fields(old.numbers.associateWith { if (it == id) value else old[it]!! }).encode()
    }
    @Test fun fullRequestMatchesOracleTranscript() {
        val frame = start(client())
        assertEquals(3, frame.category); assertEquals(1, frame.method); assertEquals(1, frame.kind)
        val actual = Fields.decode(frame.body)
        val expected = Fields.decode(vector("m1"))
        assertEquals(expected.numbers, actual.numbers)
        for (id in expected.numbers - 5) when (val field = expected[id]) {
            is FieldValue.Varint -> assertEquals(field.value, actual.number(id))
            is FieldValue.Bytes -> assertContentEquals(field.value, actual.bytes(id))
            null -> error("missing fixture field")
        }
        // ECDSA signatures are randomized DER values; verify their message instead of byte equality.
        assertTrue(Signature.getInstance("SHA256withECDSA").run {
            initVerify(AuthCrypto.decode(vector("identity_public")))
            update(vector("agreement_public") + vector("phone_nonce"))
            verify(actual.bytes(5))
        })
    }
    @Test fun fullResponseVersionsProduceOracleKeyAndSealedIpv4() {
        for (fixture in listOf("m2", "m2_0901")) {
            val auth = client(); start(auth)
            val output = auth.receive(response(vector(fixture)), 125)
            assertContentEquals(vector("session_key"), output.sessionKey)
            val confirm = output.frames.single()
            assertEquals(3, confirm.method); assertEquals(0, confirm.kind); assertEquals(8L, confirm.sequence)
            val address = SecureRecord(output.sessionKey!!).open(Fields.decode(confirm.body).bytes(1))
            assertEquals("192.168.49.108", address.toString(Charsets.UTF_8))
            val peer = output.events.filterIsInstance<AuthEvent.PeerVerified>().single()
            assertContentEquals(Fields.decode(vector("m2")).bytes(2), peer.publicKey)
            assertFails { auth.receive(response(), 126) }
        }
    }
    @Test fun quickReconnectOmitsIdentityAndIgnoresPinProof() {
        val peer = Fields.decode(vector("m2")).bytes(2)
        val auth = client(peer)
        val request = auth.start(123, 7, confirmed = true, ephemeral = ephemeral(), phoneNonce = vector("phone_nonce")).frames.single()
        val fields = Fields.decode(request.body)
        val expected = Fields.decode(vector("m1_quick"))
        assertEquals(expected.numbers, fields.numbers)
        assertNull(fields[2]); assertNull(fields[3]); assertEquals(1uL, fields.number(9))
        for (id in listOf(4, 6, 7, 8)) assertContentEquals(expected.bytes(id), fields.bytes(id))
        // Same oracle car signature transcript; quick responses may omit identity and proof entirely.
        val full = Fields.decode(vector("m2"))
        val body = Fields((full.numbers - setOf(2, 3)).associateWith { full[it]!! }).encode()
        assertContentEquals(vector("session_key"), auth.receive(response(body), 125).sessionKey)
    }
    @Test fun resultCodesAllowExplicitFreshRetry() {
        val cases = mapOf(1uL to AuthResult.PIN_REQUIRED, 2uL to AuthResult.CONFIRMATION_REQUIRED,
            3uL to AuthResult.INTERNAL_ERROR, 4uL to AuthResult.REJECTED, ULong.MAX_VALUE to AuthResult.REJECTED)
        for (version in listOf(1uL, 0x0901uL)) for ((code, expected) in cases) {
            val auth = client(); start(auth)
            val body = Fields(mapOf(1 to FieldValue.Varint(version), 7 to FieldValue.Varint(code))).encode()
            val output = auth.receive(response(body), 125)
            assertNull(output.sessionKey); assertTrue(output.frames.isEmpty())
            assertEquals(AuthEvent.Result(expected, code), output.events.single())
            assertEquals(1, start(auth, full = true, confirmed = true).method)
        }
    }
    @Test fun invalidProofSignaturePinnedKeyVersionAndFrameFailClosed() {
        val body = vector("m2")
        val cases = listOf(
            replace(body, 3, FieldValue.Bytes(ByteArray(32))),
            replace(body, 5, FieldValue.Bytes(byteArrayOf(1))),
            replace(body, 6, FieldValue.Bytes(ByteArray(31))),
            replace(body, 1, FieldValue.Varint(2uL)),
            replace(body, 4, FieldValue.Bytes(ByteArray(64))),
        )
        for (bad in cases) {
            val auth = client(); start(auth)
            val result = auth.receive(response(bad), 125)
            assertNull(result.sessionKey); assertTrue(result.frames.isEmpty())
            assertIs<AuthEvent.Failed>(result.events.single())
            assertFails { start(auth) }
        }
        val pinned = client(vector("identity_public")); start(pinned, full = true)
        assertEquals("peer_key_changed", (pinned.receive(response(), 125).events.single() as AuthEvent.Failed).reason)
        val quick = client(vector("identity_public")); start(quick)
        assertEquals("auth_signature_failed", (quick.receive(response(), 125).events.single() as AuthEvent.Failed).reason)
        val auth = client(); start(auth)
        assertIs<AuthEvent.Failed>(auth.receive(Frame(8, 0, 0, 1, 2, 3, 2, body), 125).events.single())
    }
}
