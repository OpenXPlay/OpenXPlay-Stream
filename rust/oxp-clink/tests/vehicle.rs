// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

use oxp_clink::{
    model::*,
    protocol::{vehicle, FieldValue, Fields},
};
use std::collections::BTreeMap;
fn numbers(values: &[(u32, u64)]) -> Vec<u8> {
    Fields(
        values
            .iter()
            .map(|(k, v)| (*k, FieldValue::Varint(*v)))
            .collect(),
    )
    .encode()
    .unwrap()
}
fn doubles(values: &[(u32, f64)], timestamp: Option<(u32, u64)>) -> Vec<u8> {
    let mut fields: BTreeMap<_, _> = values
        .iter()
        .map(|(k, v)| (*k, FieldValue::Fixed64(v.to_le_bytes())))
        .collect();
    if let Some((k, v)) = timestamp {
        fields.insert(k, FieldValue::Varint(v));
    }
    Fields(fields).encode().unwrap()
}
#[test]
fn all_sensor_methods_keep_raw_values_and_full_unsigned_ranges() {
    assert_eq!(
        vehicle::sensor(
            1,
            &numbers(&[
                (1, (-12i64) as u64),
                (2, 45),
                (3, (-123i64) as u64),
                (4, u32::MAX as u64),
                (5, 9),
                (6, 72),
                (7, 180),
                (8, 8),
                (9, 12),
                (10, u64::MAX)
            ])
        )
        .unwrap(),
        Some(VehicleData::Gps {
            height: -12,
            latitude: 45,
            longitude: -123,
            antenna_state: u32::MAX,
            pdop: 9,
            speed: 72,
            heading: 180,
            satellites_used: 8,
            satellites_visible: 12,
            timestamp: u64::MAX
        })
    );
    assert_eq!(
        vehicle::sensor(2, &numbers(&[(1, 1), (3, 2), (5, 1)])).unwrap(),
        Some(VehicleData::Lights {
            low_beam: true,
            high_beam: false,
            position_lamp: true,
            reverse_lamp: false,
            brake_lamp: true
        })
    );
    let mut gyro = Fields::decode(&doubles(
        &[(2, -12.5), (3, 3.25), (4, 270.5)],
        Some((5, u64::MAX)),
    ))
    .unwrap();
    gyro.0.insert(1, FieldValue::Varint(2));
    assert_eq!(
        vehicle::sensor(3, &gyro.encode().unwrap()).unwrap(),
        Some(VehicleData::Gyroscope {
            kind: GyroscopeType::ThreeAxis,
            x: -12.5,
            y: 3.25,
            z: 270.5,
            timestamp: u64::MAX
        })
    );
    assert_eq!(
        vehicle::sensor(
            4,
            &doubles(&[(1, -1.5), (2, 2.25), (3, 9.75)], Some((4, 123)))
        )
        .unwrap(),
        Some(VehicleData::Acceleration {
            x: -1.5,
            y: 2.25,
            z: 9.75,
            timestamp: 123
        })
    );
    assert_eq!(
        vehicle::sensor(5, &numbers(&[(1, 100), (2, 42)])).unwrap(),
        Some(VehicleData::Fuel {
            max: 100,
            current: 42
        })
    );
    assert_eq!(
        vehicle::sensor(6, &numbers(&[(1, 2), (2, 72)])).unwrap(),
        Some(VehicleData::Gear {
            gear: GearPosition::D,
            speed: 72
        })
    );
    assert_eq!(
        vehicle::sensor(7, &doubles(&[(1, 1000.5), (2, 0.25), (3, 50.75)], None)).unwrap(),
        Some(VehicleData::LightSensor {
            max_lux: 1000.5,
            min_lux: 0.25,
            current_lux: 50.75
        })
    );
}
#[test]
fn unknown_enums_defaults_and_invalid_wire_types_match_kotlin() {
    for (value, mode) in [
        (0, DayNightMode::Unknown),
        (1, DayNightMode::Day),
        (2, DayNightMode::Night),
        (99, DayNightMode::Unknown),
    ] {
        assert_eq!(
            vehicle::day_night(&numbers(&[(1, value)])).unwrap(),
            VehicleData::DayNight { mode }
        );
    }
    assert_eq!(
        vehicle::sensor(6, &[]).unwrap(),
        Some(VehicleData::Gear {
            gear: GearPosition::Unknown,
            speed: 0
        })
    );
    assert_eq!(vehicle::sensor(99, &[255]).unwrap(), None);
    for (method, body) in [
        (1, numbers(&[(1, 1 << 40)])),
        (5, numbers(&[(1, 1 << 32)])),
        (4, numbers(&[(1, 7)])),
        (4, vec![9, 0]),
    ] {
        assert!(vehicle::sensor(method, &body).is_err());
    }
}
