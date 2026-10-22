// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.clink

import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.Timeout
import org.openxplay.stream.clink.trace.*
import org.openxplay.stream.clink.protocol.AuthIdentity
import org.openxplay.stream.core.*
import org.openxplay.stream.core.codec.Nal
import kotlin.test.*

@Timeout(15)
class ClinkLoopbackTest {
    @Test
    fun placeholderClipInputAndCleanReconnect() {
        val ports = freePorts()
        val trust = MemoryTrustStore()
        val peerIdentity = AuthIdentity.generate()
        val clip = BaselineClip()
        val input = ClipInput(clip)
        val errors = CopyOnWriteArrayList<Throwable>()
        val states = LinkedBlockingQueue<ClinkState>()
        val traces = CopyOnWriteArrayList<ClinkTraceEvent.Input>()
        val output = ClinkOutput(StaticClinkLink(ClinkAddresses("127.0.0.1", "127.0.0.1", ports.auth, "loopback")),
            PinProvider { "012345" }, trust, ports = ports, trace = ClinkTrace { if (it is ClinkTraceEvent.Input) traces += it }, listener = object : ClinkListener {
                override fun state(state: ClinkState) { states.add(state) }
                override fun error(error: Throwable) { errors += error }
            })
        val hub = ProjectionHub(input)
        var vehicle: SimulatedHeadUnit? = null
        try {
            val first = SimulatedHeadUnit(ports, identity = peerIdentity); vehicle = first
            hub.start(); hub.addOutput(output)
            first.await("placeholder") { first.playing && first.accessUnits.isNotEmpty() &&
                first.configurationAnswered && first.requestedUibcPort == ports.uibc && first.heartbeats > 0 }
            assertTrue(first.authenticated)
            assertContentEquals(first.peerKey, trust.loadPeerKey("loopback"))
            assertEquals(1280 to 720, first.accessUnits.first().size)
            assertTrue(first.accessUnits.first().keyFrame)
            val request = input.next<ControlEvent.KeyFrameRequest>()
            assertEquals(ScreenId.MAIN, request.screen)
            assertEquals(Origin("clink", "loopback"), request.origin)

            val before = first.accessUnits.size
            input.send(1) // A delta frame cannot replace the placeholder.
            first.await("placeholder continues after a delta frame") { first.accessUnits.size >= before + 2 }
            assertTrue(first.accessUnits.all { it.size == 1280 to 720 })
            input.send(0); input.send(1)
            first.await("live IDR") { first.accessUnits.any { it.size == 640 to 360 } }
            for (index in 2 until clip.frames.size) {
                input.send(index)
                first.await("clip AU ${index - 1}") { first.accessUnits.count { it.size == 640 to 360 } >= index }
            }
            // A following PES start completes the final zero-length video PES in the capture.
            input.send(0, clip.frames.size)
            first.await("all clip access units") { first.accessUnits.count { it.size == 640 to 360 } >= clip.frames.size }
            val received = first.accessUnits.filter { it.size == 640 to 360 }.take(clip.frames.size)
            assertEquals(60, received.size)
            assertTrue(received.first().keyFrame)
            received.zip(clip.frames).forEach { (actual, expected) ->
                val vcl = Nal.units(actual.data, Nal.Format.ANNEX_B).filter { it[0].toInt() and 31 in setOf(1, 5) }.toList()
                val source = Nal.units(expected).filter { it[0].toInt() and 31 in setOf(1, 5) }.toList()
                assertEquals(source.size, vcl.size)
                source.zip(vcl).forEach { (a, b) -> assertContentEquals(a, b) }
            }
            assertTrue(first.patCount >= 3 && first.pmtCount >= 3 && first.pcrCount > 0)
            assertTrue(first.accessUnits.zipWithNext().all { (a, b) -> b.pts90kHz > a.pts90kHz })

            input.events.clear()
            first.requestKeyframe()
            assertEquals(request.origin, input.next<ControlEvent.KeyFrameRequest>().origin)
            first.touch(0, listOf(PixelContact(41, 640, 360)))
            val center = input.next<ControlEvent.Touch>()
            assertEquals(request.origin, center.origin)
            assertEquals(ScreenId.MAIN, center.screen)
            assertEquals(1, center.contacts.size)
            assertEquals(0.5, center.contacts.single().x, 0.0001)
            assertEquals(0.5, center.contacts.single().y, 0.0001)
            assertTrue(center.contacts.single().pressed)
            val trace = traces.first { it.kind == "touch_down" }
            assertEquals(listOf(320L to 180L), trace.rawCoordinates)
            assertEquals(listOf(0.5 to 0.5), trace.coordinates)
            assertEquals(640 to 360, trace.streamSize)
            assertEquals(1280 to 720, trace.viewSize)
            first.touch(1, listOf(PixelContact(41, 640, 360)))
            assertFalse(input.next<ControlEvent.Touch>().contacts.single().pressed)

            val a = PixelContact(91, 320, 180); val b = PixelContact(12, 960, 540)
            first.touch(0, listOf(a))
            val down = input.next<ControlEvent.Touch>()
            first.touch(5 or (1 shl 8), listOf(a, b)) // ACTION_POINTER_DOWN, index 1.
            val two = input.next<ControlEvent.Touch>()
            assertEquals(listOf(TouchContact(0, 0.25, 0.25, true)), down.contacts)
            assertEquals(listOf(TouchContact(0, 0.25, 0.25, true), TouchContact(1, 0.75, 0.75, true)), two.contacts)
            first.touch(2, listOf(b.copy(x = 640), a.copy(x = 640))) // Wire order changes; slots stay stable.
            assertEquals(listOf(TouchContact(0, 0.5, 0.25, true), TouchContact(1, 0.5, 0.75, true)),
                input.next<ControlEvent.Touch>().contacts)
            first.touch(6 or (1 shl 8), listOf(b, a)) // Lift index 1, leaving track 12 pressed.
            assertEquals(listOf(TouchContact(0, 0.25, 0.25, false), TouchContact(1, 0.75, 0.75, true)),
                input.next<ControlEvent.Touch>().contacts)
            first.touch(1, listOf(b))
            assertEquals(listOf(TouchContact(1, 0.75, 0.75, false)), input.next<ControlEvent.Touch>().contacts)
            first.key(66, 0); first.key(66, 1)
            assertEquals(ControlEvent.Key(KeyCode.SELECT, true, request.origin), input.next<ControlEvent.Key>())
            assertEquals(ControlEvent.Key(KeyCode.SELECT, false, request.origin), input.next<ControlEvent.Key>())

            first.assertHealthy(); first.disconnect()
            awaitEnded(states)
            assertTrue(errors.isEmpty(), errors.joinToString())
            hub.removeOutput(output.id)
            first.close(); vehicle = null
            // Saved keys exercise quick auth and the same ClinkOutput's fresh lifecycle.
            val second = SimulatedHeadUnit(ports, identity = peerIdentity, pinnedPhone = first.phoneKey)
            vehicle = second
            hub.addOutput(output)
            second.await("reconnected placeholder") { second.authenticated && second.playing && second.accessUnits.isNotEmpty() }
            assertEquals(1280 to 720, second.accessUnits.first().size)
            assertTrue(second.accessUnits.first().keyFrame)
            second.disconnect(); awaitEnded(states)
            assertTrue(errors.isEmpty(), errors.joinToString())
        } finally {
            vehicle?.expectRelayClose()
            hub.stop()
            vehicle?.close()
        }
    }

    private fun awaitEnded(states: LinkedBlockingQueue<ClinkState>) {
        val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(3)
        while (true) {
            val remaining = deadline - System.nanoTime(); assertTrue(remaining > 0, "Session did not end")
            val state = states.poll(remaining, TimeUnit.NANOSECONDS) ?: fail("Session did not end")
            if (state == ClinkState.ENDED) return
        }
    }
}
