// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-15.

package org.openxplay.stream.clink.protocol

import java.nio.ByteBuffer
import java.nio.ByteOrder
import org.openxplay.stream.core.*

/** Decodes only the documented field layout; values are never scaled or unit-converted. */
internal object VehicleCodec {
    private fun Fields.uint(id: Int): UInt {
        val value = number(id)
        require(value <= UInt.MAX_VALUE.toULong()) { "invalid_uint32" }
        return value.toUInt()
    }
    private fun Fields.int(id: Int): Int {
        val value = number(id)
        // Protobuf int32 negatives may be sign-extended to ten-byte varints.
        require(value <= UInt.MAX_VALUE.toULong() || value >= Int.MIN_VALUE.toLong().toULong()) { "invalid_int32" }
        return value.toInt()
    }
    private fun Fields.bool(id: Int): Boolean = number(id) != 0uL
    private fun Fields.double(id: Int): Double {
        val value = get(id) ?: return 0.0
        require(value is FieldValue.Bytes && value.wireType == 1) { "invalid_double" }
        return ByteBuffer.wrap(value.value).order(ByteOrder.LITTLE_ENDIAN).double
    }
    fun sensor(method: Int, body: ByteArray): VehicleData? {
        if (method !in 1..7) return null
        val f = Fields.decode(body)
        return when (method) {
            1 -> VehicleData.Gps(f.int(1), f.int(2), f.int(3), f.uint(4), f.uint(5), f.uint(6),
                f.uint(7), f.uint(8), f.uint(9), f.number(10))
            2 -> VehicleData.Lights(f.bool(1), f.bool(2), f.bool(3), f.bool(4), f.bool(5))
            3 -> VehicleData.Gyroscope(when (f.int(1)) {
                1 -> GyroscopeType.SINGLE; 2 -> GyroscopeType.THREE_AXIS; else -> GyroscopeType.UNKNOWN
            }, f.double(2), f.double(3), f.double(4), f.number(5))
            4 -> VehicleData.Acceleration(f.double(1), f.double(2), f.double(3), f.number(4))
            5 -> VehicleData.Fuel(f.uint(1), f.uint(2))
            6 -> VehicleData.Gear(when (f.int(1)) {
                1 -> GearPosition.P; 2 -> GearPosition.D; 3 -> GearPosition.R; 4 -> GearPosition.N; else -> GearPosition.UNKNOWN
            }, f.uint(2))
            else -> VehicleData.LightSensor(f.double(1), f.double(2), f.double(3))
        }
    }
    fun dayNight(body: ByteArray): VehicleData.DayNight = VehicleData.DayNight(when (Fields.decode(body).int(1)) {
        1 -> DayNightMode.DAY; 2 -> DayNightMode.NIGHT; else -> DayNightMode.UNKNOWN
    })
}
