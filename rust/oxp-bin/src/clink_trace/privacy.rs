// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

use serde_json::Value;
use std::sync::{Mutex, OnceLock};

fn secrets() -> &'static Mutex<Vec<String>> {
    static SECRETS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    SECRETS.get_or_init(Default::default)
}
pub fn remember_secret(secret: &str) {
    if !(4..=4096).contains(&secret.len()) {
        return;
    }
    let mut secrets = secrets().lock().unwrap();
    if !secrets.iter().any(|s| s == secret) {
        if secrets.len() == 256 {
            secrets.remove(0);
        }
        secrets.push(secret.into());
    }
}
fn private(key: &str) -> Option<&'static str> {
    if key.len() > 128 {
        return None;
    }
    let key: String = key
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect();
    let groups: &[(&[&str], &str)] = &[
        (
            &[
                "password",
                "passwd",
                "pwd",
                "passphrase",
                "psk",
                "presharedkey",
                "secret",
                "token",
                "apikey",
                "authorization",
                "cookie",
                "setcookie",
                "pin",
                "pincode",
                "passcode",
                "setupcode",
                "pairingcode",
            ],
            "[secret]",
        ),
        (
            &[
                "privatekey",
                "publickey",
                "pk8",
                "sessionkey",
                "readkey",
                "writekey",
                "shk",
                "ekey",
                "eiv",
                "pk",
                "salt",
                "proof",
                "signature",
                "certificate",
                "srp",
                "x25519",
            ],
            "[secret]",
        ),
        (
            &["latitude", "longitude", "lat", "lon", "lng"],
            "[location]",
        ),
        (
            &["devicename", "displayname", "contactname", "callername"],
            "[name]",
        ),
        (&["remoteid", "phonenumber"], "[phone]"),
        (&["email"], "[email]"),
    ];
    groups.iter().find_map(|(keys, marker)| {
        keys.iter()
            .any(|k| key == *k || key.ends_with(k))
            .then_some(*marker)
    })
}
/// Decode JSON and RTSP text before filtering. Escaped newlines cannot conceal credentials.
pub fn redact(value: &mut Value) {
    let sensor = value["type"] == "Frame" && value["service"] == "sensor";
    if value["type"] == "Frame" && matches!(value["service"].as_str(), Some("auth" | "cert")) {
        value["fields"] = serde_json::json!({});
    }
    if sensor {
        let gps = value["method"] == 1;
        if let Some(fields) = value["fields"].as_object_mut() {
            for (id, field) in fields {
                if let Some(preview) = field.get_mut("preview") {
                    *preview = "[hidden]".into();
                }
                if gps && matches!(id.as_str(), "2" | "3") {
                    *field = "[location]".into();
                }
            }
        }
    }
    walk(value);
}
fn walk(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                if let Some(marker) = private(key) {
                    if let Some(secret) = value.as_str() {
                        if !secret.starts_with('[') {
                            remember_secret(secret);
                        }
                    }
                    *value = marker.into();
                } else {
                    walk(value);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                walk(value);
            }
        }
        Value::String(text) => *text = clean_text(text),
        _ => (),
    }
}
fn clean_text(text: &str) -> String {
    if text.contains("-----BEGIN ") && text.contains("PRIVATE KEY-----") {
        return "[secret]".into();
    }
    if let Ok(mut value @ (Value::Object(_) | Value::Array(_))) =
        serde_json::from_str::<Value>(text)
    {
        redact(&mut value);
        return value.to_string();
    }
    let mut text = assignments(text);
    // Credentials learned from structured fields and PIN responses also disappear in free text.
    let mut known = secrets().lock().unwrap().clone();
    known.sort_by_key(|s| std::cmp::Reverse(s.len()));
    for secret in known {
        text = outside_markers(&text, |part| part.replace(&secret, "[secret]"));
    }
    text.split_inclusive('\n').map(clean_line).collect()
}
fn outside_markers(text: &str, replace: impl Fn(&str) -> String) -> String {
    let mut out = String::new();
    let mut remaining = text;
    while let Some(start) = remaining.find('[') {
        let tail = &remaining[start..];
        let marker = [
            "[secret]",
            "[location]",
            "[hidden]",
            "[name]",
            "[phone]",
            "[email]",
        ]
        .into_iter()
        .find(|marker| tail.starts_with(marker));
        if let Some(marker) = marker {
            out.push_str(&replace(&remaining[..start]));
            out.push_str(marker);
            remaining = &tail[marker.len()..];
        } else {
            out.push_str(&replace(&remaining[..start + 1]));
            remaining = &tail[1..];
        }
    }
    out.push_str(&replace(remaining));
    out
}
fn assignments(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::new();
    let mut copied = 0;
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_alphabetic()
            || (i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_'))
        {
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && (b[i].is_ascii_alphanumeric() || b"_-".contains(&b[i])) {
            i += 1;
        }
        let Some(marker) = private(&text[start..i]) else {
            continue;
        };
        let mut at = i;
        while at < b.len()
            && (b[at].is_ascii_whitespace() && b[at] != b'\n' || b"\"'".contains(&b[at]))
        {
            at += 1;
        }
        if at >= b.len() || !b":=".contains(&b[at]) {
            continue;
        }
        at += 1;
        while at < b.len() && b[at].is_ascii_whitespace() && b[at] != b'\n' {
            at += 1;
        }
        if at == b.len() {
            break;
        }
        let key = text[start..i].to_ascii_lowercase();
        let end = if key.ends_with("cookie") || key.ends_with("authorization") {
            text[at..].find(['\r', '\n']).map_or(b.len(), |n| at + n)
        } else {
            value_end(text, at)
        };
        let raw = &text[at..end];
        if [
            "[secret]",
            "[location]",
            "[hidden]",
            "[name]",
            "[phone]",
            "[email]",
        ]
        .contains(&raw)
        {
            i = end;
            continue;
        }
        if marker == "[secret]" {
            remember_secret(raw.trim_matches(['\'', '"']));
        }
        out.push_str(&text[copied..at]);
        out.push_str(marker);
        copied = end;
        i = end;
    }
    out.push_str(&text[copied..]);
    out
}
fn value_end(text: &str, start: usize) -> usize {
    let b = text.as_bytes();
    let quote = b[start];
    if b"[{".contains(&quote) {
        let mut depth = 0;
        let mut string = None;
        let mut escaped = false;
        for (i, &c) in b.iter().enumerate().skip(start) {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if string == Some(c) {
                string = None;
            } else if string.is_none() {
                if b"\"'".contains(&c) {
                    string = Some(c);
                } else if b"[{".contains(&c) {
                    depth += 1;
                } else if b"]}".contains(&c) {
                    depth -= 1;
                    if depth == 0 {
                        return i + 1;
                    }
                } else if b"\r\n".contains(&c) {
                    return i;
                }
            }
        }
        return b.len();
    }
    if b"\"'".contains(&quote) {
        let mut escaped = false;
        for (i, &c) in b.iter().enumerate().skip(start + 1) {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == quote {
                return i + 1;
            } else if c == b'\n' || c == b'\r' {
                return i;
            }
        }
        return b.len();
    }
    // RTSP authorization/cookie values can contain spaces; consume through the line boundary.
    let rest = &text[start..];
    if rest.starts_with("bytes=") {
        return rest.find(']').map_or(b.len(), |n| start + n + 1);
    }
    if rest.starts_with("Bearer ") || rest.starts_with("Basic ") {
        return rest.find(['\n', '\r']).map_or(b.len(), |n| start + n);
    }
    rest.find(|c: char| c.is_whitespace() || ",;&)}]".contains(c))
        .map_or(b.len(), |n| start + n)
}
fn clean_line(line: &str) -> String {
    let mut out = String::new();
    let mut authorization = false;
    for word in line.split_inclusive(char::is_whitespace) {
        let bare = word.trim_end_matches(char::is_whitespace);
        let suffix = &word[bare.len()..];
        let value = if authorization {
            authorization = false;
            "[secret]".into()
        } else if matches!(bare, "Bearer" | "Basic") {
            authorization = true;
            bare.into()
        } else if bare.starts_with('$') && bare.contains(',') {
            "[location]".into()
        } else if bare.contains('@') && !bare.contains("://") {
            "[email]".into()
        } else if bare.starts_with('+') && bare.bytes().filter(u8::is_ascii_digit).count() >= 7 {
            "[phone]".into()
        } else if let Some((scheme, address)) = bare.split_once("://") {
            if let Some((_, host)) = address.rsplit_once('@') {
                format!("{scheme}://[secret]@{host}")
            } else {
                bare.into()
            }
        } else {
            bare.into()
        };
        out.push_str(&value);
        out.push_str(suffix);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn unstructured_collections_material_and_contacts_are_private() {
        let mut value = json!({"message":"password=[alpha,beta]; sessionKey=bytes=4 [aabbccdd]\ncookie: a=one; b=two\nBearer contact-secret\nrtsp://user:password@example.invalid/x person@example.invalid $GPGGA,123,45.123,N\n"});
        redact(&mut value);
        let text = value.to_string();
        for secret in [
            "alpha",
            "beta",
            "aabbccdd",
            "a=one",
            "b=two",
            "contact-secret",
            "user:password",
            "person@example.invalid",
            "45.123",
        ] {
            assert!(!text.contains(secret), "{text}");
        }
        let once = value.clone();
        redact(&mut value);
        assert_eq!(value, once);
    }
    #[test]
    fn decoded_rtsp_and_nested_json_cannot_hide_credentials() {
        let mut value = json!({"type":"Rtsp","body":"wpa_psk: secret-pass\r\nAuthorization: Bearer abc def\r\nlatitude: 123\r\nwfd_video_formats: keep\r\n",
            "headers":{"Set-Cookie":"session=abc; secure","X-Pin":"123456"},"fields":{"privateKey":"key-material"}});
        redact(&mut value);
        let text = value.to_string();
        for secret in ["secret-pass", "abc", "123456", "key-material"] {
            assert!(!text.contains(secret), "{text}");
        }
        assert!(text.contains("wfd_video_formats: keep"));
        assert!(text.contains("[location]"));
    }
    #[test]
    fn authentication_fields_sensor_previews_and_locations_are_hidden() {
        for service in ["auth", "cert", "sensor"] {
            let mut value = json!({"type":"Frame","service":service,"method":1,"fields":{"1":{"length":4,"preview":"hide-me"},"2":123,"3":456}});
            redact(&mut value);
            assert!(!value.to_string().contains("hide-me"));
            if service == "sensor" {
                assert_eq!(value["fields"]["2"], "[location]");
            }
        }
    }
}
