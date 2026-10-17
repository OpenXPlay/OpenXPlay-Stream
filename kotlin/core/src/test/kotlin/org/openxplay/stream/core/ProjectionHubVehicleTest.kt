// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-15.

package org.openxplay.stream.core

import java.time.Duration
import org.junit.jupiter.api.Assertions.assertTimeoutPreemptively
import java.util.concurrent.CountDownLatch
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import kotlin.test.*

class ProjectionHubVehicleTest {
    @Test fun `vehicle delivery carries origin and is isolated from media and other listeners`() {
        val input = FakeInput()
        val hub = ProjectionHub(input)
        val output = RecordingOutput()
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        val received = LinkedBlockingQueue<Pair<Origin, VehicleData>>()
        hub.addVehicleListener { _, _ -> entered.countDown(); release.await(3, TimeUnit.SECONDS) }
        hub.addVehicleListener { _, _ -> error("observer failed") }
        val registration = hub.addVehicleListener { origin, data -> received.add(origin to data) }
        try {
            hub.start(); hub.addOutput(output)
            val host = checkNotNull(output.host)
            val data = VehicleData.Fuel(UInt.MAX_VALUE, 42u)
            assertTimeoutPreemptively(Duration.ofSeconds(1)) { host.vehicle(data, Origin(output.id, "car")) }
            assertTrue(entered.await(3, TimeUnit.SECONDS))
            assertEquals(Origin(output.id, "car") to data, received.poll(3, TimeUnit.SECONDS))
            registration.close()
            assertTimeoutPreemptively(Duration.ofSeconds(1)) { repeat(100) { host.vehicle(data) } }
            val frame = VideoFrame(ScreenId.MAIN, SessionId(1), byteArrayOf(1), 0, true, 0)
            hub.videoFrame(frame)
            assertSame(frame, output.frames.single())
            assertFalse(output.closed)
            received.clear()
            host.vehicle(data, Origin("spoofed", "car"))
            assertNull(received.poll(100, TimeUnit.MILLISECONDS))
        } finally { release.countDown(); hub.stop() }
    }

    @Test fun `default origin belongs to the output and detached reports are ignored`() {
        val hub = ProjectionHub(FakeInput())
        val output = RecordingOutput()
        val received = LinkedBlockingQueue<Pair<Origin, VehicleData>>()
        hub.addVehicleListener { origin, data -> received.add(origin to data) }
        try {
            hub.start(); hub.addOutput(output)
            val host = checkNotNull(output.host)
            val data = VehicleData.DayNight(DayNightMode.DAY)
            host.vehicle(data)
            assertEquals(Origin(output.id, "") to data, received.poll(3, TimeUnit.SECONDS))
            host.vehicle(data, Origin("another-output", ""))
            assertNull(received.poll(100, TimeUnit.MILLISECONDS))
            hub.removeOutput(output.id)
            host.vehicle(data)
            assertNull(received.poll(100, TimeUnit.MILLISECONDS))
        } finally { hub.stop() }
    }
}
