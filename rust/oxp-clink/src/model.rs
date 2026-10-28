// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

pub use oxp_core::model::*;

/// Vehicle values retain the wire range. Units, scaling and timestamp epochs are unknown.
#[derive(Clone, Debug, PartialEq)]
pub enum VehicleData {
    Gps {
        height: i32,
        latitude: i32,
        longitude: i32,
        antenna_state: u32,
        pdop: u32,
        speed: u32,
        heading: u32,
        satellites_used: u32,
        satellites_visible: u32,
        timestamp: u64,
    },
    Lights {
        low_beam: bool,
        high_beam: bool,
        position_lamp: bool,
        reverse_lamp: bool,
        brake_lamp: bool,
    },
    Gyroscope {
        kind: GyroscopeType,
        x: f64,
        y: f64,
        z: f64,
        timestamp: u64,
    },
    Acceleration {
        x: f64,
        y: f64,
        z: f64,
        timestamp: u64,
    },
    Fuel {
        max: u32,
        current: u32,
    },
    Gear {
        gear: GearPosition,
        speed: u32,
    },
    LightSensor {
        max_lux: f64,
        min_lux: f64,
        current_lux: f64,
    },
    DayNight {
        mode: DayNightMode,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GyroscopeType {
    Unknown,
    Single,
    ThreeAxis,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GearPosition {
    Unknown,
    P,
    D,
    R,
    N,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DayNightMode {
    Unknown,
    Day,
    Night,
}
