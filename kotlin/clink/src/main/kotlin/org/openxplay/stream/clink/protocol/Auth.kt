// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.clink.protocol

import java.math.BigInteger
import java.security.AlgorithmParameters
import java.security.KeyFactory
import java.security.KeyPair
import java.security.KeyPairGenerator
import java.security.MessageDigest
import java.security.SecureRandom
import java.security.Signature
import java.security.interfaces.ECPrivateKey
import java.security.interfaces.ECPublicKey
import java.security.spec.ECGenParameterSpec
import java.security.spec.ECParameterSpec
import java.security.spec.ECPoint
import java.security.spec.ECPublicKeySpec
import java.security.spec.PKCS8EncodedKeySpec
import javax.crypto.KeyAgreement
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

/** Persistent P-256 signing identity: PKCS#8 private key and raw 64-byte X || Y public key. */
public class AuthIdentity(pkcs8: ByteArray, publicKey: ByteArray) {
    private val encoded: ByteArray = pkcs8.copyOf()
    private val raw: ByteArray = publicKey.copyOf()
    public val pkcs8: ByteArray get() = encoded.copyOf()
    public val publicKey: ByteArray get() = raw.copyOf()
    internal val pair: KeyPair = KeyPair(AuthCrypto.decode(raw),
        KeyFactory.getInstance("EC").generatePrivate(PKCS8EncodedKeySpec(encoded)))
    init {
        val key = pair.private as? ECPrivateKey ?: error("invalid_persistent_key")
        require(key.params.order == AuthCrypto.parameters.order && key.params.generator == AuthCrypto.parameters.generator)
        val proof = Signature.getInstance("SHA256withECDSA").run {
            initSign(pair.private); update(byteArrayOf(1)); sign()
        }
        require(Signature.getInstance("SHA256withECDSA").run {
            initVerify(pair.public); update(byteArrayOf(1)); verify(proof)
        }) { "identity_key_mismatch" }
    }
    public companion object {
        public fun generate(): AuthIdentity = AuthCrypto.generate().let { AuthIdentity(it.private.encoded, AuthCrypto.raw(it)) }
    }
}

public enum class AuthResult { SUCCESS, PIN_REQUIRED, CONFIRMATION_REQUIRED, INTERNAL_ERROR, REJECTED }
public sealed interface AuthEvent {
    public data class Result(public val result: AuthResult, public val wireCode: ULong) : AuthEvent
    /** Verified identity only. The caller decides when to persist trust after later checks. */
    public class PeerVerified(key: ByteArray) : AuthEvent {
        private val raw: ByteArray = key.copyOf()
        public val publicKey: ByteArray get() = raw.copyOf()
    }
    public data class Failed(public val reason: String) : AuthEvent
}
public class AuthOutput(
    frames: List<Frame> = emptyList(),
    sessionKey: ByteArray? = null,
    events: List<AuthEvent> = emptyList(),
) {
    public val frames: List<Frame> = frames.toList()
    private val key: ByteArray? = sessionKey?.copyOf()
    public val sessionKey: ByteArray? get() = key?.copyOf()
    public val events: List<AuthEvent> = events.toList()
}

/** Phone-side handshake, confined to one owner thread. No transport, prompts or persistence.
 * Call start again with fresh material after PIN_REQUIRED or CONFIRMATION_REQUIRED.
 * A completed instance cannot be reused. Receipt of m2 establishes cryptographic trust only;
 * frames in the successful output must be sent before treating AUTH as completed externally.
 */
