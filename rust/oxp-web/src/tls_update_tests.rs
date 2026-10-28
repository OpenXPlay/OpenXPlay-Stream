// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

use super::*;
use rustls::{crypto::ring, ClientConfig, ClientConnection, RootCertStore};
use rustls_pki_types::{pem::PemObject, CertificateDer, ServerName};
use std::fs;

type Socket = WebSocket<rustls::StreamOwned<ClientConnection, TcpStream>>;
static NEXT: AtomicU64 = AtomicU64::new(1);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "oxp-tls-update-{}-{}",
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
struct Host;
impl OutputHost for Host {
    fn capabilities(&self, _: OutputCapabilities) {}
    fn control(&self, _: ControlEvent) {}
    fn stats(&self, _: OutputStats) {}
}
fn pem(root: &Temp, name: &str) -> (tls::TrustedPem, CertificateDer<'static>) {
    let generated = rcgen::generate_simple_self_signed(vec![name.into()]).unwrap();
    let cert = root.0.join(format!("{name}-cert.pem"));
    let key = root.0.join(format!("{name}-key.pem"));
    fs::write(&cert, generated.cert.pem()).unwrap();
    fs::write(&key, generated.key_pair.serialize_pem()).unwrap();
    (
        tls::TrustedPem {
            name: name.into(),
            cert,
            key,
        },
        generated.cert.der().clone(),
    )
}
fn connect(
    address: SocketAddr,
    name: &str,
    roots: &[CertificateDer<'static>],
    version: &'static rustls::SupportedProtocolVersion,
) -> Socket {
    let mut store = RootCertStore::empty();
    for root in roots {
        store.add(root.clone()).unwrap();
    }
    let config = ClientConfig::builder_with_provider(Arc::new(ring::default_provider()))
        .with_protocol_versions(&[version])
        .unwrap()
        .with_root_certificates(store)
        .with_no_client_auth();
    let tcp = TcpStream::connect(address).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    tcp.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
    let stream = rustls::StreamOwned::new(
        ClientConnection::new(
            Arc::new(config),
            ServerName::try_from(name.to_owned()).unwrap(),
        )
        .unwrap(),
        tcp,
    );
    let mut ws = tungstenite::client(format!("wss://{name}:{}/oxp/v0", address.port()), stream)
        .unwrap()
        .0;
    ws.send(Message::Text(
        include_str!("../../web-receiver/spec/vectors/web/hello.json").into(),
    ))
    .unwrap();
    assert!(matches!(ws.read().unwrap(), Message::Text(_)));
    ws
}
fn peer(socket: &Socket) -> CertificateDer<'static> {
    socket.get_ref().conn.peer_certificates().unwrap()[0].clone()
}
fn ping(socket: &mut Socket) {
    socket
        .send(Message::Text(
            json!({"type":"ping","t0Us":"123"}).to_string().into(),
        ))
        .unwrap();
    let Message::Text(reply) = socket.read().unwrap() else {
        panic!("Expected pong on retained connection")
    };
    assert_eq!(
        serde_json::from_str::<Value>(&reply).unwrap()["type"],
        "pong"
    );
}

