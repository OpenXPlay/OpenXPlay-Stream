// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core

import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class ProjectionHubMicrophoneTest {
    private val input = FakeInput()
    private val hub = ProjectionHub(input, clock = ManualClock())
    private val a = RecordingOutput("a")
    private val b = RecordingOutput("b")
    private val oa = Origin("a", "one")
    private val ob = Origin("b", "two")

    private fun start() { hub.start(); hub.addOutput(a); hub.addOutput(b) }
    private fun request(stream: Int = 0) { hub.microphone(stream, MicRequest(stream, AudioCodec.PCM_S16LE, 16000, 1, 20)) }
    private fun mic(output: RecordingOutput, origin: Origin, stream: Int = 0) {
        output.host!!.control(ControlEvent.MicPacket(stream, byteArrayOf(1, 2), 0, origin))
    }
    private fun touch(output: RecordingOutput, origin: Origin) {
        output.host!!.control(ControlEvent.Touch(ScreenId.MAIN, emptyList(), origin))
    }
    private fun origins(): List<Origin> = input.controls.filterIsInstance<ControlEvent.MicPacket>().map { it.origin }

    @Test
    fun firstSenderOwnsRequestAndOtherClientsAndUnrequestedStreamsAreDropped() {
        start()
        mic(a, oa)
        assertTrue(origins().isEmpty())
        request()
        mic(b, ob); mic(a, oa); mic(b, Origin("b", "other")); mic(b, ob)
        mic(b, ob, 1)
        mic(a, ob) // Forged output id.
        assertEquals(listOf(ob, ob), origins())
        hub.microphone(0, null)
        mic(b, ob)
        request()
        mic(a, oa); mic(b, ob)
        assertEquals(listOf(ob, ob, oa), origins())
        hub.stop()
    }

    @Test
    fun lastTouchTakesPriorityWhenItSendsButFallbackCanSendBeforeThen() {
        start()
        touch(a, oa)
        request()
        mic(b, ob); mic(b, ob)
        mic(a, oa); mic(b, ob); mic(a, oa)
        assertEquals(listOf(ob, ob, oa, oa), origins())
        // Mic arbitration does not affect the existing last-touch key rule.
        b.host!!.control(ControlEvent.Key(KeyCode.HOME, true, ob))
        assertTrue(input.controls.none { it is ControlEvent.Key })
        a.host!!.control(ControlEvent.Key(KeyCode.HOME, true, oa))
        assertEquals(1, input.controls.filterIsInstance<ControlEvent.Key>().size)
        hub.stop()
    }

    @Test
    fun outputRemovalReleasesOwnershipWithoutEndingOtherRequests() {
        start(); request(); request(1)
        mic(a, oa); mic(b, ob, 1)
        val stale = a.host!!
        hub.removeOutput("a")
        mic(b, ob)
        stale.control(ControlEvent.MicPacket(0, byteArrayOf(1), 0, oa))
        val replacement = RecordingOutput("a")
        hub.addOutput(replacement)
        mic(replacement, oa); mic(replacement, oa, 1); mic(b, ob, 1)
        assertEquals(listOf(oa, ob, ob, ob), origins())
        hub.stop()
    }

    @Test
    fun removingLastTouchOwnerLetsAnotherOutputClaimTheActiveRequest() {
        start(); touch(a, oa); request()
        mic(a, oa); mic(b, ob)
        hub.removeOutput("a")
        mic(b, ob)
        assertEquals(listOf(oa, ob), origins())
        hub.stop()
    }

    @Test
    fun requestReplacementNewSessionAndStopResetOwnership() {
        start(); request()
        mic(a, oa)
        request()
        mic(b, ob)
        hub.session(SessionState(SessionId(2), SessionPhase.ACTIVE))
        mic(b, ob)
        request()
        mic(a, oa)
        assertEquals(listOf(oa, ob, oa), origins())
        val stale = a.host!!
        hub.stop()
        stale.control(ControlEvent.MicPacket(0, byteArrayOf(1), 0, oa))
        hub.start()
        val replacement = RecordingOutput("b")
        hub.addOutput(replacement)
        mic(replacement, ob)
        request(); mic(replacement, ob)
        assertEquals(listOf(oa, ob, oa, ob), origins())
        hub.stop()
    }

    @Test
    fun queuedPacketsAreFencedWhenRequestsEndOrOwningOutputsClose() {
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        val backing = FakeInput()
        val blocked = object : ProjectionInput by backing {
            override fun control(event: ControlEvent): Boolean {
                if (event is ControlEvent.Key) {
                    entered.countDown()
                    check(release.await(5, TimeUnit.SECONDS))
                }
                return backing.control(event)
            }
        }
        val hub = ProjectionHub(blocked, clock = ManualClock())
        val output = RecordingOutput("a")
        hub.start(); hub.addOutput(output)
        val origin = Origin("a", "one")
        val request = MicRequest(0, AudioCodec.PCM_S16LE, 16000, 1, 20)
        hub.microphone(0, request)
        val thread = Thread { output.host!!.control(ControlEvent.Key(KeyCode.HOME, true, origin)) }
        thread.start()
        try {
            assertTrue(entered.await(5, TimeUnit.SECONDS))
            output.host!!.control(ControlEvent.MicPacket(0, byteArrayOf(1), 0, origin))
            hub.microphone(0, null)
            hub.microphone(0, request) // Even reusing the same request instance starts a new arbitration run.
            output.host!!.control(ControlEvent.MicPacket(0, byteArrayOf(2), 0, origin))
            hub.removeOutput("a")
        } finally {
            release.countDown()
            thread.join(5000)
            hub.stop()
        }
        assertTrue(backing.controls.none { it is ControlEvent.MicPacket })
    }
}
