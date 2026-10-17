// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-15.

package org.openxplay.stream.clink

import java.nio.ByteBuffer
import java.nio.ByteOrder
import org.openxplay.stream.clink.protocol.*
import org.openxplay.stream.core.*
import kotlin.test.*

class VehicleCodecTest {
    private fun numbers(vararg values: Pair<Int, ULong>): ByteArray =
        Fields(values.associate { it.first to FieldValue.Varint(it.second) }).encode()
    @Test fun `unsigned ranges and negative int32 are retained without scaling`() {
        val gps = VehicleCodec.sensor(1, numbers(1 to Int.MIN_VALUE.toLong().toULong(), 2 to (-90L).toULong(),
            3 to 180uL, 6 to UInt.MAX_VALUE.toULong(), 10 to ULong.MAX_VALUE)) as VehicleData.Gps
        assertEquals(Int.MIN_VALUE, gps.height); assertEquals(-90, gps.latitude); assertEquals(180, gps.longitude)
        assertEquals(UInt.MAX_VALUE, gps.speed); assertEquals(ULong.MAX_VALUE, gps.timestamp)
        assertFailsWith<IllegalArgumentException> { VehicleCodec.sensor(5, numbers(1 to 0x1_0000_0000uL)) }
        assertFailsWith<IllegalArgumentException> { VehicleCodec.sensor(1, numbers(1 to 0x1_0000_0000uL)) }
    }
    @Test fun `enums use documented values and map unfamiliar values to unknown`() {
        for ((raw, expected) in listOf(0 to GearPosition.UNKNOWN, 1 to GearPosition.P, 2 to GearPosition.D,
            3 to GearPosition.R, 4 to GearPosition.N, 99 to GearPosition.UNKNOWN)) {
            assertEquals(expected, (VehicleCodec.sensor(6, numbers(1 to raw.toULong())) as VehicleData.Gear).gear)
        }
        for ((raw, expected) in listOf(0 to GyroscopeType.UNKNOWN, 1 to GyroscopeType.SINGLE,
            2 to GyroscopeType.THREE_AXIS, 99 to GyroscopeType.UNKNOWN)) {
            assertEquals(expected, (VehicleCodec.sensor(3, numbers(1 to raw.toULong())) as VehicleData.Gyroscope).type)
        }
        for ((raw, expected) in listOf(0 to DayNightMode.UNKNOWN, 1 to DayNightMode.DAY, 2 to DayNightMode.NIGHT, 99 to DayNightMode.UNKNOWN)) {
            assertEquals(expected, VehicleCodec.dayNight(numbers(1 to raw.toULong())).mode)
        }
    }
    @Test fun `fixed64 doubles are little endian and malformed fields are rejected`() {
        val bits = ByteBuffer.allocate(8).order(ByteOrder.LITTLE_ENDIAN).putDouble(-123.25).array()
        val report = VehicleCodec.sensor(7, Fields(mapOf(3 to FieldValue.Bytes(bits, 1))).encode())
        assertEquals(VehicleData.LightSensor(0.0, 0.0, -123.25), report)
        assertFailsWith<IllegalArgumentException> { VehicleCodec.sensor(7, Fields(mapOf(3 to FieldValue.Bytes(bits))).encode()) }
        for (bytes in listOf(byteArrayOf(0x80.toByte()), byteArrayOf(9, 0), byteArrayOf(8, 1, 8, 2))) {
            assertFailsWith<IllegalArgumentException> { VehicleCodec.sensor(4, bytes) }
        }
        assertNull(VehicleCodec.sensor(99, byteArrayOf(0x80.toByte())))
        assertEquals(VehicleData.Fuel(0u, 0u), VehicleCodec.sensor(5, byteArrayOf()))
    }
}
