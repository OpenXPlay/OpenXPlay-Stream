// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

pub mod auth;
pub mod certificate;
pub mod control;
pub mod fields;
pub mod frame;
pub mod secure_record;
pub use auth::*;
pub use fields::{FieldValue, Fields};
pub use frame::{Frame, FrameReader};
pub use secure_record::SecureRecord;
pub(crate) fn json_object(
    bytes: &[u8],
) -> crate::Result<serde_json::Map<String, serde_json::Value>> {
    let text = std::str::from_utf8(bytes)?;
    let mut stack: Vec<Option<std::collections::BTreeSet<String>>> = Vec::new();
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'{' => stack.push(Some(Default::default())),
            b'[' => stack.push(None),
            b'}' | b']' => {
                crate::require(stack.pop().is_some(), "invalid_json_object")?;
            }
            b'"' => {
                let start = i;
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                crate::require(i < b.len(), "invalid_json_object")?;
                let mut next = i + 1;
                while next < b.len() && (b[next] as char).is_whitespace() {
                    next += 1;
                }
                if next < b.len() && b[next] == b':' {
                    let key: String = serde_json::from_str(&text[start..=i])?;
                    let keys = stack
                        .last_mut()
                        .and_then(Option::as_mut)
                        .ok_or("invalid_json_key")?;
                    crate::require(keys.insert(key), "duplicate_json_key")?;
                }
            }
            _ => {}
        }
        crate::require(stack.len() <= 16, "network_bootstrap_limit")?;
        i += 1;
    }
    let v: serde_json::Value = serde_json::from_str(text)?;
    v.as_object()
        .cloned()
        .ok_or_else(|| "invalid_json_object".into())
}
