// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

package org.openxplay.stream.core

import kotlinx.serialization.json.*
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.test.*

class ContextTest {
    @Test fun sharedTopicVectors() {
        for (name in listOf("valid", "invalid")) {
            val text = checkNotNull(javaClass.getResource("/vectors/plugins/topics/$name.json")).readText()
            for (case in Json.parseToJsonElement(text).jsonArray) {
                if (name == "valid") assertEquals(case, ContextJson.encode(ContextJson.decode(case.jsonObject)))
                else assertFails(case.toString()) { ContextJson.decode(case.jsonObject) }
            }
        }
    }
    @Test fun registryRelayVectors() {
        val text = checkNotNull(javaClass.getResource("/vectors/plugins/provider-relay.json")).readText()
        for (case in Json.parseToJsonElement(text).jsonObject.getValue("vectors").jsonArray) {
            val c = case.jsonObject
            if (c.getValue("topic").jsonPrimitive.content !in setOf("route", "audioSource")) continue
            val update = buildJsonObject { put("topic", c.getValue("topic")); put("value", c.getValue("value")) }
            if (c.getValue("valid").jsonPrimitive.boolean) {
                val decoded = ContextJson.decode(update)
                assertEquals(decoded, ContextJson.decode(ContextJson.encode(decoded)))
            } else assertFails { ContextJson.decode(update) }
        }
    }
    private fun await(predicate: () -> Boolean) {
        val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(3)
        while (!predicate()) { assertTrue(System.nanoTime() < deadline, "Context timed out"); Thread.sleep(2) }
    }
    private fun output(id: String, updates: MutableList<ContextUpdate>): ProjectionOutput =
        object : ProjectionOutput by RecordingOutput(id) {
            override fun wantsContext(): Boolean = true
            override fun onContext(update: ContextUpdate) { updates.add(update) }
        }
    @Test fun replayDedupClearingAndSessionEnd() {
        val hub = ProjectionHub(FakeInput(), clock = ManualClock())
        val one = CopyOnWriteArrayList<ContextUpdate>()
        val late = CopyOnWriteArrayList<ContextUpdate>()
        val idle = CopyOnWriteArrayList<ContextUpdate>()
        try {
            hub.start(); hub.addOutput(output("one", one)); await { one.size == ContextUpdate.idle().size }
            val connected = ContextUpdate.Session(SessionContext(CarPlayState.CONNECTED))
            hub.context(connected); hub.context(connected); await { one.size == ContextUpdate.idle().size + 1 }
            hub.addOutput(output("late", late)); await { late.size == ContextUpdate.idle().size }
            assertTrue(connected in late)
            hub.context(ContextUpdate.Call(null)); await { one.size == ContextUpdate.idle().size + 2 }
            hub.session(SessionState(SessionId(1), SessionPhase.ENDED)); await { one.size == ContextUpdate.idle().size + 4 }
            hub.addOutput(output("idle", idle)); await { idle.size == ContextUpdate.idle().size }
            assertEquals(ContextUpdate.idle().toSet(), idle.toSet())
        } finally { hub.stop() }
    }
    @Test fun slowOutputNeverBlocksPublisherOrOtherOutputAndStopResetsReplay() {
        val hub = ProjectionHub(FakeInput(), clock = ManualClock())
        val entered = CountDownLatch(1); val release = CountDownLatch(1)
        val fast = CopyOnWriteArrayList<ContextUpdate>()
        try {
            hub.start()
            hub.addOutput(object : ProjectionOutput by RecordingOutput("slow") {
                override fun wantsContext(): Boolean = true
                override fun onContext(update: ContextUpdate) { entered.countDown(); release.await() }
            })
            assertTrue(entered.await(3, TimeUnit.SECONDS))
            hub.addOutput(output("fast", fast)); await { fast.size == ContextUpdate.idle().size }
            val active = ContextUpdate.Call(CallContext(CallState.ACTIVE))
            hub.context(active); await { active in fast }
            release.countDown(); hub.stop()
            val idle = CopyOnWriteArrayList<ContextUpdate>()
            hub.start(); hub.addOutput(output("idle", idle)); await { idle.size == ContextUpdate.idle().size }
            assertEquals(ContextUpdate.idle().toSet(), idle.toSet())
        } finally { release.countDown(); hub.stop() }
    }
    private fun contextThreads(id: String): List<Thread> =
        Thread.getAllStackTraces().keys.filter { it.isAlive && it.name == "projection-context-$id" }

