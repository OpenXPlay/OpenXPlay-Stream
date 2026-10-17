// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use crate::{protocol::json_object, require, Result};
pub const ADVERTISEMENT_SERVICE: &str = "0000fcfb-0000-1000-8000-00805f9b34fb";
pub const IDENTITY_SERVICE: &str = "00000001-0000-1000-8000-00805f9b34fb";
pub const NAME_SERVICE: &str = "00000002-0000-1000-8000-00805f9b34fb";
pub const GATT_SERVICE: &str = "2abcc850-9935-4f8a-ba84-123456789100";
pub const PHONE_CHARACTERISTIC: &str = "2abcc850-9935-4f8a-ba84-123456789101";
pub const HEAD_UNIT_CHARACTERISTIC: &str = "2abcc850-9935-4f8a-ba84-123456789102";
pub const CCCD: &str = "00002902-0000-1000-8000-00805f9b34fb";
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Advertisement {
    pub major: u8,
    pub minor: u8,
    pub serial: u8,
    pub car_id: String,
    pub model_id: String,
    pub vendor_data: String,
    pub name: String,
}
#[derive(Clone, Debug)]
pub struct PhoneRequest {
    pub id: String,
    pub name: String,
    pub model: String,
    pub band: i32,
    pub mac: String,
    pub kind: i32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadUnitReply {
    pub name: String,
    pub ssid: String,
    pub psk: String,
    pub mac: String,
    pub freq: i32,
    pub port: u16,
    pub kind: i32,
}
pub fn advertisement(identity: &[u8], name: &[u8]) -> Result<Advertisement> {
    require(
        identity.len() == 15 || (identity.len() == 16 && identity[0] == 1 && identity[1] == 9),
        "invalid_advertisement",
    )?;
    require(name.len() <= 16, "invalid_advertisement")?;
    let text = std::str::from_utf8(name)?.trim_end_matches('\0');
    require(valid_text(text, 16, true), "invalid_advertisement")?;
    let hex = |a, b| identity[a..b].iter().map(|v| format!("{v:02x}")).collect();
    Ok(Advertisement {
        major: identity[0],
        minor: identity[1],
        serial: identity[2],
        car_id: hex(3, 9),
        model_id: hex(9, 13),
        vendor_data: hex(13, 15),
        name: text.into(),
    })
}
pub fn phone_request(p: &PhoneRequest) -> Result<Vec<u8>> {
    let kind = if p.kind == 0 { 1001 } else { p.kind };
    require(
        valid_text(&p.id, 128, false)
            && valid_text(&p.name, 128, false)
            && valid_text(&p.model, 128, false)
            && valid_mac(&p.mac)
            && (0..=1).contains(&p.band)
            && (1001..=1002).contains(&kind),
        "invalid_phone_request",
    )?;
    // Preserve Kotlin's insertion order, including type last.
    let out = format!(
        "{{\"id\":{},\"name\":{},\"model\":{},\"band\":{},\"mac\":{},\"type\":{}}}",
        serde_json::to_string(&p.id)?,
        serde_json::to_string(&p.name)?,
        serde_json::to_string(&p.model)?,
        p.band,
        serde_json::to_string(&p.mac.to_lowercase())?,
        kind
    )
    .into_bytes();
    require(out.len() <= 512, "invalid_phone_request")?;
    Ok(out)
}
pub fn head_unit_reply(bytes: &[u8]) -> Result<HeadUnitReply> {
    require(bytes.len() <= 4096, "network_bootstrap_limit")?;
    let obj = json_object(bytes)?;
    let string = |k| -> Result<String> {
        Ok(obj
            .get(k)
            .and_then(|v| v.as_str())
            .ok_or("invalid_network_bootstrap")?
            .into())
    };
    let integer = |k| -> Result<i32> {
        Ok(i32::try_from(
            obj.get(k)
                .and_then(|v| v.as_i64())
                .ok_or("invalid_network_bootstrap")?,
        )?)
    };
    let name = string("name")?;
    let ssid = string("ssid")?;
    let psk = string("psk")?;
    let mac = string("mac")?.to_lowercase();
    let freq = integer("freq")?;
    let port = integer("port")?;
    let kind = integer("type")?;
    require(
        valid_text(&name, 256, true)
            && valid_text(&ssid, 32, false)
            && valid_text(&psk, 64, false)
            && psk.len() >= 8
            && valid_mac(&mac)
            && (0..=7125).contains(&freq)
            && (0..=65535).contains(&port)
            && (1001..=1002).contains(&kind),
        "invalid_network_bootstrap",
    )?;
    Ok(HeadUnitReply {
        name,
        ssid,
        psk,
        mac,
        freq,
        port: port as u16,
        kind,
    })
}
fn valid_text(s: &str, max: usize, empty: bool) -> bool {
    (empty || !s.is_empty()) && s.len() <= max && !s.chars().any(char::is_control)
}
fn valid_mac(s: &str) -> bool {
    s.len() == 17
        && s.split(':').count() == 6
        && s.split(':')
            .all(|p| p.len() == 2 && p.bytes().all(|b| b.is_ascii_hexdigit()))
}