#[test]
fn hot_swap_changes_trusted_name_and_sans_without_interrupting_existing_connections() {
    let root = Temp::new();
    let (first, first_cert) = pem(&root, "first.test");
    let (second, second_cert) = pem(&root, "second.test");
    let state_dir = root.0.join("local");
    let sans: Vec<String> = ["first.test", "second.test"]
        .into_iter()
        .map(Into::into)
        .collect();
    let config = WebOutputConfig::new(
        "127.0.0.1:0".parse().unwrap(),
        root.0.clone(),
        Pairing::Disabled,
        Tls::Sni {
            name: first.name.clone(),
            cert: first.cert,
            key: first.key,
            state_dir: state_dir.clone(),
            sans: sans.clone(),
        },
    );
    let web = WebOutput::new(config).unwrap();
    web.open(Arc::new(Host)).unwrap();
    let address = web.await_listening(Duration::from_secs(5)).unwrap();
    let local = CertificateDer::from_pem_file(state_dir.join("cert.pem")).unwrap();
    let mut old = connect(
        address,
        "first.test",
        &[first_cert.clone(), local.clone()],
        &rustls::version::TLS13,
    );
    let mut old_ip = connect(
        address,
        "127.0.0.1",
        std::slice::from_ref(&local),
        &rustls::version::TLS12,
    );
    assert_eq!(peer(&old), first_cert);
    assert_eq!(peer(&old_ip), local);
    let mut updated = sans.clone();
    updated.push("192.0.2.7".into());
    let event = web
        .update_tls(Some(second.clone()), updated.clone())
        .unwrap();
    assert_eq!(event["trustedName"], "second.test");
    assert!(event["trustedFingerprint"].is_string());
    let replacement = CertificateDer::from_pem_file(state_dir.join("cert.pem")).unwrap();
    assert_ne!(replacement, local);
    for version in [&rustls::version::TLS12, &rustls::version::TLS13] {
        let roots = [replacement.clone(), second_cert.clone()];
        for (name, expected) in [
            ("second.test", &second_cert),
            ("first.test", &replacement),
            ("192.0.2.7", &replacement),
        ] {
            let mut socket = connect(address, name, &roots, version);
            assert_eq!(&peer(&socket), expected, "{name}");
            ping(&mut socket);
            socket.close(None).unwrap();
        }
    }
    ping(&mut old);
    ping(&mut old_ip);
    let mut previous_trusted = connect(
        address,
        "second.test",
        std::slice::from_ref(&second_cert),
        &rustls::version::TLS13,
    );
    let (renewed, renewed_cert) = pem(&root, "second.test");
    let renewed_event = web.update_tls(Some(renewed), updated.clone()).unwrap();
    assert_ne!(
        renewed_event["trustedFingerprint"],
        event["trustedFingerprint"]
    );
    let mut renewed_socket = connect(
        address,
        "second.test",
        std::slice::from_ref(&renewed_cert),
        &rustls::version::TLS13,
    );
    assert_eq!(peer(&renewed_socket), renewed_cert);
    ping(&mut previous_trusted);
    assert_eq!(peer(&previous_trusted), second_cert);
    ping(&mut renewed_socket);
    let before = [
        fs::read(state_dir.join("cert.pem")).unwrap(),
        fs::read(state_dir.join("key.pem")).unwrap(),
    ];
    web.update_tls(Some(second), sans.clone()).unwrap();
    assert_eq!(
        before,
        [
            fs::read(state_dir.join("cert.pem")).unwrap(),
            fs::read(state_dir.join("key.pem")).unwrap()
        ]
    );
    let event = web.update_tls(None, updated).unwrap();
    assert!(event.get("trustedName").is_none());
    let mut socket = connect(
        address,
        "second.test",
        &[replacement],
        &rustls::version::TLS13,
    );
    ping(&mut socket);
    ping(&mut old);
    assert_eq!(peer(&old), first_cert);
    web.close();
}