public class Auth(
    private val identity: AuthIdentity,
    private val deviceId: String,
    private val name: String,
    private val localIpv4: String,
    pinnedPeer: ByteArray? = null,
    private val random: SecureRandom = SecureRandom(),
) {
    private val peer: ByteArray? = pinnedPeer?.copyOf()
    private var agreement: KeyPair? = null
    private var nonce: ByteArray = byteArrayOf()
    private var pin: ByteArray = byteArrayOf()
    private var quick: Boolean = false
    private var sequence: Long = 0
    private var complete: Boolean = false
    init {
        require(deviceId.isNotEmpty() && name.isNotEmpty())
        val parts = localIpv4.split('.')
        require(parts.size == 4 && parts.all { it.isNotEmpty() && it.all(Char::isDigit) && (it.toIntOrNull() ?: -1) in 0..255 })
        peer?.let { AuthCrypto.decode(it) }
    }
    /** Ephemeral key and nonce injection permits offline interoperability fixtures.
     * Production callers must use fresh default values on every attempt.
     */
    public fun start(
        unixSeconds: Long,
        sequence: Long,
        pin: String? = null,
        forceFull: Boolean = false,
        confirmed: Boolean = false,
        ephemeral: KeyPair = AuthCrypto.generate(),
        phoneNonce: ByteArray = ByteArray(32).also(random::nextBytes),
    ): AuthOutput {
        check(!complete && agreement == null) { "unexpected_auth_request" }
        require(sequence in 0..0xffffffffL && phoneNonce.size == 32)
        require(ephemeral.private is ECPrivateKey && ephemeral.public is ECPublicKey)
        require((ephemeral.private as ECPrivateKey).params.order == AuthCrypto.parameters.order)
        quick = peer != null && !forceFull
        this.pin = pin?.toByteArray(Charsets.UTF_8) ?: byteArrayOf()
        require(this.pin.size <= 64 && (this.pin.isEmpty() || this.pin.size >= 6))
        require(quick || this.pin.size >= 6) { "pin_required" }
        agreement = ephemeral
        nonce = phoneNonce.copyOf()
        this.sequence = sequence
        val raw = AuthCrypto.raw(ephemeral)
        val signature = Signature.getInstance("SHA256withECDSA").run {
            initSign(identity.pair.private, random); update(raw + nonce); sign()
        }
        val values = mutableMapOf<Int, FieldValue>(1 to FieldValue.Varint(1uL),
            4 to FieldValue.Bytes(raw), 5 to FieldValue.Bytes(signature), 6 to FieldValue.Bytes(nonce),
            7 to FieldValue.Bytes(deviceId.toByteArray()), 8 to FieldValue.Bytes(name.toByteArray()),
            9 to FieldValue.Varint(if (confirmed) 1uL else 0uL))
        if (!quick) {
            values[2] = FieldValue.Bytes(identity.publicKey)
            values[3] = FieldValue.Bytes(AuthCrypto.proof(this.pin, nonce, identity.publicKey))
        }
        return AuthOutput(listOf(Frame(sequence, unixSeconds, 1, 1, 1, 3, 1, Fields(values).encode())))
    }
    public fun receive(frame: Frame, unixSeconds: Long): AuthOutput {
        check(!complete && agreement != null) { "unexpected_auth_response" }
        return try {
            require(frame.category == 3 && frame.method == 2 && frame.kind == 2 && frame.sequence == sequence)
            require(frame.source == 0 && frame.format == 1) { "invalid_auth_header" }
            val f = Fields.decode(frame.body)
            require(f.number(1) == 1uL || f.number(1) == 0x0901uL) { "auth_version_unsupported" }
            val result = f.number(7)
            if (result != 0uL) {
                agreement = null
                val event = when (result) {
                    1uL -> AuthResult.PIN_REQUIRED
                    2uL -> AuthResult.CONFIRMATION_REQUIRED
                    3uL -> AuthResult.INTERNAL_ERROR
                    else -> AuthResult.REJECTED
                }
                AuthOutput(events = listOf(AuthEvent.Result(event, result)))
            } else {
                val carNonce = f.bytes(6, 32)
                val carAgreement = f.bytes(4, 64)
                val signature = f.bytes(5)
                require(signature.size in 1..80) { "invalid_auth_signature" }
                val carIdentity = if (quick) checkNotNull(peer) else f.bytes(2, 64).also {
                    require(MessageDigest.isEqual(f.bytes(3, 32), AuthCrypto.proof(pin, carNonce + nonce, it + identity.publicKey))) { "auth_pin_proof_failed" }
                    require(peer == null || MessageDigest.isEqual(peer, it)) { "peer_key_changed" }
                }
                val ep = checkNotNull(agreement)
                require(Signature.getInstance("SHA256withECDSA").run {
                    initVerify(AuthCrypto.decode(carIdentity))
                    update(carAgreement + AuthCrypto.raw(ep) + carNonce + nonce)
                    verify(signature)
                }) { "auth_signature_failed" }
                val shared = KeyAgreement.getInstance("ECDH").run {
                    init(ep.private); doPhase(AuthCrypto.decode(carAgreement), true); generateSecret()
                }
                val prk = AuthCrypto.hmac(AuthCrypto.hash(nonce + carNonce), shared)
                val key = AuthCrypto.hmac(prk, "session_key".toByteArray() + byteArrayOf(1)).copyOf(16)
                val record = SecureRecord(key, random).seal(localIpv4.toByteArray())
                val confirmation = Frame((sequence + 1) and 0xffffffffL, unixSeconds, 1, 1, 0, 3, 3,
                    Fields(mapOf(1 to FieldValue.Bytes(record))).encode())
                complete = true
                agreement = null
                pin.fill(0)
                AuthOutput(listOf(confirmation), key,
                    listOf(AuthEvent.Result(AuthResult.SUCCESS, 0uL), AuthEvent.PeerVerified(carIdentity)))
            }
        } catch (e: Exception) {
            agreement = null
            complete = true
            pin.fill(0)
            AuthOutput(events = listOf(AuthEvent.Failed(e.message ?: "authentication_failed")))
        }
    }
}

