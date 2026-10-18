// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use oxp_core::{model::*, ProjectionOutput};
use oxp_web::{http, json as codec, Pairing, Tls, WebOutput, WebOutputConfig};
use serde_json::{json, Value};
use std::{
    fs,
    io::{Cursor, Read, Write},
    net::TcpStream,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tungstenite::{Message, WebSocket};
static NEXT: AtomicU64 = AtomicU64::new(1);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "oxp-web-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[derive(Default)]
struct Host {
    controls: Mutex<Vec<ControlEvent>>,
    caps: Mutex<Vec<OutputCapabilities>>,
    stats: Mutex<Vec<OutputStats>>,
}
impl OutputHost for Host {
    fn capabilities(&self, c: OutputCapabilities) {
        self.caps.lock().unwrap().push(c);
    }
    fn control(&self, e: ControlEvent) {
        self.controls.lock().unwrap().push(e);
    }
    fn stats(&self, s: OutputStats) {
        self.stats.lock().unwrap().push(s);
    }
}
fn vector(name: &str) -> Value {
    serde_json::from_slice(
        &fs::read(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join(format!("../web-receiver/spec/vectors/web/{name}.json")),
        )
        .unwrap(),
    )
    .unwrap()
}
#[test]
fn vectors_and_strict_schemas() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../web-receiver/spec/vectors/web");
    for p in fs::read_dir(root).unwrap() {
        let p = p.unwrap().path();
        if p.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let s = fs::read_to_string(&p).unwrap();
        let v = codec::parse(&s, None).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        assert_eq!(v, codec::parse(&codec::encode(&v).unwrap(), None).unwrap());
        let mut bad = v;
        bad["unknown"] = json!(true);
        if matches!(bad["type"].as_str(), Some("receiver-info" | "stats")) {
            codec::validate(&bad, None).unwrap();
        } else {
            assert!(codec::validate(&bad, None).is_err());
        }
    }
    for bad in [json!(null), json!("true"), json!(0), json!(1)] {
        let mut v = vector("pause");
        v["paused"] = bad;
        assert!(codec::validate(&v, None).is_err());
    }
    for bad in [json!(-1), json!(256), json!(0.5)] {
        let mut v = vector("pause");
        v["screen"] = bad;
        assert!(codec::validate(&v, None).is_err());
    }
    assert!(codec::validate(&vector("pause"), Some(codec::Direction::Relay)).is_err());
    assert!(codec::validate(&vector("welcome"), Some(codec::Direction::Client)).is_err());
    for s in [
        r#"{"type":"ping","t0Us":1}"#,
        r#"{"type":"ping","t0Us":"01"}"#,
        r#"{"type":"ping","t0Us":"9223372036854775808"}"#,
    ] {
        assert!(codec::parse(s, None).is_err());
    }
    let mut h = vector("hello");
    h["decoders"][0]["extra"] = json!(true);
    assert!(codec::validate(&h, None).is_err());
}
#[test]
fn http_limits_routes_and_traversal() {
    for (request, status) in [
        ("GET / HTTP/1.0\r\nHost: h\r\n\r\n".into(), 400),
        ("GET / HTTP/1.1\r\n\r\n".into(), 400),
        ("GET / HTTP/1.1\r\nHost: h\r\nHOST: h\r\n\r\n".into(), 400),
        (
            "GET / HTTP/1.1\r\nHost: h\r\nContent-Length: 1\r\n\r\n".into(),
            400,
        ),
        (
            "GET / HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: chunked\r\n\r\n".into(),
            400,
        ),
        (
            format!("GET /{} HTTP/1.1\r\nHost: h\r\n\r\n", "a".repeat(4096)),
            431,
        ),
        (
            format!(
                "GET / HTTP/1.1\r\nHost: h\r\nX: {}\r\n\r\n",
                "a".repeat(8192)
            ),
            431,
        ),
        (
            format!(
                "GET / HTTP/1.1\r\nHost: h\r\n{}\r\n",
                (0..5)
                    .map(|i| format!("X{i}: {}\r\n", "a".repeat(7000)))
                    .collect::<String>()
            ),
            431,
        ),
        (
            format!(
                "GET / HTTP/1.1\r\nHost: h\r\n{}\r\n",
                (0..100).map(|i| format!("X{i}: a\r\n")).collect::<String>()
            ),
            400,
        ),
    ] {
        assert_eq!(http::read(&mut Cursor::new(request)).err(), Some(status));
    }
    let temp = Temp::new();
    fs::create_dir_all(temp.0.join("receiver")).unwrap();
    fs::create_dir_all(temp.0.join("probe")).unwrap();
    fs::write(temp.0.join("receiver/index.html"), "hello").unwrap();
    fs::write(temp.0.join("probe/index.html"), "probe").unwrap();
    fs::write(temp.0.join("worker.mjs"), "export {}").unwrap();
    std::os::unix::fs::symlink("/etc/passwd", temp.0.join("escape")).unwrap();
    for (path, body, ty) in [
        ("/?x=y", "hello", "text/html; charset=utf-8"),
        ("/probe/", "probe", "text/html; charset=utf-8"),
        ("/worker.mjs", "export {}", "text/javascript; charset=utf-8"),
    ] {
        let r = http::read(&mut Cursor::new(format!(
            "GET {path} HTTP/1.1\r\nHost: h\r\n\r\n"
        )))
        .unwrap();
        let mut out = Vec::new();
        http::serve(&r, &temp.0, &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains(ty));
        assert!(s.ends_with(body));
        if path.starts_with("/?") {
            assert!(s.contains("Cache-Control: no-store"));
        }
    }
    for path in [
        "/../secret",
        "/%2e%2e/secret",
        "/./worker.mjs",
        "/escape",
        "/a%00b",
        "/a%5cb",
        "//host/file",
        "/receiver/index.html#frag",
        "/%ZZ",
        "/%2fworker.mjs",
        "/worker.mjs?x=%ZZ",
        "/missing",
    ] {
        let r = http::read(&mut Cursor::new(format!(
            "GET {path} HTTP/1.1\r\nHost: h\r\n\r\n"
        )))
        .unwrap();
        assert_eq!(
            http::serve(&r, &temp.0, &mut Vec::new()),
            Err(404),
            "{path}"
        );
    }
}
#[derive(Debug)]
struct TestVerifier;
impl rustls::client::danger::ServerCertVerifier for TestVerifier {
    fn verify_server_cert(
        &self,
        _: &rustls_pki_types::CertificateDer<'_>,
        _: &[rustls_pki_types::CertificateDer<'_>],
        _: &rustls_pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls_pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &rustls_pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &rustls_pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![rustls::SignatureScheme::ECDSA_NISTP256_SHA256]
    }
}
type Socket = WebSocket<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>;
fn connect(addr: std::net::SocketAddr) -> Socket {
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .dangerous()
    .with_custom_certificate_verifier(Arc::new(TestVerifier))
    .with_no_client_auth();
    let tcp = TcpStream::connect(addr).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    tcp.set_write_timeout(Some(Duration::from_secs(3))).unwrap();
    let stream = rustls::StreamOwned::new(
        rustls::ClientConnection::new(
            Arc::new(config),
            rustls_pki_types::ServerName::try_from("localhost").unwrap(),
        )
        .unwrap(),
        tcp,
    );
    tungstenite::client(format!("wss://localhost:{}/oxp/v0", addr.port()), stream)
        .unwrap()
        .0
}
fn send(ws: &mut Socket, v: Value) {
    ws.send(Message::Text(v.to_string().into())).unwrap();
}
fn read_json(ws: &mut Socket, kind: &str) -> Value {
    let Message::Text(text) = ws.read().unwrap() else {
        panic!("Expected {kind}")
    };
    let v = codec::parse(&text, Some(codec::Direction::Relay)).unwrap();
    assert_eq!(v["type"], kind);
    v
}
fn barrier(ws: &mut Socket) {
    send(ws, json!({"type":"ping","t0Us":"123"}));
    let pong = read_json(ws, "pong");
    assert_eq!(pong["t0Us"], "123");
    assert!(
        pong["t2Us"].as_str().unwrap().parse::<i64>().unwrap()
            >= pong["t1Us"].as_str().unwrap().parse::<i64>().unwrap()
    );
}
fn wait_for(mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < Duration::from_secs(3));
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn video_format() -> Arc<VideoFormat> {
    let v = vector("video-format");
    use base64::Engine;
    Arc::new(
        VideoFormat::create(
            0,
            VideoCodec::H264,
            base64::engine::general_purpose::STANDARD
                .decode(v["format"]["config"].as_str().unwrap())
                .unwrap()
                .into(),
            None,
        )
        .unwrap(),
    )
}
fn frame(key: bool, pts: i64) -> VideoFrame {
    VideoFrame {
        screen: 0,
        session: 42,
        data: Arc::from([0, 0, 0, 2, 0x65, 0x80]),
        pts_us: pts,
        key_frame: key,
        received_at_nanos: 123000,
    }
}
#[test]
fn tls_loopback_hello_video_controls_pause_clock_and_persistence() {
    let temp = Temp::new();
    let cfg = WebOutputConfig::new(
        "127.0.0.1:0".parse().unwrap(),
        temp.0.clone(),
        Pairing::Required("example-pairing-token".into()),
        Tls::SelfSigned {
            state_dir: temp.0.join("tls"),
            sans: vec!["test.invalid".into()],
        },
    );
    let host = Arc::new(Host::default());
    let web = WebOutput::new(cfg.clone()).unwrap();
    web.open(host.clone()).unwrap();
    let addr = web.await_listening(Duration::from_secs(5)).unwrap();
    let cert = fs::read(temp.0.join("tls/cert.pem")).unwrap();
    let identity = oxp_web::tls::load(&cfg.tls).unwrap().unwrap();
    assert_eq!(cert, fs::read(temp.0.join("tls/cert.pem")).unwrap());
    assert_eq!(identity.fingerprint.len(), 95);
    assert_eq!(
        fs::metadata(temp.0.join("tls/key.pem"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let mut ws = connect(addr);
    send(&mut ws, vector("hello"));
    let welcome = read_json(&mut ws, "welcome");
    assert_eq!(welcome["attribution"], "Powered by OpenXPlay.");
    assert_eq!(welcome["session"]["phase"], "CONNECTING");
    web.on_session(
        &SessionState {
            id: 42,
            phase: SessionPhase::Active,
            reason: None,
        },
        &SessionConfig::default(),
    );
    web.on_video_format(0, Some(video_format()));
    read_json(&mut ws, "session");
    read_json(&mut ws, "video-format");
    web.on_video_frame(frame(false, 0));
    wait_for(|| !host.controls.lock().unwrap().is_empty());
    web.on_video_frame(frame(true, 10));
    let Message::Binary(b) = ws.read().unwrap() else {
        panic!("Expected keyframe")
    };
    let oxp_wire::Message::Binary { header, .. } = oxp_wire::decode(&b).unwrap() else {
        panic!()
    };
    assert_eq!(header.flags, 3);
    assert_eq!(header.sender_clock_us, 123);
    let b = oxp_wire::encode(&oxp_wire::Message::Binary {
        header: oxp_wire::Header {
            kind: oxp_wire::Kind::Touch,
            stream: 0,
            flags: 0,
            sequence: 0,
            pts_us: 0,
            sender_clock_us: 0,
        },
        payload: oxp_wire::encode_touch(&[oxp_wire::Contact {
            id: 0,
            x: 0.5,
            y: 0.25,
            pressed: true,
        }])
        .unwrap(),
    })
    .unwrap();
    ws.send(Message::Binary(b.into())).unwrap();
    wait_for(|| {
        host.controls
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e,ControlEvent::Touch{contacts,..} if contacts.len()==1))
    });
    send(&mut ws, json!({"type":"pause","screen":0,"paused":true}));
    barrier(&mut ws);
    web.on_video_frame(frame(true, 20));
    web.on_video_frame(frame(false, 30));
    barrier(&mut ws); // A binary here would fail the barrier.
    send(&mut ws, json!({"type":"pause","screen":0,"paused":false}));
    read_json(&mut ws, "video-format");
    web.on_video_frame(frame(false, 40));
    web.on_video_frame(frame(true, 50));
    let Message::Binary(b) = ws.read().unwrap() else {
        panic!()
    };
    let oxp_wire::Message::Binary { header, .. } = oxp_wire::decode(&b).unwrap() else {
        panic!()
    };
    assert_eq!(header.flags, 3);
    assert_eq!(header.pts_us, 50);
    send(&mut ws, vector("stats"));
    barrier(&mut ws);
    wait_for(|| host.stats.lock().unwrap().len() == 1);
    assert!(host
        .caps
        .lock()
        .unwrap()
        .iter()
        .all(|c| c.audio.is_empty() && !c.microphone));
    ws.close(None).unwrap();
    drop(ws);
    wait_for(|| {
        host.controls
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e,ControlEvent::Touch {contacts,..} if contacts.is_empty()))
    });
    let mut bad = connect(addr);
    let mut hello = vector("hello");
    hello["token"] = json!("wrong");
    send(&mut bad, hello);
    assert!(
        matches!(bad.read().unwrap(),Message::Close(Some(c)) if c.code==tungstenite::protocol::frame::coding::CloseCode::Policy)
    );
    web.close();
}
#[test]
fn plain_http_error_drains_and_static_server() {
    let temp = Temp::new();
    fs::create_dir_all(temp.0.join("receiver")).unwrap();
    fs::write(temp.0.join("receiver/index.html"), "receiver").unwrap();
    let web = WebOutput::new(WebOutputConfig::new(
        "127.0.0.1:0".parse().unwrap(),
        temp.0.clone(),
        Pairing::Required("token".into()),
        Tls::Disabled,
    ))
    .unwrap();
    web.open(Arc::new(Host::default())).unwrap();
    let addr = web.await_listening(Duration::from_secs(3)).unwrap();
    for (request, expected) in [
        ("GET / HTTP/1.1\r\nHost: h\r\n\r\n".to_owned(), "200 OK"),
        (
            "POST / HTTP/1.1\r\nHost: h\r\n\r\n".to_owned(),
            "405 Method Not Allowed",
        ),
        (
            format!(
                "GET / HTTP/1.1\r\nHost: h\r\nX: {}\r\n\r\n",
                "a".repeat(10000)
            ),
            "431 Request Header Fields Too Large",
        ),
    ] {
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        s.write_all(request.as_bytes()).unwrap();
        s.shutdown(std::net::Shutdown::Write).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        assert!(out.contains(expected), "{out}");
    }
    web.close();
}

