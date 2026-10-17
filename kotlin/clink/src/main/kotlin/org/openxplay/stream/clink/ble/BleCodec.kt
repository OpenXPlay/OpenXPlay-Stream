// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.clink.ble

import java.nio.ByteBuffer
import java.nio.charset.CodingErrorAction
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.put

public data class Advertisement(
    public val major: Int,
    public val minor: Int,
    public val serial: Int,
    public val carId: String,
    public val modelId: String,
    public val vendorData: String,
    public val name: String,
)
public data class PhoneRequest(
    public val id: String,
    public val name: String,
    public val model: String,
    public val band: Int,
    public val mac: String,
    public val type: Int = 1001,
)
public data class HeadUnitReply(
    public val name: String,
    public val ssid: String,
    public val psk: String,
    public val mac: String,
    public val freq: Int,
    public val port: Int,
    public val type: Int,
)

/** Pure codecs. GATT payloads are complete UTF-8 JSON without a length prefix. */
public object BleCodec {
    public const val ADVERTISEMENT_SERVICE: String = "0000fcfb-0000-1000-8000-00805f9b34fb"
    public const val IDENTITY_SERVICE: String = "00000001-0000-1000-8000-00805f9b34fb"
    public const val NAME_SERVICE: String = "00000002-0000-1000-8000-00805f9b34fb"
    public const val GATT_SERVICE: String = "2abcc850-9935-4f8a-ba84-123456789100"
    public const val PHONE_CHARACTERISTIC: String = "2abcc850-9935-4f8a-ba84-123456789101"
    public const val HEAD_UNIT_CHARACTERISTIC: String = "2abcc850-9935-4f8a-ba84-123456789102"
    public const val CCCD: String = "00002902-0000-1000-8000-00805f9b34fb"

    public fun advertisement(identity: ByteArray, name: ByteArray = byteArrayOf()): Advertisement {
        // The Go reference also accepts the observed 1.9 record with an extra trailing byte.
        require(identity.size == 15 || identity.size == 16 && identity[0] == 1.toByte() && identity[1] == 9.toByte())
        require(name.size <= 16)
        val text = utf8(name).trimEnd('\u0000')
        require(validText(text, 16, true))
        fun hex(start: Int, end: Int): String = identity.copyOfRange(start, end).joinToString("") { "%02x".format(it) }
        return Advertisement(identity[0].toInt() and 255, identity[1].toInt() and 255,
            identity[2].toInt() and 255, hex(3, 9), hex(9, 13), hex(13, 15), text)
    }
    public fun phoneRequest(phone: PhoneRequest): ByteArray {
        require(validText(phone.id, 128, false) && validText(phone.name, 128, false) && validText(phone.model, 128, false))
        val type = if (phone.type == 0) 1001 else phone.type
        require(validMac(phone.mac) && phone.band in 0..1 && type in 1001..1002)
        return buildJsonObject {
            put("id", phone.id); put("name", phone.name); put("model", phone.model)
            put("band", phone.band); put("mac", phone.mac.lowercase()); put("type", type)
        }.toString().toByteArray().also { require(it.size <= 512) }
    }
    public fun headUnitReply(bytes: ByteArray): HeadUnitReply {
        require(bytes.size <= 4096)
        val text = utf8(bytes)
        rejectDuplicateKeys(text)
        val obj = Json.parseToJsonElement(text) as? JsonObject ?: error("invalid_network_bootstrap")
        fun string(key: String): String {
            val value = obj[key] as? JsonPrimitive ?: error("missing_$key")
            require(value.isString)
            return value.content
        }
        fun integer(key: String): Int {
            val value = obj[key] as? JsonPrimitive ?: error("missing_$key")
            require(!value.isString)
            return value.intOrNull ?: error("invalid_$key")
        }
        val reply = HeadUnitReply(string("name"), string("ssid"), string("psk"), string("mac").lowercase(),
            integer("freq"), integer("port"), integer("type"))
        require(validText(reply.name, 256, true) && validText(reply.ssid, 32, false) && validText(reply.psk, 64, false))
        require(reply.psk.toByteArray().size >= 8 && validMac(reply.mac))
        require(reply.freq in 0..7125 && reply.port in 0..65535 && reply.type in 1001..1002)
        return reply
    }
    private fun validMac(value: String): Boolean = Regex("[0-9a-fA-F]{2}(:[0-9a-fA-F]{2}){5}").matches(value)
    private fun validText(value: String, max: Int, empty: Boolean): Boolean =
        (empty || value.isNotEmpty()) && value.toByteArray().size <= max && value.none { Character.isISOControl(it) }
    private fun utf8(bytes: ByteArray): String = Charsets.UTF_8.newDecoder()
        .onMalformedInput(CodingErrorAction.REPORT).onUnmappableCharacter(CodingErrorAction.REPORT)
        .decode(ByteBuffer.wrap(bytes)).toString()
    // JsonElement maps collapse duplicates, so inspect keys before constructing that tree.
    private fun rejectDuplicateKeys(text: String) {
        val stack = mutableListOf<MutableSet<String>?>()
        var i = 0
        while (i < text.length) {
            when (text[i]) {
                '{' -> stack.add(mutableSetOf())
                '[' -> stack.add(null)
                '}', ']' -> { require(stack.isNotEmpty()); stack.removeAt(stack.lastIndex) }
                '"' -> {
                    val start = i++
                    while (i < text.length && text[i] != '"') {
                        if (text[i] == '\\') i++
                        i++
                    }
                    require(i < text.length)
                    var next = i + 1
                    while (next < text.length && text[next].isWhitespace()) next++
                    if (next < text.length && text[next] == ':') {
                        val key = (Json.parseToJsonElement(text.substring(start, i + 1)) as JsonPrimitive).content
                        val keys = stack.lastOrNull() ?: error("invalid_json_key")
                        require(keys.add(key)) { "duplicate_json_key" }
                    }
                }
            }
            require(stack.size <= 16) { "network_bootstrap_limit" }
            i++
        }
    }
}
