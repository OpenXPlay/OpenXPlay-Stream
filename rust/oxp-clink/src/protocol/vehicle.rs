// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

use super::{FieldValue, Fields};
use crate::{model::*, require, Result};

fn uint(f: &Fields, id: u32) -> Result<u32> {
    Ok(u32::try_from(f.number(id)?).map_err(|_| "invalid_uint32")?)
}
fn int(f: &Fields, id: u32) -> Result<i32> {
    let value = f.number(id)?;
    require(
        value <= u32::MAX as u64 || value >= i32::MIN as i64 as u64,
        "invalid_int32",
    )?;
    Ok(value as i32)
}
fn double(f: &Fields, id: u32) -> Result<f64> {
    match f.0.get(&id) {
        None => Ok(0.0),
        Some(FieldValue::Fixed64(value)) => Ok(f64::from_le_bytes(*value)),
        _ => Err("invalid_double".into()),
    }
}
pub fn sensor(method: u16, body: &[u8]) -> Result<Option<VehicleData>> {
    if !(1..=7).contains(&method) {
        return Ok(None);
    }
    let f = Fields::decode(body)?;
    Ok(Some(match method {
        1 => VehicleData::Gps {
            height: int(&f, 1)?,
            latitude: int(&f, 2)?,
            longitude: int(&f, 3)?,
            antenna_state: uint(&f, 4)?,
            pdop: uint(&f, 5)?,
            speed: uint(&f, 6)?,
            heading: uint(&f, 7)?,
            satellites_used: uint(&f, 8)?,
            satellites_visible: uint(&f, 9)?,
            timestamp: f.number(10)?,
        },
        2 => VehicleData::Lights {
            low_beam: f.number(1)? != 0,
            high_beam: f.number(2)? != 0,
            position_lamp: f.number(3)? != 0,
            reverse_lamp: f.number(4)? != 0,
            brake_lamp: f.number(5)? != 0,
        },
        3 => VehicleData::Gyroscope {
            kind: match int(&f, 1)? {
                1 => GyroscopeType::Single,
                2 => GyroscopeType::ThreeAxis,
                _ => GyroscopeType::Unknown,
            },
            x: double(&f, 2)?,
            y: double(&f, 3)?,
            z: double(&f, 4)?,
            timestamp: f.number(5)?,
        },
        4 => VehicleData::Acceleration {
            x: double(&f, 1)?,
            y: double(&f, 2)?,
            z: double(&f, 3)?,
            timestamp: f.number(4)?,
        },
        5 => VehicleData::Fuel {
            max: uint(&f, 1)?,
            current: uint(&f, 2)?,
        },
        6 => VehicleData::Gear {
            gear: match int(&f, 1)? {
                1 => GearPosition::P,
                2 => GearPosition::D,
                3 => GearPosition::R,
                4 => GearPosition::N,
                _ => GearPosition::Unknown,
            },
            speed: uint(&f, 2)?,
        },
        _ => VehicleData::LightSensor {
            max_lux: double(&f, 1)?,
            min_lux: double(&f, 2)?,
            current_lux: double(&f, 3)?,
        },
    }))
}
pub fn day_night(body: &[u8]) -> Result<VehicleData> {
    Ok(VehicleData::DayNight {
        mode: match int(&Fields::decode(body)?, 1)? {
            1 => DayNightMode::Day,
            2 => DayNightMode::Night,
            _ => DayNightMode::Unknown,
        },
    })
}
