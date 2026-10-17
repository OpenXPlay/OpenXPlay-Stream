// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-15.

package org.openxplay.stream.clink

import java.net.InetSocketAddress
import java.net.Socket
import java.nio.ByteBuffer
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.Timeout
import org.openxplay.stream.clink.protocol.*
import org.openxplay.stream.core.*
import kotlin.test.*

@Timeout(15)
class ClinkVehicleLoopbackTest {
    private class Loop(enabled: Boolean, samples: Boolean = true) : AutoCloseable {
        val ports = freePorts()
        val vehicle = SimulatedHeadUnit(ports, sensorAttempts = 3, sensorRetryMillis = 10, sendVehicleSamples = samples)
        val reports = LinkedBlockingQueue<VehicleData>()
        val states = CopyOnWriteArrayList<Pair<Boolean, Long>>()
        val errors = CopyOnWriteArrayList<Throwable>()
        val ended = CountDownLatch(1)
        val output = ClinkOutput(StaticClinkLink(ClinkAddresses("127.0.0.1", "127.0.0.1", ports.auth, "vehicle-loop")),
            PinProvider { "012345" }, MemoryTrustStore(), ports = ports, vehicleData = enabled,
            listener = object : ClinkListener {
                override fun vehicleState(sensorActive: Boolean, malformedFrames: Long) { states += sensorActive to malformedFrames }
                override fun error(error: Throwable) { errors += error }
                override fun state(state: ClinkState) { if (state == ClinkState.ENDED) ended.countDown() }
            })
        init {
            output.open(object : OutputHost {
                override fun capabilities(capabilities: OutputCapabilities) = Unit
                override fun control(event: ControlEvent) = Unit
                override fun stats(stats: OutputStats) = Unit
                override fun vehicle(data: VehicleData) { reports.add(data) }
            })
            vehicle.await("projection ready") { vehicle.playing && vehicle.accessUnits.isNotEmpty() && vehicle.sensorFinishedDialing }
        }
        fun next(): VehicleData = checkNotNull(reports.poll(3, TimeUnit.SECONDS)) { "Missing vehicle report" }
        override fun close() {
            vehicle.expectRelayClose(); output.close()
            assertTrue(ended.await(3, TimeUnit.SECONDS))
            vehicle.close()
            assertTrue(errors.isEmpty(), errors.joinToString())
        }
    }

    @Test fun `all seven SENSOR methods and CONTROL day night reach the host`() = Loop(true).use { loop ->
        val reports = List(8) { loop.next() }.toSet()
        assertEquals(setOf(
            VehicleData.Gps(-12, 45, -123, 1u, 9u, 72u, 180u, 8u, 12u, 1_700_000_000_123uL),
            VehicleData.Lights(true, false, true, false, true),
            VehicleData.Gyroscope(GyroscopeType.THREE_AXIS, -12.5, 3.25, 270.5, 1_700_000_000_124uL),
            VehicleData.Acceleration(-1.5, 2.25, 9.75, 1_700_000_000_125uL),
            VehicleData.Fuel(100u, 42u), VehicleData.Gear(GearPosition.D, 72u),
            VehicleData.LightSensor(1000.5, 0.25, 50.75), VehicleData.DayNight(DayNightMode.NIGHT),
        ), reports)
        loop.vehicle.await("SENSOR SEND_SYNC acknowledgement") { loop.vehicle.sensorAcknowledgements.isNotEmpty() }
        assertContains(loop.states, true to 0L)
        loop.vehicle.dayNight(1)
        assertEquals(VehicleData.DayNight(DayNightMode.DAY), loop.next())
    }

    @Test fun `disabled collection leaves the SENSOR port closed and suppresses day night`() = Loop(false).use { loop ->
        assertFalse(loop.vehicle.sensorConnected)
        Socket().use { socket ->
            assertFailsWith<java.io.IOException> { socket.connect(InetSocketAddress("127.0.0.1", loop.ports.sensor), 200) }
        }
        loop.vehicle.dayNight(2, kind = 3)
        assertNull(loop.reports.poll(200, TimeUnit.MILLISECONDS))
        assertTrue(loop.states.isEmpty())
    }

