// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-25

package org.openxplay.stream.core

import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class ProjectionHubHostUiTest {
    private fun output(id: String): RecordingOutput = object : RecordingOutput(id) {
        override fun onHostUiRequested() { events.add("host-ui") }
    }

    @Test fun forwardsEachRequestToAttachedOutputsWithoutReplay() {
        val input = FakeInput()
        val hub = ProjectionHub(input, clock = ManualClock())
        val first = output("first")
        val second = output("second")
        hub.addOutput(first); hub.addOutput(second)
        try {
            hub.hostUiRequested()
            assertTrue(first.events.isEmpty())
            hub.start()
            input.emit { it.hostUiRequested(); it.hostUiRequested() }
            assertEquals(2, first.events.count { it == "host-ui" })
            assertEquals(2, second.events.count { it == "host-ui" })
            val late = output("late")
            hub.addOutput(late)
            assertFalse("host-ui" in late.events)
            hub.removeOutput(first.id)
            input.emit { it.hostUiRequested() }
            assertEquals(2, first.events.count { it == "host-ui" })
            assertEquals(3, second.events.count { it == "host-ui" })
            assertEquals(1, late.events.count { it == "host-ui" })
        } finally { hub.stop() }
    }

    @Test fun oldInputEpochCannotRequestHostUiAfterRestartOrStop() {
        val input = FakeInput().apply { reconfigureResult = ReconfigureResult.RESTART_SESSION }
        val hub = ProjectionHub(input, clock = ManualClock())
        val recorder = output("output")
        hub.addOutput(recorder)
        try {
            hub.start()
            val oldSink = input.sink!!
            oldSink.hostUiRequested()
            input.emit { it.inputCapabilities(input.capabilities.copy(screens = setOf(ScreenId.MAIN, ScreenId.CLUSTER))) }
            hub.requestRestart()
            assertEquals(2, input.starts.size)
            val currentSink = input.sink!!
            oldSink.hostUiRequested()
            assertEquals(1, recorder.events.count { it == "host-ui" })
            currentSink.hostUiRequested()
            assertEquals(2, recorder.events.count { it == "host-ui" })
            hub.stop()
            oldSink.hostUiRequested(); currentSink.hostUiRequested(); hub.hostUiRequested()
            assertEquals(2, recorder.events.count { it == "host-ui" })
            hub.start()
            val next = output("next")
            hub.addOutput(next)
            currentSink.hostUiRequested()
            assertFalse("host-ui" in next.events)
            input.emit { it.hostUiRequested() }
            assertEquals(1, next.events.count { it == "host-ui" })
        } finally { hub.stop() }
    }

    @Test fun requestWaitsForOpenWithoutBlockingTheInput() {
        val input = FakeInput()
        val hub = ProjectionHub(input, clock = ManualClock())
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        val output = object : RecordingOutput("opening") {
            override fun open(host: OutputHost) {
                entered.countDown()
                assertTrue(release.await(5, TimeUnit.SECONDS))
                super.open(host)
            }
            override fun onHostUiRequested() { events.add("host-ui"); assertFalse(closed) }
        }
        hub.start()
        val opening = Thread { hub.addOutput(output) }
        opening.start()
        try {
            assertTrue(entered.await(1, TimeUnit.SECONDS))
            input.emit { it.hostUiRequested() }
            assertFalse("host-ui" in output.events)
        } finally {
            release.countDown()
            opening.join(1000)
            hub.stop()
        }
        assertFalse(opening.isAlive)
        assertEquals(1, output.events.count { it == "host-ui" })
    }

    @Test fun failedHostUiCallbackIsIsolatedAndCloseFollowsTheCallback() {
        val failures = mutableListOf<String>()
        val input = FakeInput()
        val hub = ProjectionHub(input, clock = ManualClock(), listener = object : HubListener {
            override fun outputFailed(outputId: String, failure: Throwable) { failures.add(outputId) }
        })
        val bad = object : RecordingOutput("bad") {
            override fun onHostUiRequested() { error("host UI failed") }
        }
        val good = object : RecordingOutput("good") {
            override fun onHostUiRequested() {
                val read = Thread { hub.sessionConfig }
                read.start(); read.join(1000)
                assertFalse(read.isAlive, "Hub lock must not cover callbacks")
                hub.removeOutput(id)
                assertFalse(closed)
                events.add("host-ui")
            }
        }
        hub.addOutput(bad); hub.addOutput(good); hub.start()
        try {
            input.emit { it.hostUiRequested() }
            assertEquals(listOf("bad"), failures)
            assertTrue(bad.closed)
            assertTrue(good.closed)
            assertEquals(1, good.events.count { it == "host-ui" })
        } finally { hub.stop() }
    }
}
