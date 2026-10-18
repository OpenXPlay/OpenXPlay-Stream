// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-18
package org.openxplay.stream.web

import java.net.Socket
import java.nio.file.Files
import java.nio.file.Path
import kotlinx.serialization.json.*
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.io.TempDir
import org.openxplay.stream.core.*
import org.openxplay.stream.core.wire.WireFormatException
import kotlin.test.*

class ReceiverBuildTest {
    @TempDir lateinit var root: Path
    @Test fun readsOnlyTheBuildFileInTheServedAssetRoot() {
        assertNull(readReceiverBuild(root))
        val file = root.resolve("receiver-build.txt")
        Files.writeString(file, "  \n")
        assertNull(readReceiverBuild(root))
        Files.writeString(file, "build-a\n")
        assertEquals("build-a", readReceiverBuild(root))
        Files.createDirectories(root.resolve("receiver"))
        Files.writeString(root.resolve("receiver/receiver-build.txt"), "wrong-build")
        assertEquals("build-a", readReceiverBuild(root))
        Files.delete(file); Files.createDirectory(file)
        assertNull(readReceiverBuild(root))
    }
    @Test fun welcomeBuildIsOptionalAndStrictlyANonemptyString() {
        val state = SessionState(SessionId(42), SessionPhase.ACTIVE)
        val url = "https://example.org/source"
        val golden = WebJsonCodec.parse(vector("welcome-receiver-build"), WebJsonCodec.Direction.RELAY_TO_CLIENT)
        assertEquals(golden, receiverWelcome("browser-1", state, golden["sourceUrl"]!!.jsonPrimitive.content,
            receiverBuild = golden["receiverBuild"]!!.jsonPrimitive.content))
        assertNull(receiverWelcome("browser-1", state, url)["receiverBuild"])
        WebJsonCodec.validate(JsonObject(golden - "receiverBuild"))
        for (bad in listOf(JsonPrimitive(""), JsonNull, JsonPrimitive(1), JsonPrimitive(true), JsonObject(emptyMap()), JsonArray(emptyList())))
            assertFailsWith<WireFormatException> { WebJsonCodec.validate(JsonObject(golden + ("receiverBuild" to bad))) }
        assertFailsWith<WireFormatException> { WebJsonCodec.validate(golden, WebJsonCodec.Direction.CLIENT_TO_RELAY) }
    }
    @Test fun outputWelcomesWithTheBuildOfItsAssetsAndOmitsItForLegacyAssets() {
        for (build in listOf(null, "build-a", "build-b")) {
            val file = root.resolve("receiver-build.txt")
            if (build == null) Files.deleteIfExists(file) else Files.writeString(file, "$build\n")
            val output = WebOutput(WebOutputConfig(port = 0, staticAssetRoot = root, sourceUrl = "https://example.org/source"))
            output.open(object : OutputHost {
                override fun capabilities(value: OutputCapabilities) = Unit
                override fun control(event: ControlEvent) = Unit
                override fun stats(stats: OutputStats) = Unit
            })
            try {
                TestClient(Socket("127.0.0.1", output.awaitListening())).use { client ->
                    client.hello()
                    val welcome = client.json()
                    WebJsonCodec.validate(welcome, WebJsonCodec.Direction.RELAY_TO_CLIENT)
                    assertEquals(build?.let(::JsonPrimitive), welcome["receiverBuild"])
                }
            } finally { output.close() }
        }
    }
}