    @Test fun `malformed payload CRC and GCM records are counted while valid reports and media continue`() = Loop(true, false).use { loop ->
        loop.vehicle.sensorFrame(4, byteArrayOf(9, 0)) // Truncated fixed64.
        val corrupt = Frame(90, 0, 0, 1, 0, 2, 5, byteArrayOf()).encode().also { it[19] = (it[19].toInt() xor 1).toByte() }
        // A valid-length malformed frame and its successor share one write.
        val badGcm = Frame(91, 0, 0, 1, 0, 2, 5, byteArrayOf(0)).encode()
        loop.vehicle.sensorBytes(corrupt + badGcm)
        loop.vehicle.sensorFrame(5, Fields(mapOf(1 to FieldValue.Varint(10uL), 2 to FieldValue.Varint(7uL))).encode(), kind = 3)
        assertEquals(VehicleData.Fuel(10u, 7u), loop.next())
        loop.vehicle.await("ack after malformed reports") { loop.vehicle.sensorAcknowledgements.isNotEmpty() }
        assertTrue(loop.states.any { it.first && it.second == 3L })
        val before = loop.vehicle.accessUnits.size
        loop.vehicle.await("video survives malformed SENSOR") { loop.vehicle.accessUnits.size > before }
        loop.vehicle.closeSensor()
        loop.vehicle.await("sensor closed") { !loop.vehicle.sensorConnected }
        loop.vehicle.dayNight(1)
        assertEquals(VehicleData.DayNight(DayNightMode.DAY), loop.next())
        assertFalse(loop.ended.await(100, TimeUnit.MILLISECONDS))
    }

    @Test fun `Carlink vehicle reports reach hub listeners with the head unit origin`() {
        val ports = freePorts()
        val peer = SimulatedHeadUnit(ports, sensorAttempts = 3, sensorRetryMillis = 10)
        val hub = ProjectionHub(FakeInput())
        val received = LinkedBlockingQueue<Pair<Origin, VehicleData>>()
        val ended = CountDownLatch(1)
        val errors = CopyOnWriteArrayList<Throwable>()
        hub.addVehicleListener { origin, data -> received.add(origin to data) }
        val output = ClinkOutput(StaticClinkLink(ClinkAddresses("127.0.0.1", "127.0.0.1", ports.auth, "vehicle-hub")),
            PinProvider { "012345" }, MemoryTrustStore(), ports = ports, vehicleData = true,
            listener = object : ClinkListener {
                override fun error(error: Throwable) { errors += error }
                override fun state(state: ClinkState) { if (state == ClinkState.ENDED) ended.countDown() }
            })
        try {
            hub.start(); hub.addOutput(output)
            repeat(8) {
                val report = checkNotNull(received.poll(3, TimeUnit.SECONDS))
                assertEquals(Origin("clink", "vehicle-hub"), report.first)
            }
        } finally {
            peer.expectRelayClose(); hub.stop()
            assertTrue(ended.await(3, TimeUnit.SECONDS))
            peer.close()
        }
        assertTrue(errors.isEmpty(), errors.joinToString())
    }

    @Test fun `invalid frame length closes only SENSOR`() = Loop(true, false).use { loop ->
        loop.vehicle.sensorBytes(ByteBuffer.allocate(4).putInt(1).array())
        loop.vehicle.await("corrupt SENSOR closed") { !loop.vehicle.sensorConnected }
        loop.vehicle.await("closed SENSOR diagnostics") { loop.states.any { !it.first && it.second == 1L } }
        val before = loop.vehicle.accessUnits.size
        loop.vehicle.await("projection survives channel close") { loop.vehicle.accessUnits.size > before }
        assertFalse(loop.ended.await(100, TimeUnit.MILLISECONDS))
    }
}
