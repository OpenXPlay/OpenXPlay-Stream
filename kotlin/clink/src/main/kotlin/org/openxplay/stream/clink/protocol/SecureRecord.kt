// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.clink.protocol

import java.nio.ByteBuffer
import java.security.MessageDigest
import java.security.SecureRandom
import javax.crypto.Cipher
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.SecretKeySpec

/** Per-direction, per-channel state. Thread confined; recreate for each session.
 * Outbound IVs are random. Inbound replay history retains the last 4096 authenticated records.
 */
public class SecureRecord(key: ByteArray, private val random: SecureRandom = SecureRandom()) {
    private val secret: ByteArray = key.copyOf()
    private var buffer: ByteArray = byteArrayOf()
    private val seen: LinkedHashSet<String> = linkedSetOf()
    init { require(secret.size == 16) { "invalid_session_key" } }
    public val pendingBytes: Int get() = buffer.size
    public fun seal(plain: ByteArray): ByteArray {
        require(plain.size <= Frame.MAX_BYTES - 40)
        val iv = ByteArray(12).also(random::nextBytes)
        val cipher = cipher(Cipher.ENCRYPT_MODE, iv).doFinal(plain)
        return ByteBuffer.allocate(20 + cipher.size).putInt(12).put(iv).putInt(cipher.size).put(cipher).array()
    }
    private fun cipher(mode: Int, iv: ByteArray): Cipher = Cipher.getInstance("AES/GCM/NoPadding").apply {
        init(mode, SecretKeySpec(secret, "AES"), GCMParameterSpec(128, iv))
    }
    private fun decrypt(record: ByteArray): ByteArray {
        val n = ByteBuffer.wrap(record).int
        val fingerprint = MessageDigest.getInstance("SHA-256").digest(record.copyOfRange(4, record.size))
            .joinToString("") { "%02x".format(it) }
        require(fingerprint !in seen) { "replayed_cipher_record" }
        val plain = cipher(Cipher.DECRYPT_MODE, record.copyOfRange(4, 4 + n))
            .doFinal(record.copyOfRange(n + 8, record.size))
        if (seen.size == 4096) seen.remove(seen.first())
        seen.add(fingerprint)
        return plain
    }
    public fun open(record: ByteArray): ByteArray {
        require(buffer.isEmpty()) { "partial_record_pending" }
        require(record.size >= 8)
        val n = ByteBuffer.wrap(record).int
        require(n in 12..32 && record.size >= n + 8) { "invalid_cipher_record" }
        val size = ByteBuffer.wrap(record, n + 4, 4).int
        require(size in 16..Frame.MAX_BYTES && record.size.toLong() == n + 8L + size)
        return decrypt(record)
    }
    /** Feed fragments; discard this instance after malformed data or authentication failure. */
    public fun feed(bytes: ByteArray): List<ByteArray> {
        require(buffer.size.toLong() + bytes.size <= Frame.MAX_BYTES + 65576L)
        buffer += bytes
        val out = mutableListOf<ByteArray>()
        var offset = 0
        while (buffer.size - offset >= 4) {
            val n = ByteBuffer.wrap(buffer, offset, 4).int
            require(n in 12..32) { "invalid_cipher_record" }
            if (buffer.size - offset < n + 8) break
            val size = ByteBuffer.wrap(buffer, offset + n + 4, 4).int
            require(size in 16..Frame.MAX_BYTES) { "invalid_cipher_record" }
            val total = n + 8 + size
            if (buffer.size - offset < total) break
            out += decrypt(buffer.copyOfRange(offset, offset + total))
            offset += total
        }
        buffer = buffer.copyOfRange(offset, buffer.size)
        return out
    }
}
