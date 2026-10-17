// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use oxp_clink::protocol::*;
use p256::{
    ecdsa::{signature::Verifier, Signature, VerifyingKey},
    pkcs8::DecodePrivateKey,
    SecretKey,
};
use std::collections::BTreeMap;
fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}
fn vector(k: &str) -> Vec<u8> {
    let s = include_str!("../../../kotlin/clink/src/test/resources/vectors/auth.properties");
    hex(s
        .lines()
        .filter_map(|l| l.split_once('='))
        .find(|(key, _)| *key == k)
        .unwrap()
        .1)
}
fn client(peer: Option<Vec<u8>>) -> Auth {
    Auth::new(
        AuthIdentity::new(&vector("identity_private"), &vector("identity_public")).unwrap(),
        "phone".into(),
        "relay".into(),
        "192.168.49.108".into(),
        peer,
    )
    .unwrap()
}
fn start(a: &mut Auth, full: bool, confirmed: bool) -> Frame {
    a.start_with_material(
        123,
        7,
        Some("123456"),
        full,
        confirmed,
        SecretKey::from_pkcs8_der(&vector("agreement_private")).unwrap(),
        vector("phone_nonce").try_into().unwrap(),
    )
    .unwrap()
    .frames
    .remove(0)
}
fn response(body: Vec<u8>) -> Frame {
    Frame {
        sequence: 7,
        unix_seconds: 124,
        source: 0,
        format: 1,
        kind: 2,
        category: 3,
        method: 2,
        body,
    }
}
#[test]
fn full_request_matches_oracle_and_signature_transcript() {
    let request = start(&mut client(None), false, false);
    let actual = Fields::decode(&request.body).unwrap();
    let mut expected = Fields::decode(&vector("m1")).unwrap();
    let signature = actual.bytes(5).unwrap().to_vec();
    let mut compare = actual;
    compare.0.remove(&5);
    expected.0.remove(&5);
    assert_eq!(expected, compare);
    let verifying = VerifyingKey::from(decode_public(&vector("identity_public")).unwrap());
    verifying
        .verify(
            &[vector("agreement_public"), vector("phone_nonce")].concat(),
            &Signature::from_der(&signature).unwrap(),
        )
        .unwrap();
}
#[test]
fn full_response_versions_oracle_key_confirm_ipv4_and_complete() {
    for fixture in ["m2", "m2_0901"] {
        let mut auth = client(None);
        start(&mut auth, false, false);
        let out = auth.receive(&response(vector(fixture)), 125).unwrap();
        assert_eq!(vector("session_key"), out.session_key.unwrap());
        let confirm = &out.frames[0];
        assert_eq!((3, 0, 8), (confirm.method, confirm.kind, confirm.sequence));
        let fields = Fields::decode(&confirm.body).unwrap();
        assert_eq!(
            b"192.168.49.108",
            SecureRecord::new(&out.session_key.unwrap())
                .unwrap()
                .open(fields.bytes(1).unwrap())
                .unwrap()
                .as_slice()
        );
        assert!(out.events.contains(&AuthEvent::PeerVerified(
            Fields::decode(&vector("m2"))
                .unwrap()
                .bytes(2)
                .unwrap()
                .to_vec()
        )));
        assert!(auth.receive(&response(vector(fixture)), 126).is_err());
    }
}
#[test]
fn quick_reconnect_omits_identity_proof_and_confirms() {
    let peer = Fields::decode(&vector("m2"))
        .unwrap()
        .bytes(2)
        .unwrap()
        .to_vec();
    let mut auth = client(Some(peer));
    let request = start(&mut auth, false, true);
    let mut actual = Fields::decode(&request.body).unwrap();
    assert!(!actual.0.contains_key(&2));
    assert!(!actual.0.contains_key(&3));
    assert_eq!(1, actual.number(9).unwrap());
    let mut expected = Fields::decode(&vector("m1_quick")).unwrap();
    actual.0.remove(&5);
    expected.0.remove(&5);
    assert_eq!(expected, actual);
    let mut response_fields = Fields::decode(&vector("m2")).unwrap();
    response_fields.0.remove(&2);
    response_fields.0.remove(&3);
    assert_eq!(
        vector("session_key"),
        auth.receive(&response(response_fields.encode().unwrap()), 125)
            .unwrap()
            .session_key
            .unwrap()
    );
}
#[test]
fn result_codes_allow_fresh_explicit_retry() {
    for version in [1, 0x0901] {
        for (code, expected) in [
            (1, AuthResult::PinRequired),
            (2, AuthResult::ConfirmationRequired),
            (3, AuthResult::InternalError),
            (4, AuthResult::Rejected),
            (u64::MAX, AuthResult::Rejected),
        ] {
            let mut auth = client(None);
            start(&mut auth, false, false);
            let body = Fields(BTreeMap::from([
                (1, FieldValue::Varint(version)),
                (7, FieldValue::Varint(code)),
            ]))
            .encode()
            .unwrap();
            let out = auth.receive(&response(body), 125).unwrap();
            assert!(out.session_key.is_none());
            assert!(out.frames.is_empty());
            assert_eq!(vec![AuthEvent::Result(expected, code)], out.events);
            assert_eq!(1, start(&mut auth, true, true).method);
        }
    }
}
#[test]
fn bad_proof_signature_nonce_version_key_pin_and_header_fail_closed() {
    for (id, value) in [
        (3, FieldValue::Bytes(vec![0; 32])),
        (5, FieldValue::Bytes(vec![1])),
        (6, FieldValue::Bytes(vec![0; 31])),
        (1, FieldValue::Varint(2)),
        (4, FieldValue::Bytes(vec![0; 64])),
    ] {
        let mut body = Fields::decode(&vector("m2")).unwrap();
        body.0.insert(id, value);
        let mut auth = client(None);
        start(&mut auth, false, false);
        let out = auth
            .receive(&response(body.encode().unwrap()), 125)
            .unwrap();
        assert!(out.session_key.is_none());
        assert!(out.frames.is_empty());
        assert!(matches!(out.events[0], AuthEvent::Failed(_)));
        assert!(auth.start(126, 8, Some("123456"), false, false).is_err());
    }
    for (full, message) in [(true, "peer_key_changed"), (false, "auth_signature_failed")] {
        let mut auth = client(Some(vector("identity_public")));
        start(&mut auth, full, false);
        assert_eq!(
            vec![AuthEvent::Failed(message.into())],
            auth.receive(&response(vector("m2")), 125).unwrap().events
        );
    }
    let mut auth = client(None);
    start(&mut auth, false, false);
    let mut f = response(vector("m2"));
    f.sequence = 8;
    assert!(matches!(
        auth.receive(&f, 125).unwrap().events[0],
        AuthEvent::Failed(_)
    ));
    assert!(AuthIdentity::new(&vector("identity_private"), &vector("agreement_public")).is_err());
}
