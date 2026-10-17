// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

use oxp_core::*;
use serde_json::Value;
use std::{fs, path::Path};

#[test]
fn shared_topic_vectors() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/vectors/plugins/topics");
    for name in ["valid", "invalid"] {
        let cases: Vec<Value> =
            serde_json::from_slice(&fs::read(root.join(format!("{name}.json"))).unwrap()).unwrap();
        for case in cases {
            let result = ContextUpdate::from_json(&case);
            if name == "valid" {
                let update = result.unwrap_or_else(|e| panic!("{case}: {e}"));
                assert_eq!(update.to_json().unwrap(), case);
            } else {
                assert!(result.is_err(), "Accepted {case}");
            }
        }
    }
    assert_eq!(
        ContextUpdate::idle()
            .iter()
            .map(ContextUpdate::topic)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        ContextUpdate::idle().len()
    );
    let update = ContextUpdate::NowPlaying(Some(NowPlayingContext {
        title: None,
        artist: None,
        album: None,
        duration_ms: None,
        elapsed_ms: None,
        playing: None,
        rate: Some(f64::INFINITY),
        artwork: None,
    }));
    assert!(update.to_json().is_err());
}

#[test]
fn registry_relay_vectors() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/vectors/plugins");
    let vectors: Value =
        serde_json::from_slice(&fs::read(root.join("provider-relay.json")).unwrap()).unwrap();
    for case in vectors["vectors"].as_array().unwrap() {
        if !matches!(case["topic"].as_str(), Some("route" | "audioSource")) {
            continue;
        }
        let result = ContextUpdate::from_json(case);
        if case["valid"] == true {
            let update = result.unwrap();
            assert_eq!(
                ContextUpdate::from_json(&update.to_json().unwrap()).unwrap(),
                update
            );
        } else {
            assert!(result.is_err(), "Accepted {case}");
        }
    }
}
#[test]
fn route_encoding_rejects_nonfinite_numbers() {
    for n in [f64::NAN, f64::INFINITY, -1.0] {
        let mut r = RouteContext {
            active: true,
            destination: Some("Home".into()),
            eta_ms: Some(n),
            distance_remaining_m: Some(0.0),
            maneuver: None,
            app_name: Some("Maps".into()),
        };
        assert!(ContextUpdate::Route(Some(r.clone())).to_json().is_err());
        r.eta_ms = Some(0.0);
        r.maneuver = Some(RouteManeuver {
            r#type: Some(2),
            road: Some("Main St".into()),
            distance_m: Some(n),
        });
        assert!(ContextUpdate::Route(Some(r)).to_json().is_err());
    }
}

#[test]
fn new_topics_ignore_extension_fields() {
    for value in [
        serde_json::json!({"topic":"route","value":{"active":true,"destination":"Home","etaMs":1,"distanceRemainingM":2,"maneuver":{"type":2,"road":"Main St","distanceM":3,"extra":true},"appName":"Maps","durationMs":"extension"}}),
        serde_json::json!({"topic":"audioSource","value":{"appName":null,"durationMs":"extension"}}),
    ] {
        assert!(ContextUpdate::from_json(&value).is_ok());
    }
}
