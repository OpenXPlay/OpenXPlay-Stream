// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

package org.openxplay.stream.wire

import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.PipedInputStream
import java.io.PipedOutputStream
import java.util.concurrent.CopyOnWriteArrayList
import kotlinx.serialization.json.*
import org.openxplay.stream.core.*
import kotlin.test.*

class ContextWireTest {
    private fun resource(path: String): String = checkNotNull(javaClass.getResource("/vectors/$path")).readText()
    @Test fun sharedWireAndTopicVectors() {
        for (name in listOf("context-session", "context-now-playing", "context-call", "context-activity", "context-clear", "context-route", "context-audio-source")) {
            val tree = Json.parseToJsonElement(resource("wire/$name.json"))
            val bytes = resource("wire/$name.hex").trim().split(Regex("\\s+")).map { it.toInt(16).toByte() }.toByteArray()
            val message = assertIs<WireMessage.Context>(StreamWireCodec.read(ByteArrayInputStream(bytes)))
            assertEquals(tree, Json.parseToJsonElement(StreamWireCodec.encode(message).toString(Charsets.UTF_8)))
            val out = ByteArrayOutputStream(); StreamWireCodec.write(out, message)
            assertEquals(tree, Json.parseToJsonElement(out.toByteArray().drop(4).toByteArray().toString(Charsets.UTF_8)))
        }
        for (name in listOf("valid", "invalid")) {
            for (case in Json.parseToJsonElement(resource("plugins/topics/$name.json")).jsonArray) {
                val o = JsonObject(case.jsonObject + ("type" to JsonPrimitive("context")))
                if (name == "valid") assertIs<WireMessage.Context>(StreamWireCodec.decode(o.toString().toByteArray()))
                else assertFails { StreamWireCodec.decode(o.toString().toByteArray()) }
            }
        }
    }
    @Test fun hubContextDeliveryRequiresExplicitOutputOptIn() {
        val transport = WireTransport.streams(ByteArrayInputStream(byteArrayOf()), ByteArrayOutputStream())
        assertFalse(WireOutput(transport).wantsContext())
        assertTrue(WireOutput(transport, contextEnabled = true).wantsContext())
    }
    @Test fun unknownMessageDoesNotConsumeTheFollowingFrame() {
        val out = ByteArrayOutputStream()
        StreamWireCodec.write(out, WireMessage.Unknown(buildJsonObject { put("type", "future-topic"); put("value", 42) }))
        StreamWireCodec.write(out, WireMessage.Restart)
        val input = ByteArrayInputStream(out.toByteArray())
        assertEquals(WireMessage.Restart, StreamWireCodec.read(input)); assertNull(StreamWireCodec.read(input))
    }
    @Test fun legacyConsumerGetsNoContextAndNewConsumerReceivesIt() {
        for (enabled in listOf(false, true)) {
            val producerInput = PipedInputStream(65536); val consumerOutput = PipedOutputStream(producerInput)
            val consumerInput = PipedInputStream(65536); val producerOutput = PipedOutputStream(consumerInput)
            val errors = CopyOnWriteArrayList<Throwable>()
            val peer = WireMessage.Hello(WireRole.CONSUMER, "wire-input", if (enabled) ContextNegotiation.client("default") else "default")
            // Both hellos obey the exact old five-field schema, with version 0.
            val tree = Json.parseToJsonElement(StreamWireCodec.encode(peer).toString(Charsets.UTF_8)).jsonObject
            assertEquals(setOf("type", "version", "role", "name", "client"), tree.keys)
            assertEquals(JsonPrimitive(0), tree["version"])
            val output = WireOutput(WireTransport.streams(producerInput, producerOutput), onError = { errors.add(it) })
            try {
                StreamWireCodec.write(consumerOutput, peer); consumerOutput.flush()
                output.open(object : OutputHost {
                    override fun capabilities(capabilities: OutputCapabilities) = Unit
                    override fun control(event: ControlEvent) = Unit
                    override fun stats(stats: OutputStats) = Unit
                })
                assertIs<WireMessage.Hello>(StreamWireCodec.read(consumerInput))
                assertIs<WireMessage.InputCapabilitiesMessage>(StreamWireCodec.read(consumerInput))
                output.onContext(ContextUpdate.Call(CallContext(CallState.ACTIVE)))
                // A session marker after the queued context proves the legacy stream stays usable.
                output.onSession(SessionState(SessionId(1), SessionPhase.ACTIVE), SessionConfig(emptyMap()))
                val first = StreamWireCodec.read(consumerInput)
                if (!enabled) assertIs<WireMessage.Session>(first)
                else {
                    // Control markers may precede context, but the next frame is the update.
                    val context = if (first is WireMessage.Context) first else StreamWireCodec.read(consumerInput)
                    assertEquals(ContextUpdate.Call(CallContext(CallState.ACTIVE)), assertIs<WireMessage.Context>(context).update)
                }
                assertTrue(errors.isEmpty(), errors.joinToString())
            } finally { output.close(); consumerInput.close(); consumerOutput.close() }
        }
    }
    @Test fun bindingsDeliverContextThroughInputSink() {
        val consumerInput = PipedInputStream(65536); val producerOutput = PipedOutputStream(consumerInput)
        val producerInput = PipedInputStream(65536); val consumerOutput = PipedOutputStream(producerInput)
        val updates = CopyOnWriteArrayList<ContextUpdate>(); val errors = CopyOnWriteArrayList<Throwable>()
        val input = WireInput(WireTransport.streams(consumerInput, consumerOutput), onError = { errors.add(it) })
        val output = WireOutput(WireTransport.streams(producerInput, producerOutput), onError = { errors.add(it) }, contextEnabled = true)
        val hub = ProjectionHub(FakeInput(), clock = ManualClock())
        val sink = object : InputSink {
            override fun context(update: ContextUpdate) { updates.add(update) }
            override fun session(state: SessionState) = Unit
            override fun videoFormat(screen: ScreenId, format: VideoFormat?) = Unit
            override fun videoFrame(frame: VideoFrame) = Unit
            override fun audioFormat(stream: Int, format: AudioFormat?) = Unit
            override fun audioPacket(packet: AudioPacket) = Unit
            override fun microphone(stream: Int, request: MicRequest?) = Unit
        }
        try {
            input.start(SessionConfig(emptyMap()), sink)
            hub.start(); hub.addOutput(output)
            val active = ContextUpdate.Call(CallContext(CallState.ACTIVE))
            hub.context(active); eventually { active in updates }
            val cleared = ContextUpdate.Call(null)
            hub.context(cleared); eventually { cleared in updates }
            assertTrue(errors.isEmpty(), errors.joinToString())
        } finally { hub.stop(); input.stop() }
    }
    @Test fun oldProducerStillFeedsNewInputAndUnknownMessagesAreIgnored() {
        val consumerInput = PipedInputStream(65536); val producerOutput = PipedOutputStream(consumerInput)
        val producerInput = PipedInputStream(65536); val consumerOutput = PipedOutputStream(producerInput)
        val states = CopyOnWriteArrayList<SessionState>(); val errors = CopyOnWriteArrayList<Throwable>()
        val input = WireInput(WireTransport.streams(consumerInput, consumerOutput), onError = { errors.add(it) })
        val sink = object : InputSink {
            override fun session(state: SessionState) { states.add(state) }
            override fun videoFormat(screen: ScreenId, format: VideoFormat?) = Unit
            override fun videoFrame(frame: VideoFrame) = Unit
            override fun audioFormat(stream: Int, format: AudioFormat?) = Unit
            override fun audioPacket(packet: AudioPacket) = Unit
            override fun microphone(stream: Int, request: MicRequest?) = Unit
        }
        try {
            input.start(SessionConfig(emptyMap()), sink)
            val hello = assertIs<WireMessage.Hello>(StreamWireCodec.read(producerInput))
            assertTrue(ContextNegotiation.supports(hello))
            StreamWireCodec.write(producerOutput, WireMessage.Hello(WireRole.PRODUCER, "old", "default"))
            StreamWireCodec.write(producerOutput, WireMessage.Unknown(buildJsonObject { put("type", "future") }))
            StreamWireCodec.write(producerOutput, WireMessage.Session(SessionState(SessionId(1), SessionPhase.ACTIVE)))
            producerOutput.flush()
            eventually { states.size == 1 }
            assertTrue(errors.isEmpty(), errors.joinToString())
        } finally { input.stop(); producerInput.close(); producerOutput.close() }
    }
}
