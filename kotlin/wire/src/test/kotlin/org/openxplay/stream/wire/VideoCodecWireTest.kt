// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-18

package org.openxplay.stream.wire

import kotlinx.serialization.json.*
import org.junit.jupiter.api.Test
import org.openxplay.stream.core.wire.WireFormatException
import kotlin.test.*

class VideoCodecWireTest {
    @Test fun codecPreferenceDefaultsAndValidation() {
        val original = checkNotNull(javaClass.getResource("/vectors/wire/capabilities.json")).readText()
        val caps = (StreamWireCodec.decode(original.toByteArray()) as WireMessage.Capabilities).value
        assertFalse(caps.prefersH264)
        assertFalse("prefersH264" in Json.parseToJsonElement(StreamWireCodec.encode(WireMessage.Capabilities(caps)).toString(Charsets.UTF_8)).jsonObject)
        val obj = Json.parseToJsonElement(original).jsonObject
        for (preference in listOf(true, false)) {
            val bytes = JsonObject(obj + ("prefersH264" to JsonPrimitive(preference))).toString().toByteArray()
            val decoded = (StreamWireCodec.decode(bytes) as WireMessage.Capabilities).value
            assertEquals(preference, decoded.prefersH264)
            val roundTrip = (StreamWireCodec.decode(StreamWireCodec.encode(WireMessage.Capabilities(decoded))) as WireMessage.Capabilities).value
            assertEquals(decoded, roundTrip)
        }
        for (bad in listOf(JsonNull, JsonPrimitive(1), JsonPrimitive("true"))) {
            assertFailsWith<WireFormatException> {
                StreamWireCodec.decode(JsonObject(obj + ("prefersH264" to bad)).toString().toByteArray())
            }
        }
    }
}
