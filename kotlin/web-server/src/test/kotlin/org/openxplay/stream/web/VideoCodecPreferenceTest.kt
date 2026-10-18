// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-18

package org.openxplay.stream.web

import java.net.Socket
import java.nio.file.Path
import java.util.concurrent.CopyOnWriteArrayList
import kotlinx.serialization.json.*
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.io.TempDir
import org.openxplay.stream.core.*
import org.openxplay.stream.core.wire.WireFormatException
import kotlin.test.*

class VideoCodecPreferenceTest {
    @TempDir lateinit var root: Path

    @Test fun helloCodecIsOptionalAndStrict() {
        val hello = WebJsonCodec.parse(vector("hello"))
        for (mode in listOf("AUTO", "H264")) WebJsonCodec.validate(JsonObject(hello + ("videoCodec" to JsonPrimitive(mode))))
        for (bad in listOf(JsonPrimitive("H265"), JsonPrimitive(""), JsonPrimitive(1), JsonPrimitive(true), JsonNull)) {
            assertFailsWith<WireFormatException> { WebJsonCodec.validate(JsonObject(hello + ("videoCodec" to bad))) }
        }
    }

    @Test fun repeatedHelloRepublishesPreferenceAndPreservesHardware() {
        val reports = CopyOnWriteArrayList<OutputCapabilities>()
        val web = WebOutput(WebOutputConfig(port = 0, staticAssetRoot = root,
            sourceUrl = "https://example.org/source"))
        web.open(object : OutputHost {
            override fun capabilities(capabilities: OutputCapabilities) { reports.add(capabilities) }
            override fun control(event: ControlEvent) = Unit
            override fun stats(stats: OutputStats) = Unit
        })
        try {
            val port = web.awaitListening()
            TestClient(Socket("127.0.0.1", port)).use { first ->
                TestClient(Socket("127.0.0.1", port)).use { second ->
                    val hello = WebJsonCodec.parse(vector("hello-video-codec-h264"))
                    first.send(1, JsonObject(hello - "videoCodec").toString().toByteArray()); first.json()
                    eventually { reports.lastOrNull()?.displays?.containsKey(ScreenId.MAIN) == true }
                    assertFalse(reports.last().prefersH264)
                    second.send(1, hello.toString().toByteArray()); second.json()
                    eventually { reports.lastOrNull()?.prefersH264 == true }
                    assertTrue(reports.last().decoders.any { it.hardware && VideoCodecPolicy.isHevc(it.codecString) })
                    val secondary = JsonObject(hello + ("displays" to JsonArray(hello["displays"]!!.jsonArray.map {
                        JsonObject(it.jsonObject + ("screen" to JsonPrimitive(1)))
                    })))
                    second.send(1, secondary.toString().toByteArray())
                    eventually { reports.lastOrNull()?.prefersH264 == false }
                    second.send(1, hello.toString().toByteArray())
                    eventually { reports.lastOrNull()?.prefersH264 == true }
                    second.send(1, JsonObject(hello + ("videoCodec" to JsonPrimitive("AUTO"))).toString().toByteArray())
                    eventually { reports.lastOrNull()?.prefersH264 == false }
                }
            }
        } finally { web.close() }
    }
}
