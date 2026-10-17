// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-15.

package org.openxplay.stream.core

/**
 * Vehicle reports retain raw values. Units, scaling and timestamp epochs are undocumented;
 * no normalization or conversion is implied. Unsigned fields retain their full wire range.
 * These reports are not routed to the input. A future input could use GPS for CarPlay location.
 */
public sealed interface VehicleData {
    public data class Gps(
        public val height: Int, public val latitude: Int, public val longitude: Int,
        public val antennaState: UInt, public val pdop: UInt, public val speed: UInt,
        public val heading: UInt, public val satellitesUsed: UInt, public val satellitesVisible: UInt,
        public val timestamp: ULong,
    ) : VehicleData
    public data class Lights(
        public val lowBeam: Boolean, public val highBeam: Boolean, public val positionLamp: Boolean,
        public val reverseLamp: Boolean, public val brakeLamp: Boolean,
    ) : VehicleData
    public data class Gyroscope(
        public val type: GyroscopeType, public val x: Double, public val y: Double, public val z: Double,
        public val timestamp: ULong,
    ) : VehicleData
    public data class Acceleration(
        public val x: Double, public val y: Double, public val z: Double, public val timestamp: ULong,
    ) : VehicleData
    public data class Fuel(public val max: UInt, public val current: UInt) : VehicleData
    public data class Gear(public val gear: GearPosition, public val speed: UInt) : VehicleData
    public data class LightSensor(public val maxLux: Double, public val minLux: Double, public val currentLux: Double) : VehicleData
    public data class DayNight(public val mode: DayNightMode) : VehicleData
}

public enum class GearPosition { UNKNOWN, P, D, R, N }
public enum class GyroscopeType { UNKNOWN, SINGLE, THREE_AXIS }
public enum class DayNightMode { UNKNOWN, DAY, NIGHT }

/** Called asynchronously by the hub, independently of media and other vehicle listeners. */
public fun interface VehicleListener {
    public fun vehicle(origin: Origin, data: VehicleData)
}
