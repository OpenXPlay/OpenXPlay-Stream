// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-21

use super::*;
use rustls::{ClientConfig, ClientConnection, RootCertStore, ServerConnection};
use rustls_pki_types::ServerName;
use std::{
    io::Cursor,
    net::IpAddr,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(1);
pub(super) struct Temp(pub(super) PathBuf);
impl Temp {
    pub(super) fn new() -> Self {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../target/oxp-tls-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(super) fn names(values: &[&str]) -> Vec<String> {
    values.iter().map(|name| (*name).into()).collect()
}

fn parsed_names(cert: &CertificateDer<'_>) -> Vec<String> {
    sans::parse(cert.as_ref())
        .unwrap()
        .iter()
        .map(ToString::to_string)
        .collect()
}

pub(super) fn sni_identity(temp: &Temp) -> (Tls, CertificateDer<'static>, CertificateDer<'static>) {
    let trusted = rcgen::generate_simple_self_signed(names(&["relay.test"])).unwrap();
    let cert = temp.0.join("trusted.pem");
    let key = temp.0.join("trusted-key.pem");
    fs::write(&cert, trusted.cert.pem()).unwrap();
    fs::write(&key, trusted.key_pair.serialize_pem()).unwrap();
    let state_dir = temp.0.join("local");
    let mut required = names(&[
        "localhost",
        "127.0.0.1",
        "relay.test",
        "other.test",
        "relay.local",
    ]);
    required.extend(local_ipv4());
    self_signed_paths(&state_dir, &required).unwrap();
    let local = CertificateDer::from_pem_file(state_dir.join("cert.pem")).unwrap();
    (
        Tls::Sni {
            name: "ReLaY.TeSt.".into(),
            cert,
            key,
            state_dir,
            sans: required,
        },
        trusted.cert.der().clone(),
        local,
    )
}

pub(super) fn handshake(
    server: Arc<ServerConfig>,
    roots: &[CertificateDer<'static>],
    name: &str,
    sni: bool,
    version: &'static rustls::SupportedProtocolVersion,
) -> Vec<CertificateDer<'static>> {
    let mut store = RootCertStore::empty();
    for cert in roots {
        store.add(cert.clone()).unwrap();
    }
    let mut config = ClientConfig::builder_with_provider(Arc::new(ring::default_provider()))
        .with_protocol_versions(&[version])
        .unwrap()
        .with_root_certificates(store)
        .with_no_client_auth();
    config.enable_sni = sni;
    config.alpn_protocols = vec![b"http/1.1".to_vec(), b"h2".to_vec()];
    let mut client = ClientConnection::new(
        Arc::new(config),
        ServerName::try_from(name.to_owned()).unwrap(),
    )
    .unwrap();
    let mut server = ServerConnection::new(server).unwrap();
    for _ in 0..16 {
        let mut bytes = Vec::new();
        client.write_tls(&mut bytes).unwrap();
        if !bytes.is_empty() {
            server.read_tls(&mut Cursor::new(bytes)).unwrap();
            server.process_new_packets().unwrap();
        }
        let mut bytes = Vec::new();
        server.write_tls(&mut bytes).unwrap();
        if !bytes.is_empty() {
            client.read_tls(&mut Cursor::new(bytes)).unwrap();
            client.process_new_packets().unwrap();
        }
        if !client.is_handshaking() && !server.is_handshaking() {
            break;
        }
    }
    assert!(!client.is_handshaking());
    assert!(!server.is_handshaking());
    assert_eq!(client.protocol_version(), Some(version.version));
    assert_eq!(client.alpn_protocol(), None);
    client.peer_certificates().unwrap().to_vec()
}

#[test]
fn resolver_matches_only_the_configured_sni_ignoring_case_and_one_trailing_dot() {
    let temp = Temp::new();
    let (tls, _, _) = sni_identity(&temp);
    let Tls::Sni {
        cert,
        key,
        state_dir,
        ..
    } = tls
    else {
        unreachable!()
    };
    let trusted = certified_key(&cert, &key).unwrap();
    let fallback = certified_key(&state_dir.join("cert.pem"), &state_dir.join("key.pem")).unwrap();
    let resolver = SniResolver {
        name: Some("relay.test".into()),
        trusted: Some(trusted.clone()),
        fallback: fallback.clone(),
    };
    for name in ["relay.test", "RELAY.TEST", "ReLaY.TeSt."] {
        assert!(Arc::ptr_eq(&resolver.select(Some(name)), &trusted));
    }
    for name in [
        None,
        Some("other.test"),
        Some("relay.local"),
        Some("127.0.0.1"),
        Some("relay.test.."),
        Some("sub.relay.test"),
        Some("relay.test.evil"),
    ] {
        assert!(Arc::ptr_eq(&resolver.select(name), &fallback));
    }
}

#[test]
fn sni_handshakes_present_the_selected_leaf_with_tls12_and_tls13() {
    let temp = Temp::new();
    let (tls, trusted, local) = sni_identity(&temp);
    let identity = load(&tls).unwrap().unwrap();
    assert_ne!(trusted, local);
    let roots = [trusted.clone(), local.clone()];
    for version in [&rustls::version::TLS12, &rustls::version::TLS13] {
        for (name, sni, expected) in [
            ("relay.test", true, &trusted),
            ("RELAY.TEST", true, &trusted),
            ("relay.test.", true, &trusted),
            ("127.0.0.1", true, &local),
            ("other.test", true, &local),
            ("relay.local", true, &local),
            ("relay.test", false, &local),
        ] {
            let chain = handshake(identity.config.clone(), &roots, name, sni, version);
            assert_eq!(&chain[0], expected, "{name}, SNI={sni}");
        }
    }
    let Tls::Sni {
        state_dir,
        cert,
        key,
        ..
    } = tls
    else {
        unreachable!()
    };
    let local_identity = load(&Tls::SelfSigned {
        state_dir,
        sans: Vec::new(),
    })
    .unwrap()
    .unwrap();
    let trusted_identity = load(&Tls::Pem { cert, key }).unwrap().unwrap();
    let event = identity.fingerprint_event();
    assert_eq!(event["type"], "tls-fingerprint");
    assert_eq!(event["algorithm"], "SHA-256");
    assert_eq!(event["fingerprint"], local_identity.fingerprint);
    assert_eq!(event["trustedName"], "relay.test");
    assert_eq!(event["trustedFingerprint"], trusted_identity.fingerprint);
    for identity in [local_identity, trusted_identity] {
        assert!(identity.fingerprint_event().get("trustedName").is_none());
        assert_eq!(
            identity.fingerprint_event()["trustedFingerprint"],
            Value::Null
        );
        assert_eq!(
            identity.fingerprint_event()["defaultFingerprint"],
            Value::Null
        );
    }
}

#[test]
fn bad_trusted_pem_falls_back_for_all_clients_but_pem_alone_still_fails() {
    let temp = Temp::new();
    let (tls, trusted, local) = sni_identity(&temp);
    let Tls::Sni {
        cert,
        key,
        state_dir,
        ..
    } = &tls
    else {
        unreachable!()
    };
    let valid_cert = fs::read(cert).unwrap();
    let valid_key = fs::read(key).unwrap();
    let wrong_key = rcgen::KeyPair::generate()
        .unwrap()
        .serialize_pem()
        .into_bytes();
    let local_bytes = fs::read(state_dir.join("cert.pem")).unwrap();
    let local_fingerprint = fingerprint(
        &certified_key(&state_dir.join("cert.pem"), &state_dir.join("key.pem")).unwrap(),
    );
    let invalid_der = b"-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n".to_vec();
    let mut invalid_chain = valid_cert.clone();
    invalid_chain.extend_from_slice(&invalid_der);
    for (cert_bytes, key_bytes) in [
        (None, Some(valid_key.clone())),
        (Some(valid_cert.clone()), None),
        (Some(Vec::new()), Some(valid_key.clone())),
        (Some(b"not a certificate".to_vec()), Some(valid_key.clone())),
        (Some(invalid_der), Some(valid_key.clone())),
        (Some(invalid_chain), Some(valid_key.clone())),
        (Some(valid_cert.clone()), Some(b"not a key".to_vec())),
        (Some(valid_cert.clone()), Some(wrong_key)),
        (
            Some(b"-----BEGIN CERTIFICATE-----\n!\n-----END CERTIFICATE-----".to_vec()),
            Some(valid_key),
        ),
    ] {
        for (path, bytes) in [(cert, cert_bytes), (key, key_bytes)] {
            if let Some(bytes) = bytes {
                fs::write(path, bytes).unwrap();
            } else {
                fs::remove_file(path).unwrap();
            }
        }
        let identity = load(&tls).unwrap().unwrap();
        assert_eq!(identity.fingerprint, local_fingerprint);
        assert!(identity.fingerprint_event()["trustedName"].is_null());
        assert!(identity.fingerprint_event()["trustedFingerprint"].is_null());
        assert_eq!(fs::read(state_dir.join("cert.pem")).unwrap(), local_bytes);
        for name in ["relay.test", "127.0.0.1"] {
            assert_eq!(
                handshake(
                    identity.config.clone(),
                    &[trusted.clone(), local.clone()],
                    name,
                    true,
                    &rustls::version::TLS13
                )[0],
                local,
            );
        }
        assert!(load(&Tls::Pem {
            cert: cert.clone(),
            key: key.clone()
        })
        .is_err());
    }
}

#[test]
fn a_covered_identity_keeps_certificate_and_key_bytes_and_timestamps() {
    let temp = Temp::new();
    let required = names(&[
        "localhost",
        "127.0.0.1",
        "relay.local",
        "192.168.1.1",
        "2001:db8::1",
    ]);
    let (cert, key) = self_signed_paths(&temp.0, &required).unwrap();
    let before = [fs::read(&cert).unwrap(), fs::read(&key).unwrap()];
    let modified = [
        fs::metadata(&cert).unwrap().modified().unwrap(),
        fs::metadata(&key).unwrap().modified().unwrap(),
    ];
    let mut equivalent = required;
    equivalent[2] = "RELAY.LOCAL.".into();
    equivalent[4] = "2001:0db8:0:0:0:0:0:1".into();
    self_signed_paths(&temp.0, &equivalent).unwrap();
    assert_eq!(before, [fs::read(&cert).unwrap(), fs::read(&key).unwrap()]);
    assert_eq!(
        modified,
        [
            fs::metadata(&cert).unwrap().modified().unwrap(),
            fs::metadata(&key).unwrap().modified().unwrap()
        ]
    );
    assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 2);
}

#[test]
fn new_hotspot_sans_regenerate_with_the_union_and_a_private_matching_key() {
    let temp = Temp::new();
    let old = names(&[
        "localhost",
        "127.0.0.1",
        "192.168.1.1",
        "old.local",
        "2001:db8::1",
    ]);
    let (cert, key) = self_signed_paths(&temp.0, &old).unwrap();
    let old_cert = fs::read(&cert).unwrap();
    let old_key = fs::read(&key).unwrap();
    fs::set_permissions(&key, fs::Permissions::from_mode(0o644)).unwrap();
    let required = names(&["localhost", "127.0.0.1", "192.168.55.1", "hotspot.local"]);
    self_signed_paths(&temp.0, &required).unwrap();
    assert_ne!(fs::read(&cert).unwrap(), old_cert);
    assert_ne!(fs::read(&key).unwrap(), old_key);
    let identity = certified_key(&cert, &key).unwrap();
    identity.keys_match().unwrap();
    let expected: Vec<_> = old
        .into_iter()
        .chain(names(&["192.168.55.1", "hotspot.local"]))
        .collect();
    assert_eq!(parsed_names(&identity.cert[0]), expected);
    assert_eq!(
        fs::metadata(key).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 2);
}

#[test]
fn dns_sans_containing_ip_text_do_not_count_as_ip_sans() {
    let temp = Temp::new();
    let mut params = rcgen::CertificateParams::new(Vec::new()).unwrap();
    params.subject_alt_names = vec![rcgen::SanType::DnsName("127.0.0.1".try_into().unwrap())];
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = params.self_signed(&key).unwrap();
    fs::write(temp.0.join("cert.pem"), cert.pem()).unwrap();
    fs::write(temp.0.join("key.pem"), key.serialize_pem()).unwrap();
    let required = names(&["localhost", "127.0.0.1"]);
    let (cert_path, _) = self_signed_paths(&temp.0, &required).unwrap();
    let current = sans::parse(CertificateDer::from_pem_file(&cert_path).unwrap().as_ref()).unwrap();
    assert!(current.contains(&Name::Dns("127.0.0.1".into())));
    assert!(current.contains(&Name::Ip("127.0.0.1".parse().unwrap())));
    let before = fs::read(&cert_path).unwrap();
    self_signed_paths(&temp.0, &required).unwrap();
    assert_eq!(fs::read(cert_path).unwrap(), before);
}

#[test]
fn accumulation_caps_dns_and_ips_by_evicting_oldest_non_required_entries() {
    let temp = Temp::new();
    let mut old = names(&["localhost", "127.0.0.1"]);
    old.extend((0..10).map(|i| format!("old{i}.test")));
    old.extend((1..=18).map(|i| format!("10.0.0.{i}")));
    let (cert, _) = self_signed_paths(&temp.0, &old).unwrap();
    let required = names(&[
        "localhost",
        "127.0.0.1",
        "old0.test",
        "10.0.0.1",
        "new.test",
        "192.168.55.1",
    ]);
    self_signed_paths(&temp.0, &required).unwrap();
    let current = parsed_names(&CertificateDer::from_pem_file(cert).unwrap());
    let mut expected = names(&["localhost", "127.0.0.1", "old0.test"]);
    expected.extend((5..10).map(|i| format!("old{i}.test")));
    expected.push("10.0.0.1".into());
    expected.extend((6..=18).map(|i| format!("10.0.0.{i}")));
    expected.extend(names(&["new.test", "192.168.55.1"]));
    assert_eq!(current, expected);
    assert_eq!(
        current
            .iter()
            .filter(|s| s.parse::<IpAddr>().is_ok())
            .count(),
        16
    );
    assert_eq!(
        current
            .iter()
            .filter(|s| s.parse::<IpAddr>().is_err())
            .count(),
        8
    );
    assert!(required.iter().all(|name| current.contains(name)));
}

#[test]
fn required_sans_are_retained_even_when_they_exceed_both_caps() {
    let temp = Temp::new();
    let mut required = names(&["localhost", "127.0.0.1"]);
    required.extend((0..10).map(|i| format!("required{i}.test")));
    required.extend((1..=20).map(|i| format!("10.0.0.{i}")));
    let old = names(&["old.test", "192.168.1.1"]);
    self_signed_paths(&temp.0, &old).unwrap();
    let (cert, _) = self_signed_paths(&temp.0, &required).unwrap();
    let current = parsed_names(&CertificateDer::from_pem_file(cert).unwrap());
    assert_eq!(current, required);
}

fn check_self_signed_recovery(damage: impl FnOnce(&Path, &Path)) {
    let temp = Temp::new();
    load(&Tls::SelfSigned {
        state_dir: temp.0.clone(),
        sans: names(&["relay.local", "192.168.55.1", "old.local"]),
    })
    .unwrap()
    .unwrap();
    let cert = temp.0.join("cert.pem");
    let key = temp.0.join("key.pem");
    damage(&cert, &key);
    let damaged = [fs::read(&cert).ok(), fs::read(&key).ok()];
    assert!(certified_key(&cert, &key).is_err());
    let identity = load(&Tls::SelfSigned {
        state_dir: temp.0.clone(),
        sans: names(&["relay.local", "192.168.55.1"]),
    })
    .unwrap()
    .unwrap();
    for (path, before) in [(&cert, &damaged[0]), (&key, &damaged[1])] {
        assert_ne!(&Some(fs::read(path).unwrap()), before);
    }
    let current = certified_key(&cert, &key).unwrap();
    current.keys_match().unwrap();
    let mut expected = names(&["localhost", "127.0.0.1", "relay.local", "192.168.55.1"]);
    expected.extend(local_ipv4());
    expected.sort();
    expected.dedup();
    let mut actual = parsed_names(&current.cert[0]);
    actual.sort();
    assert_eq!(actual, expected);
    assert_eq!(identity.fingerprint, fingerprint(&current));
    for version in [&rustls::version::TLS12, &rustls::version::TLS13] {
        for name in ["localhost", "127.0.0.1", "relay.local", "192.168.55.1"] {
            assert_eq!(
                handshake(identity.config.clone(), &current.cert, name, true, version)[0],
                current.cert[0]
            );
        }
    }
    assert_eq!(
        fs::metadata(&key).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 2);
}

#[test]
fn incomplete_self_signed_pairs_are_regenerated_and_served() {
    for missing in ["cert.pem", "key.pem"] {
        check_self_signed_recovery(|cert, key| {
            fs::remove_file(if missing == "cert.pem" { cert } else { key }).unwrap();
        });
    }
}

#[test]
fn unparseable_self_signed_certificates_are_regenerated_and_served() {
    for certificate in [
        b"unparseable certificate".as_slice(),
        b"-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n".as_slice(),
    ] {
        check_self_signed_recovery(|cert, _| fs::write(cert, certificate).unwrap());
    }
}

#[test]
fn mismatched_self_signed_pairs_are_regenerated_and_served() {
    check_self_signed_recovery(|_, key| {
        // Simulate losing power after publishing the new key but before the new certificate.
        fs::write(key, rcgen::KeyPair::generate().unwrap().serialize_pem()).unwrap();
    });
}

#[test]
fn unparseable_self_signed_keys_are_regenerated_and_served() {
    check_self_signed_recovery(|_, key| fs::write(key, "unparseable key").unwrap());
}
