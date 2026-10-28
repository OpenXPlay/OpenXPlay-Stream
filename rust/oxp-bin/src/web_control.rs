// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

use oxp_web::{tls::TrustedPem, WebOutput};
use serde_json::{json, Value};

/// On --control-stdin (UTF-8 JSON lines, at most 4096 bytes excluding newline):
/// {"type":"web-tls","id":"revision","trusted":{"name":"relay.example","cert":"/cert.pem",
/// "key":"/key.pem"},"sans":["192.168.1.1","relay.local"]}. trusted:null removes trusted TLS.
/// All fields are required. Paths identify complete PEM files, not inline PEM. The startup
/// --tls-self-signed directory stays fixed. Reply type=web-tls echoes id, with applied:true and
/// SHA-256 fingerprint/trustedName/trustedFingerprint, or error. Advertise the name only after its
/// successful reply. Neither rejected requests nor diagnostics echo certificate/key contents.
pub fn handle(line: &[u8], web: Option<&WebOutput>) -> Option<Value> {
    let value: Value = serde_json::from_slice(line).ok()?;
    if value["type"] != "web-tls" {
        return None;
    }
    let id = value["id"].as_str().filter(|s| valid_text(s, 128));
    let error = |reason| json!({"type":"web-tls","id":id,"error":reason});
    let request = parse(&value);
    let (trusted, sans) = match request {
        Ok(request) if id.is_some() => request,
        _ => return Some(error("invalid_request")),
    };
    let Some(web) = web else {
        return Some(error("web_off"));
    };
    Some(match web.update_tls(trusted, sans) {
        Ok(mut reply) => {
            reply["type"] = json!("web-tls");
            reply["id"] = json!(id);
            reply["applied"] = json!(true);
            // Explicit nulls let the relay clear an earlier advertised trusted name.
            if reply.get("trustedName").is_none() {
                reply["trustedName"] = Value::Null;
                reply["trustedFingerprint"] = Value::Null;
            }
            reply
        }
        Err(_) => error("tls_update_failed"),
    })
}

