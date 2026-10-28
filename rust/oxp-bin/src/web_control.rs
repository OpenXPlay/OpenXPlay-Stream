// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

use oxp_web::{
    tls::{DefaultPem, DefaultUpdate, TrustedPem},
    WebOutput,
};
use serde_json::{json, Value};

/// On --control-stdin (UTF-8 JSON lines, at most 4096 bytes excluding newline):
/// {"type":"web-tls","id":"revision","trusted":{"name":"relay.example","cert":"/cert.pem",
/// "key":"/key.pem"},"sans":["192.168.1.1","relay.local"]}. trusted:null removes trusted TLS.
/// Optional "default":{"cert":"/user-cert.pem","key":"/user-key.pem"} replaces IP/nonmatching
/// SNI TLS; null restores generated TLS; omission retains the installed default. Other fields are
/// required. Paths identify complete PEM files, not inline PEM. The startup
/// --tls-self-signed directory stays fixed. Reply type=web-tls echoes id, with applied:true and
/// SHA-256 fingerprint/trustedName/trustedFingerprint/defaultFingerprint, or error. The default
/// fingerprint is null without an imported default. Advertise the name only after its
/// successful reply. Neither rejected requests nor diagnostics echo certificate/key contents.
pub fn handle(line: &[u8], web: Option<&WebOutput>) -> Option<Value> {
    let value: Value = serde_json::from_slice(line).ok()?;
    if value["type"] != "web-tls" {
        return None;
    }
    let id = value["id"].as_str().filter(|s| valid_text(s, 128));
    let error = |reason| json!({"type":"web-tls","id":id,"error":reason});
    let request = parse(&value);
    let request = match request {
        Ok(request) if id.is_some() => request,
        _ => return Some(error("invalid_request")),
    };
    let Some(web) = web else {
        return Some(error("web_off"));
    };
    Some(
        match web.update_tls_with_default(request.trusted, request.sans, request.default) {
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
        },
    )
}

fn valid_text(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

fn fields(value: &Value, names: &[&str]) -> bool {
    value.as_object().is_some_and(|object| {
        object.len() == names.len() && names.iter().all(|name| object.contains_key(*name))
    })
}

#[derive(Debug, PartialEq, Eq)]
struct Request {
    trusted: Option<TrustedPem>,
    sans: Vec<String>,
    default: DefaultUpdate,
}

fn path(pem: &Value, field: &str) -> Result<std::path::PathBuf, ()> {
    pem[field]
        .as_str()
        .filter(|s| valid_text(s, 1024))
        .map(Into::into)
        .ok_or(())
}

fn parse(value: &Value) -> Result<Request, ()> {
    if !fields(value, &["type", "id", "trusted", "sans"])
        && !fields(value, &["type", "id", "trusted", "sans", "default"])
    {
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
        Some(TrustedPem {
            name: name.into(),
            cert: path(pem, "cert")?,
            key: path(pem, "key")?,
        })
    };
    let default = match value.get("default") {
        None => DefaultUpdate::Keep,
        Some(Value::Null) => DefaultUpdate::Remove,
        Some(pem) if fields(pem, &["cert", "key"]) => DefaultUpdate::Replace(DefaultPem {
            cert: path(pem, "cert")?,
            key: path(pem, "key")?,
        }),
        _ => return Err(()),
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
    Ok(Request {
        trusted,
        sans,
        default,
    })
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
            Ok(Request {
                trusted: None,
                sans: vec!["192.168.1.1".into(), "relay.local".into()],
                default: DefaultUpdate::Keep,
            })
        );
        assert_eq!(
            handle(base.to_string().as_bytes(), None),
            Some(json!({"type":"web-tls","id":"r1","error":"web_off"}))
        );
        let pem = json!({"name":"relay.example","cert":"/tls/cert.pem","key":"/tls/key.pem"});
        let mut request = base.clone();
        request["trusted"] = pem.clone();
        assert!(parse(&request).unwrap().trusted.is_some());
        request["default"] = json!({"cert":"/user/cert.pem","key":"/user/key.pem"});
        assert!(matches!(
            parse(&request).unwrap().default,
            DefaultUpdate::Replace(_)
        ));
        request["default"] = Value::Null;
        assert_eq!(parse(&request).unwrap().default, DefaultUpdate::Remove);
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
            ("default", json!(false)),
            ("default", json!({})),
            ("default", json!({"cert":"/cert"})),
            ("default", json!({"cert":"/cert","key":""})),
            ("default", json!({"cert":"secret\nPEM","key":"/key"})),
            (
                "default",
                json!({"cert":"/cert","key":"/key","name":"relay.test"}),
            ),
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

    #[test]
    fn default_control_acknowledges_install_keep_failure_and_removal() {
        let root = std::env::temp_dir().join(format!("oxp-default-control-{}", std::process::id()));
        let make = |state_dir| {
            WebOutput::new(oxp_web::WebOutputConfig::new(
                "127.0.0.1:0".parse().unwrap(),
                root.clone(),
                oxp_web::Pairing::Disabled,
                oxp_web::Tls::SelfSigned {
                    state_dir,
                    sans: vec![],
                },
            ))
            .unwrap()
        };
        let web = make(root.join("generated"));
        let _user = make(root.join("user"));
        let _other = make(root.join("other"));
        let mut request = json!({"type":"web-tls","id":"default-1","trusted":null,"sans":[],"default":{"cert":root.join("user/cert.pem"),"key":root.join("user/key.pem")}});
        let installed = handle(request.to_string().as_bytes(), Some(&web)).unwrap();
        assert_eq!(installed["applied"], true);
        assert_eq!(installed["defaultFingerprint"], installed["fingerprint"]);
        let fingerprint = installed["fingerprint"].as_str().unwrap();
        assert_eq!(fingerprint.split(':').count(), 32);
        assert!(fingerprint.split(':').all(|byte| byte.len() == 2
            && byte
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))));
        request.as_object_mut().unwrap().remove("default");
        assert_eq!(
            handle(request.to_string().as_bytes(), Some(&web)).unwrap(),
            installed
        );
        request["default"] =
            json!({"cert":root.join("user/cert.pem"),"key":root.join("other/key.pem")});
        assert_eq!(
            handle(request.to_string().as_bytes(), Some(&web)).unwrap(),
            json!({"type":"web-tls","id":"default-1","error":"tls_update_failed"})
        );
        request.as_object_mut().unwrap().remove("default");
        assert_eq!(
            handle(request.to_string().as_bytes(), Some(&web)).unwrap(),
            installed
        );
        request["default"] = Value::Null;
        let removed = handle(request.to_string().as_bytes(), Some(&web)).unwrap();
        assert_eq!(removed["applied"], true);
        assert_eq!(removed["defaultFingerprint"], Value::Null);
        assert_ne!(removed["fingerprint"], installed["fingerprint"]);
        fs::remove_dir_all(root).unwrap();
    }
}
