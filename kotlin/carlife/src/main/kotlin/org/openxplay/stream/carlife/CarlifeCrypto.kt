// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-11-02

package org.openxplay.stream.carlife

import java.security.KeyFactory
import java.security.MessageDigest
import java.security.SecureRandom
import java.security.spec.X509EncodedKeySpec
import java.util.Base64
import javax.crypto.Cipher
import javax.crypto.spec.SecretKeySpec

internal class CarlifeCrypto {
    private var key: ByteArray? = null
    @Volatile var active = false
        private set

    fun negotiate(publicKey: String): String {
        val parsed = KeyFactory.getInstance("RSA").generatePublic(
            X509EncodedKeySpec(Base64.getDecoder().decode(publicKey)))
        val fresh = ByteArray(16).also { SecureRandom().nextBytes(it) }
        val encrypted = Cipher.getInstance("RSA/ECB/PKCS1Padding").run {
            init(Cipher.ENCRYPT_MODE, parsed)
            doFinal(fresh)
        }
        key = fresh
        return Base64.getEncoder().encodeToString(encrypted)
    }

    fun activate() {
        protocolRequire(key != null, "unexpected_phase")
        active = true
    }

    fun transform(message: CarlifeMessage, encrypt: Boolean): CarlifeMessage {
        if (!active || message.id == 98305 || message.payload.isEmpty()) return message
        val cipher = Cipher.getInstance("AES/ECB/PKCS5Padding")
        cipher.init(if (encrypt) Cipher.ENCRYPT_MODE else Cipher.DECRYPT_MODE, SecretKeySpec(checkNotNull(key), "AES"))
        return message.copy(payload = cipher.doFinal(message.payload))
    }

    companion object {
        fun authenticate(info: CarlifeDeviceInfo, challenge: String): String {
            protocolRequire(challenge.startsWith("2.0;") && challenge.length > 4, "unsupported_auth_challenge")
            val text = "CARLIFE" + info.brand + info.model + info.sdk + challenge.substring(4)
            return MessageDigest.getInstance("MD5").digest(text.toByteArray(Charsets.UTF_8))
                .joinToString("") { (it.toInt() and 255).toString(16).padStart(2, '0') }
        }
    }
}