#[test]
fn invalid_replacements_keep_the_entire_served_set_and_cached_files() {
    let root = Temp::new();
    let (valid, _) = pem(&root, "relay.test");
    let state_dir = root.0.join("local");
    let web = WebOutput::new(WebOutputConfig::new(
        "127.0.0.1:0".parse().unwrap(),
        root.0.clone(),
        Pairing::Disabled,
        Tls::SelfSigned {
            state_dir: state_dir.clone(),
            sans: Vec::new(),
        },
    ))
    .unwrap();
    web.update_tls(Some(valid.clone()), vec![]).unwrap();
    let original = web
        .tls
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .fingerprint_event();
    let files = [
        fs::read(state_dir.join("cert.pem")).unwrap(),
        fs::read(state_dir.join("key.pem")).unwrap(),
    ];
    let mut wrong_name = valid.clone();
    wrong_name.name = "wrong.test".into();
    let mut missing = valid.clone();
    missing.cert = root.0.join("absent.pem");
    let mut wrong_key = valid.clone();
    wrong_key.key = pem(&root, "other.test").0.key;
    let mut invalid_name = valid;
    invalid_name.name = "invalid/name".into();
    for trusted in [wrong_name, missing, wrong_key, invalid_name] {
        assert!(web
            .update_tls(Some(trusted), vec!["192.0.2.8".into()])
            .is_err());
        assert_eq!(
            web.tls
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .fingerprint_event(),
            original
        );
        assert_eq!(
            files,
            [
                fs::read(state_dir.join("cert.pem")).unwrap(),
                fs::read(state_dir.join("key.pem")).unwrap()
            ]
        );
    }
    assert!(web.update_tls(None, vec!["invalid/name".into()]).is_err());
    let plain = WebOutput::new(WebOutputConfig::new(
        "127.0.0.1:0".parse().unwrap(),
        root.0.clone(),
        Pairing::Disabled,
        Tls::Disabled,
    ))
    .unwrap();
    assert!(plain.update_tls(None, vec![]).is_err());
    let (trusted, _) = pem(&root, "standalone.test");
    let pem_only = WebOutput::new(WebOutputConfig::new(
        "127.0.0.1:0".parse().unwrap(),
        root.0.clone(),
        Pairing::Disabled,
        Tls::Pem {
            cert: trusted.cert,
            key: trusted.key,
        },
    ))
    .unwrap();
    assert!(pem_only.update_tls(None, vec![]).is_err());
}

#[test]
fn user_default_install_keep_replace_and_remove_preserve_connections_and_trusted_sni() {
    let root = Temp::new();
    let state_dir = root.0.join("local");
    let web = WebOutput::new(WebOutputConfig::new(
        "127.0.0.1:0".parse().unwrap(),
        root.0.clone(),
        Pairing::Disabled,
        Tls::SelfSigned {
            state_dir: state_dir.clone(),
            sans: vec![],
        },
    ))
    .unwrap();
    web.open(Arc::new(Host)).unwrap();
    let address = web.await_listening(Duration::from_secs(5)).unwrap();
    let generated = CertificateDer::from_pem_file(state_dir.join("cert.pem")).unwrap();
    let mut old = connect(
        address,
        "127.0.0.1",
        std::slice::from_ref(&generated),
        &rustls::version::TLS12,
    );
    let (trusted, trusted_cert) = pem(&root, "trusted.test");
    let (first, first_cert) = pem(&root, "127.0.0.1");
    let event = web
        .update_tls_with_default(
            Some(trusted.clone()),
            vec![],
            tls::DefaultUpdate::Replace(tls::DefaultPem {
                cert: first.cert.clone(),
                key: first.key.clone(),
            }),
        )
        .unwrap();
    assert_eq!(event["defaultFingerprint"], event["fingerprint"]);
    let original = event.clone();
    let mut user = connect(
        address,
        "127.0.0.1",
        std::slice::from_ref(&first_cert),
        &rustls::version::TLS13,
    );
    assert_eq!(peer(&user), first_cert);
    let mut trusted_socket = connect(
        address,
        "trusted.test",
        std::slice::from_ref(&trusted_cert),
        &rustls::version::TLS12,
    );
    assert_eq!(peer(&trusted_socket), trusted_cert);
    ping(&mut old);
    // Keep uses the validated installed key even if its source files disappear.
    fs::remove_file(&first.cert).unwrap();
    fs::remove_file(&first.key).unwrap();
    let sans = vec!["192.0.2.77".into()];
    assert_eq!(
        web.update_tls(Some(trusted.clone()), sans.clone()).unwrap(),
        original
    );
    let (second, second_cert) = pem(&root, "127.0.0.1");
    let default = tls::DefaultPem {
        cert: second.cert.clone(),
        key: second.key.clone(),
    };
    let event = web
        .update_tls_with_default(
            Some(trusted.clone()),
            sans.clone(),
            tls::DefaultUpdate::Replace(default),
        )
        .unwrap();
    assert_ne!(event["defaultFingerprint"], original["defaultFingerprint"]);
    assert_eq!(event["trustedFingerprint"], original["trustedFingerprint"]);
    let mut current = connect(
        address,
        "127.0.0.1",
        std::slice::from_ref(&second_cert),
        &rustls::version::TLS12,
    );
    assert_eq!(peer(&current), second_cert);
    ping(&mut user);
    // A valid default combined with invalid trusted material must install neither half.
    let mut invalid_trusted = trusted.clone();
    invalid_trusted.key = second.key.clone();
    assert!(web
        .update_tls_with_default(
            Some(invalid_trusted),
            sans.clone(),
            tls::DefaultUpdate::Remove
        )
        .is_err());
    assert_eq!(
        web.tls
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .fingerprint_event(),
        event
    );
    let event = web
        .update_tls_with_default(Some(trusted), sans, tls::DefaultUpdate::Remove)
        .unwrap();
    assert_eq!(event["defaultFingerprint"], Value::Null);
    assert_eq!(event["trustedFingerprint"], original["trustedFingerprint"]);
    let restored = CertificateDer::from_pem_file(state_dir.join("cert.pem")).unwrap();
    assert_ne!(restored, generated);
    for version in [&rustls::version::TLS12, &rustls::version::TLS13] {
        let mut ip = connect(
            address,
            "192.0.2.77",
            std::slice::from_ref(&restored),
            version,
        );
        assert_eq!(peer(&ip), restored);
        ping(&mut ip);
        let mut named = connect(
            address,
            "trusted.test",
            std::slice::from_ref(&trusted_cert),
            version,
        );
        assert_eq!(peer(&named), trusted_cert);
        ping(&mut named);
    }
    for socket in [&mut old, &mut user, &mut current, &mut trusted_socket] {
        ping(socket);
    }
    web.close();
}