internal object AuthCrypto {
    val parameters: ECParameterSpec = AlgorithmParameters.getInstance("EC").run {
        init(ECGenParameterSpec("secp256r1")); getParameterSpec(ECParameterSpec::class.java)
    }
    fun generate(): KeyPair = KeyPairGenerator.getInstance("EC").run {
        initialize(ECGenParameterSpec("secp256r1")); generateKeyPair()
    }
    fun raw(pair: KeyPair): ByteArray {
        val w = (pair.public as ECPublicKey).w
        fun coordinate(n: BigInteger): ByteArray {
            val b = n.toByteArray()
            return ByteArray(32).also { b.copyInto(it, 32 - minOf(32, b.size), maxOf(0, b.size - 32)) }
        }
        return coordinate(w.affineX) + coordinate(w.affineY)
    }
    fun decode(raw: ByteArray): ECPublicKey {
        require(raw.size == 64) { "invalid_peer_key" }
        val x = BigInteger(1, raw.copyOfRange(0, 32))
        val y = BigInteger(1, raw.copyOfRange(32, 64))
        val p = (parameters.curve.field as java.security.spec.ECFieldFp).p
        require(x < p && y < p && y.multiply(y).mod(p) ==
            x.pow(3).add(parameters.curve.a.multiply(x)).add(parameters.curve.b).mod(p)) { "invalid_peer_key" }
        return KeyFactory.getInstance("EC").generatePublic(ECPublicKeySpec(ECPoint(x, y), parameters)) as ECPublicKey
    }
    fun hash(bytes: ByteArray): ByteArray = MessageDigest.getInstance("SHA-256").digest(bytes)
    fun hmac(key: ByteArray, bytes: ByteArray): ByteArray = Mac.getInstance("HmacSHA256").run {
        init(SecretKeySpec(key, "HmacSHA256")); doFinal(bytes)
    }
    fun proof(pin: ByteArray, nonce: ByteArray, bytes: ByteArray): ByteArray = hmac(hash(nonce + pin), bytes)
}
