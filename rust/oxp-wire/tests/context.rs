// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

use oxp_core::ContextUpdate;
use oxp_wire::*;
use serde_json::{json, Value};
use std::io::Cursor;

#[test]
fn legacy_v0_negotiation_in_both_directions() {
    let Message::Json(old_consumer) = Message::hello("consumer", "old", "default").unwrap() else {
        panic!()
    };
    let update = ContextUpdate::idle().into_iter().next().unwrap();
    assert!(Message::context_for_peer(&update, &old_consumer)
        .unwrap()
        .is_none());
    let Message::Json(new_consumer) =
        Message::hello("consumer", "new", &context_client("default")).unwrap()
    else {
        panic!()
    };
    // This is the legacy hello schema, including its exact allowed keys and version.
    assert_eq!(new_consumer.as_object().unwrap().len(), 5);
    assert_eq!(new_consumer["version"], 0);
    assert!(new_consumer["client"]
        .as_str()
        .is_some_and(|s| !s.is_empty()));
    assert!(Message::context_for_peer(&update, &new_consumer)
        .unwrap()
        .is_some());
    let Message::Json(old_producer) = Message::hello("producer", "old", "default").unwrap() else {
        panic!()
    };
    assert!(!supports_context(&old_producer));
    // An old producer can continue sending its existing messages to a new consumer.
    let mut bytes = Vec::new();
    let unknown = br#"{"type":"future-context","value":42}"#;
    bytes.extend_from_slice(&(unknown.len() as u32).to_le_bytes());
    bytes.extend_from_slice(unknown);
    write(&mut bytes, &Message::Json(json!({"type":"restart"}))).unwrap();
    let mut input = Cursor::new(bytes);
    assert!(matches!(read(&mut input).unwrap(), Some(Message::Json(v)) if v["type"] == "restart"));
}
#[test]
fn context_binding_uses_shared_topic_vectors() {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/vectors/plugins/topics");
    for name in ["valid", "invalid"] {
        let cases: Vec<Value> =
            serde_json::from_slice(&std::fs::read(root.join(format!("{name}.json"))).unwrap())
                .unwrap();
        for mut case in cases {
            case.as_object_mut()
                .unwrap()
                .insert("type".into(), json!("context"));
            if name == "valid" {
                let update = decode_context(&case).unwrap();
                assert_eq!(Message::context(&update).unwrap(), Message::Json(case));
            } else {
                assert!(Message::json(case).is_err());
            }
        }
    }
}