#[test]
fn invalid_defaults_leave_served_set_and_generated_cache_untouched() {
    let root = Temp::new();
    let state_dir = root.0.join("local");
    let web = WebOutput::new(WebOutputConfig::new(
        "127.0.0.1:0".parse().unwrap(),
        root.0.clone(),
        Pairing::Disabled,
        Tls::SelfSigned {
            state_dir: state_dir.clone(),
            sans: vec![],
        },
    ))
    .unwrap();
    let (trusted, _) = pem(&root, "trusted.test");
    let (user, _) = pem(&root, "127.0.0.1");
    let default = tls::DefaultPem {
        cert: user.cert.clone(),
        key: user.key.clone(),
    };
    web.update_tls_with_default(
        Some(trusted.clone()),
        vec![],
        tls::DefaultUpdate::Replace(default.clone()),
    )
    .unwrap();
    let before = web
        .tls
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .fingerprint_event();
    let files = [
        fs::read(state_dir.join("cert.pem")).unwrap(),
        fs::read(state_dir.join("key.pem")).unwrap(),
    ];
    let mut bad_chain = default.clone();
    bad_chain.cert = root.0.join("malformed.pem");
    fs::write(
        &bad_chain.cert,
        format!(
            "{}-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n",
            fs::read_to_string(&default.cert).unwrap()
        ),
    )
    .unwrap();
    let invalid = [
        tls::DefaultPem {
            cert: root.0.join("missing"),
            ..default.clone()
        },
        tls::DefaultPem {
            key: trusted.key.clone(),
            ..default
        },
        bad_chain,
    ];
    for default in invalid {
        assert!(web
            .update_tls_with_default(
                None,
                vec!["192.0.2.88".into()],
                tls::DefaultUpdate::Replace(default)
            )
            .is_err());
        assert_eq!(
            web.tls
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .fingerprint_event(),
            before
        );
        assert_eq!(
            files,
            [
                fs::read(state_dir.join("cert.pem")).unwrap(),
                fs::read(state_dir.join("key.pem")).unwrap()
            ]
        );
    }
}