#[test]
fn tls_identity_is_persisted_and_validates_sans_in_memory() {
    use rustls_pki_types::{pem::PemObject, CertificateDer, ServerName};
    let temp = Temp::new();
    let tls = Tls::SelfSigned {
        state_dir: temp.0.clone(),
        sans: vec!["relay.test".into()],
    };
    let first = oxp_web::tls::load(&tls).unwrap().unwrap();
    let cert = fs::read(temp.0.join("cert.pem")).unwrap();
    let key = fs::read(temp.0.join("key.pem")).unwrap();
    let second = oxp_web::tls::load(&tls).unwrap().unwrap();
    assert_eq!(first.fingerprint, second.fingerprint);
    assert_eq!(cert, fs::read(temp.0.join("cert.pem")).unwrap());
    assert_eq!(key, fs::read(temp.0.join("key.pem")).unwrap());
    assert_eq!(
        fs::metadata(temp.0.join("key.pem"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from_pem_slice(&cert).unwrap())
        .unwrap();
    let config = Arc::new(
        rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth(),
    );
    for name in ["localhost", "127.0.0.1", "relay.test", "wrong.test"] {
        let mut client = rustls::ClientConnection::new(
            config.clone(),
            ServerName::try_from(name.to_owned()).unwrap(),
        )
        .unwrap();
        let mut server = rustls::ServerConnection::new(first.config.clone()).unwrap();
        let mut failure = None;
        for _ in 0..10 {
            let mut bytes = Vec::new();
            client.write_tls(&mut bytes).unwrap();
            if !bytes.is_empty() {
                server.read_tls(&mut Cursor::new(bytes)).unwrap();
                if let Err(e) = server.process_new_packets() {
                    failure = Some(e);
                    break;
                }
            }
            let mut bytes = Vec::new();
            server.write_tls(&mut bytes).unwrap();
            if !bytes.is_empty() {
                client.read_tls(&mut Cursor::new(bytes)).unwrap();
                if let Err(e) = client.process_new_packets() {
                    failure = Some(e);
                    break;
                }
            }
            if !client.is_handshaking() && !server.is_handshaking() {
                break;
            }
        }
        if name == "wrong.test" {
            assert!(failure.is_some());
        } else {
            assert!(failure.is_none(), "{name}: {failure:?}");
            assert!(!client.is_handshaking());
        }
    }
    let pem = Tls::Pem {
        cert: temp.0.join("cert.pem"),
        key: temp.0.join("key.pem"),
    };
    assert_eq!(
        oxp_web::tls::load(&pem).unwrap().unwrap().fingerprint,
        first.fingerprint
    );
}

#[test]
fn preferences_and_restart_are_strict() {
    let original = vector("hello-preferences");
    for mode in ["SMART", "QUALITY", "SPEED"] {
        let mut v = original.clone();
        v["playback"] = json!(mode);
        codec::validate(&v, Some(codec::Direction::Client)).unwrap();
    }
    for rate in [
        "AUTO", "FPS_30", "FPS_35", "FPS_40", "FPS_45", "FPS_50", "FPS_55", "FPS_60",
    ] {
        let mut v = original.clone();
        v["displays"][0]["frameRate"] = json!(rate);
        codec::validate(&v, None).unwrap();
    }
    for bad in [json!("AUTO"), json!(null), json!(1)] {
        let mut v = original.clone();
        v["playback"] = bad;
        assert!(codec::validate(&v, None).is_err());
    }
    for (field, bad) in [
        ("frameRate", json!("FPS_120")),
        ("frameRate", json!(null)),
        ("frameRate", json!(60)),
        ("sustainedFps", json!(0)),
        ("sustainedFps", json!(-1)),
        ("sustainedFps", json!(null)),
        ("sustainedFps", json!("60")),
        ("unknown", json!(true)),
    ] {
        let mut v = original.clone();
        v["displays"][0][field] = bad;
        assert!(codec::validate(&v, None).is_err(), "{field}");
    }
    codec::validate(&vector("restart"), Some(codec::Direction::Client)).unwrap();
    assert!(codec::validate(&vector("restart"), Some(codec::Direction::Relay)).is_err());
    assert!(codec::validate(&json!({"type":"restart","screen":0}), None).is_err());
    let d = oxp_core::model::DisplayInfo::default();
    assert_eq!(d.frame_rate, oxp_core::model::FrameRatePreference::Auto);
    assert_eq!(d.sustained_fps, None);
    assert_eq!(d.refresh_hz, 60.0);
}