fn valid_text(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

fn fields(value: &Value, names: &[&str]) -> bool {
    value.as_object().is_some_and(|object| {
        object.len() == names.len() && names.iter().all(|name| object.contains_key(*name))
    })
}

fn parse(value: &Value) -> Result<(Option<TrustedPem>, Vec<String>), ()> {
    if !fields(value, &["type", "id", "trusted", "sans"]) {
        return Err(());
    }
    let trusted = if value["trusted"].is_null() {
        None
    } else {
        let pem = &value["trusted"];
        if !fields(pem, &["name", "cert", "key"]) {
            return Err(());
        }
        let name = pem["name"]
            .as_str()
            .filter(|s| oxp_web::tls::valid_dns_name(s))
            .ok_or(())?;
        let path = |field: &str| {
            pem[field]
                .as_str()
                .filter(|s| valid_text(s, 1024))
                .map(Into::into)
                .ok_or(())
        };
        Some(TrustedPem {
            name: name.into(),
            cert: path("cert")?,
            key: path("key")?,
        })
    };
    let sans = value["sans"]
        .as_array()
        .filter(|names| names.len() <= 24)
        .ok_or(())?
        .iter()
        .map(|name| {
            let name = name.as_str().ok_or(())?;
            if name.parse::<std::net::IpAddr>().is_err() && !oxp_web::tls::valid_dns_name(name) {
                return Err(());
            }
            Ok(name.to_owned())
        })
        .collect::<Result<_, _>>()?;
    Ok((trusted, sans))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        io::{BufReader, Cursor},
    };

    #[test]
    fn stdin_mixes_tls_and_carlink_commands_and_acknowledges_the_served_set() {
        let root = std::env::temp_dir().join(format!("oxp-web-control-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let web = WebOutput::new(oxp_web::WebOutputConfig::new(
            "127.0.0.1:0".parse().unwrap(),
            root.clone(),
            oxp_web::Pairing::Disabled,
            oxp_web::Tls::SelfSigned {
                state_dir: root.clone(),
                sans: Vec::new(),
            },
        ))
        .unwrap();
        let before = fs::read(root.join("cert.pem")).unwrap();
        let input = concat!(
            "{\"type\":\"web-tls\",\"id\":\"r1\",\"trusted\":null,\"sans\":[\"192.0.2.21\"]}\n",
            "{\"type\":\"clink-capture\",\"action\":\"stop\"}\n",
            "{\"type\":\"web-tls\",\"id\":\"r2\",\"trusted\":null,\"sans\":[\"192.0.2.21\"]}"
        );
        let mut replies = Vec::new();
        crate::clink_control::read_control_with(
            BufReader::with_capacity(7, Cursor::new(input)),
            None,
            |line| handle(line, Some(&web)),
            |reply| replies.push(reply),
        );
        assert_eq!(replies.len(), 3);
        for (index, id) in [(0, "r1"), (2, "r2")] {
            assert_eq!(replies[index]["type"], "web-tls");
            assert_eq!(replies[index]["id"], id);
            assert_eq!(replies[index]["applied"], true);
            assert_eq!(replies[index]["algorithm"], "SHA-256");
            assert!(replies[index]["fingerprint"]
                .as_str()
                .is_some_and(|s| s.len() == 95));
            assert!(replies[index]["trustedName"].is_null());
            assert!(replies[index]["trustedFingerprint"].is_null());
        }
        assert_eq!(replies[0]["fingerprint"], replies[2]["fingerprint"]);
        assert_ne!(before, fs::read(root.join("cert.pem")).unwrap());
        assert_eq!(
            replies[1],
            json!({"type":"clink-trace","error":"trace_off"})
        );
        let failed = json!({"type":"web-tls","id":"bad","trusted":{"name":"relay.test","cert":root.join("missing"),"key":root.join("missing-key")},"sans":[]});
        assert_eq!(
            handle(failed.to_string().as_bytes(), Some(&web)).unwrap(),
            json!({"type":"web-tls","id":"bad","error":"tls_update_failed"})
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn validates_the_complete_replacement_without_echoing_material() {
        let base =
            json!({"type":"web-tls","id":"r1","trusted":null,"sans":["192.168.1.1","relay.local"]});
        assert_eq!(
            parse(&base),
            Ok((None, vec!["192.168.1.1".into(), "relay.local".into()]))
        );
        assert_eq!(
            handle(base.to_string().as_bytes(), None),
            Some(json!({"type":"web-tls","id":"r1","error":"web_off"}))
        );
        let pem = json!({"name":"relay.example","cert":"/tls/cert.pem","key":"/tls/key.pem"});
        let mut request = base.clone();
        request["trusted"] = pem.clone();
        assert!(parse(&request).unwrap().0.is_some());
        for (field, bad) in [
            ("sans", json!(null)),
            ("sans", json!(["bad/name"])),
            ("sans", json!(vec!["relay.local"; 25])),
            ("trusted", json!({})),
            ("trusted", json!({"name":"relay.example","cert":"/cert"})),
            (
                "trusted",
                json!({"name":"relay.example","cert":"secret\nPEM","key":"/key"}),
            ),
            ("id", json!(null)),
            ("id", json!("")),
            ("extra", json!(true)),
        ] {
            let mut request = base.clone();
            request[field] = bad;
            let reply = handle(request.to_string().as_bytes(), None).unwrap();
            assert_eq!(reply["error"], "invalid_request", "{field}");
            assert!(!reply.to_string().contains("secret"));
        }
        for field in ["trusted", "sans", "id"] {
            let mut request = base.clone();
            request.as_object_mut().unwrap().remove(field);
            assert_eq!(
                handle(request.to_string().as_bytes(), None).unwrap()["error"],
                "invalid_request"
            );
        }
        assert!(handle(b"not json", None).is_none());
        assert!(handle(br#"{"type":"clink-capture","action":"stop"}"#, None).is_none());
    }
}
