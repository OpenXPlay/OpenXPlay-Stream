// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use super::{json_object, Fields};
use crate::{require, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
/// Structural checks only, matching Kotlin; no manufacturer signature verification.
pub fn check(body: &[u8]) -> Result<()> {
    let f = Fields::decode(body)?;
    let encoded = f.bytes(1)?;
    require(
        encoded.len() <= 87384 && encoded.len() % 4 == 0,
        "invalid_certificate",
    )?;
    let raw = STANDARD.decode(encoded)?;
    require(
        STANDARD.encode(&raw).as_bytes() == encoded && (1..=65536).contains(&raw.len()),
        "invalid_certificate",
    )?;
    let obj = json_object(&raw)?;
    require(
        obj.keys().all(|k| {
            [
                "content",
                "signature",
                "signAlgorithm",
                "version",
                "cryptoConfigIndex",
                "salt",
            ]
            .contains(&k.as_str())
        }),
        "invalid_certificate",
    )?;
    require(
        obj.get("version").and_then(|v| v.as_i64()) == Some(1),
        "invalid_certificate",
    )?;
    require(
        text(&obj, "signAlgorithm", 32)? == "SHA256withRSA",
        "invalid_certificate",
    )?;
    text(&obj, "cryptoConfigIndex", 128)?;
    if obj.contains_key("salt") {
        text(&obj, "salt", 256)?;
    }
    let signed = |key: &str, limit: usize| -> Result<Vec<u8>> {
        let a = obj
            .get(key)
            .and_then(|v| v.as_array())
            .ok_or("invalid_certificate")?;
        require((1..=limit).contains(&a.len()), "invalid_certificate")?;
        a.iter()
            .map(|v| {
                let n = v.as_i64().ok_or("invalid_certificate")?;
                require((-128..=127).contains(&n), "invalid_certificate")?;
                Ok(n as u8)
            })
            .collect()
    };
    require(
        signed("signature", 1024)?.len() >= 256,
        "invalid_certificate",
    )?;
    let metadata = json_object(&signed("content", 32768)?)?;
    require(
        metadata.len() == 5
            && metadata.keys().all(|k| {
                [
                    "short_name",
                    "product_name",
                    "vendor_name",
                    "vendor_id",
                    "product_id",
                ]
                .contains(&k.as_str())
            }),
        "invalid_certificate",
    )?;
    for k in metadata.keys() {
        text(&metadata, k, if k.ends_with("_id") { 64 } else { 512 })?;
    }
    Ok(())
}
fn text<'a>(
    obj: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
    limit: usize,
) -> Result<&'a str> {
    let t = obj
        .get(key)
        .and_then(|v| v.as_str())
        .ok_or("invalid_certificate")?;
    require(
        !t.is_empty() && t.len() <= limit && !t.chars().any(|c| c.is_control() || c == '\u{fffd}'),
        "invalid_certificate",
    )?;
    Ok(t)
}