    @Test fun defaultOutputNeverCreatesContextWorkerOrReceivesContext() {
        val hub = ProjectionHub(FakeInput(), clock = ManualClock())
        val updates = CopyOnWriteArrayList<ContextUpdate>()
        val output = object : ProjectionOutput by RecordingOutput("default-context-disabled") {
            override fun onContext(update: ContextUpdate) { updates.add(update) }
        }
        assertFalse(output.wantsContext())
        try {
            hub.start(); hub.addOutput(output)
            hub.context(ContextUpdate.Call(null))
            hub.session(SessionState(SessionId(1), SessionPhase.ENDED))
            // Open schedules executor work synchronously if context delivery was allocated.
            assertTrue(contextThreads(output.id).isEmpty())
            assertTrue(updates.isEmpty())
            hub.removeOutput(output.id)
            assertTrue(contextThreads(output.id).isEmpty())
        } finally { hub.stop() }
        assertTrue(updates.isEmpty())
    }

    @Test fun contextFailureDisablesOnlyContextAndKeepsVideo() {
        val failures = CopyOnWriteArrayList<Throwable>()
        val hub = ProjectionHub(FakeInput(), clock = ManualClock(), listener = object : HubListener {
            override fun outputFailed(outputId: String, failure: Throwable) { failures.add(failure) }
        })
        val frames = CopyOnWriteArrayList<VideoFrame>()
        val callbacks = java.util.concurrent.atomic.AtomicInteger()
        val closed = java.util.concurrent.atomic.AtomicBoolean()
        val output = object : ProjectionOutput by RecordingOutput("failing-context") {
            override fun wantsContext(): Boolean = true
            override fun onContext(update: ContextUpdate) {
                callbacks.incrementAndGet()
                throw AssertionError("broken context callback")
            }
            override fun onVideoFrame(frame: VideoFrame) { frames.add(frame) }
            override fun close() { closed.set(true) }
        }
        try {
            hub.start(); hub.addOutput(output)
            await { failures.size == 1 && contextThreads(output.id).isEmpty() }
            repeat(3) {
                hub.context(ContextUpdate.Call(null))
                hub.videoFrame(VideoFrame(ScreenId(0), SessionId(1), byteArrayOf(1), 0, true, 0))
            }
            assertEquals(1, callbacks.get()); assertEquals(1, failures.size)
            assertEquals(3, frames.size); assertFalse(closed.get())
        } finally { hub.stop() }
        assertTrue(closed.get())
    }

    @Test fun contextWorkerWaitsForEventsAndExitsOnRemoveOrStop() {
        for (remove in listOf(true, false)) {
            val hub = ProjectionHub(FakeInput(), clock = ManualClock())
            val updates = CopyOnWriteArrayList<ContextUpdate>()
            val id = "waiting-context-$remove"
            try {
                hub.start(); hub.addOutput(output(id, updates))
                await { updates.size == ContextUpdate.idle().size && contextThreads(id).singleOrNull()?.state == Thread.State.WAITING }
                val active = ContextUpdate.Call(CallContext(CallState.ACTIVE))
                hub.context(active); await { active in updates }
                if (remove) hub.removeOutput(id) else hub.stop()
                await { contextThreads(id).isEmpty() }
            } finally { hub.stop() }
        }
    }

}
