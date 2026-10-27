// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

use oxp_bin::{clink_support::Sink, Consumer, EventLog};
use oxp_core::{model::*, Hub};
use oxp_web::{json as codec, Pairing, Tls, WebOutput, WebOutputConfig};
use oxp_wire::Message;
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::{atomic::AtomicBool, mpsc, Arc, Mutex},
    time::{Duration, Instant},
};

// A minimal masked WebSocket client keeps this bridge test independent of receiver code.
struct Page(TcpStream);
impl Page {
    fn connect(address: std::net::SocketAddr) -> Self {
        let mut socket = TcpStream::connect(address).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        write!(socket, "GET /oxp/v0 HTTP/1.1\r\nHost: {address}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n").unwrap();
        let mut response = Vec::new();
        while !response.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            socket.read_exact(&mut byte).unwrap();
            response.extend(byte);
            assert!(response.len() < 8192);
        }
        assert!(response.starts_with(b"HTTP/1.1 101"));
        Self(socket)
    }
    fn send(&mut self, value: &Value) {
        let bytes = serde_json::to_vec(value).unwrap();
        let mut frame = vec![0x81];
        if bytes.len() < 126 {
            frame.push(0x80 | bytes.len() as u8);
        } else {
            frame.push(0x80 | 126);
            frame.extend((bytes.len() as u16).to_be_bytes());
        }
        frame.extend([0; 4]);
        frame.extend(bytes);
        self.0.write_all(&frame).unwrap();
    }
    fn receive(&mut self) -> Value {
        let mut header = [0; 2];
        self.0.read_exact(&mut header).unwrap();
        assert_eq!(header[0], 0x81);
        assert_eq!(header[1] & 0x80, 0);
        let size = match header[1] {
            126 => {
                let mut n = [0; 2];
                self.0.read_exact(&mut n).unwrap();
                u16::from_be_bytes(n) as usize
            }
            n if n < 126 => n as usize,
            _ => panic!("unexpected oversized test message"),
        };
        let mut bytes = vec![0; size];
        self.0.read_exact(&mut bytes).unwrap();
        codec::parse(
            std::str::from_utf8(&bytes).unwrap(),
            Some(codec::Direction::Relay),
        )
        .unwrap()
    }
}

#[test]
fn wire_hub_web_preserves_startup_host_ui_settings_and_rendered_session() {
    let (tx, rx) = mpsc::sync_channel(128);
    let host = Arc::new(Sink {
        tx,
        state: Mutex::new(("default".into(), [0; 256])),
        clock: Instant::now(),
        shutdown: Arc::new(|| {}),
        failed: Arc::new(AtomicBool::new(false)),
        log: EventLog::global().clone(),
    });
    let hub = Hub::new(host);
    let mut config = WebOutputConfig::new(
        "127.0.0.1:0".parse().unwrap(),
        env!("CARGO_MANIFEST_DIR").into(),
        Pairing::Disabled,
        Tls::Disabled,
    );
    config.host_ui = true;
    let web = Arc::new(WebOutput::new(config).unwrap());
    hub.add_output(web.clone()).unwrap();
    let mut page = Page::connect(web.await_listening(Duration::from_secs(3)).unwrap());
    let hello: Value = serde_json::from_str(include_str!(
        "../../web-receiver/spec/vectors/web/hello.json"
    ))
    .unwrap();
    page.send(&hello);
    assert_eq!(page.receive()["features"], json!(["host-ui"]));
    let mut consumer = Consumer::default();
    for reason in [
        "connecting",
        "authenticating",
        "joining-network",
        "waiting-media",
        "failed",
        "disconnected",
    ] {
        let message = Message::json(json!({"type":"session","session":{"id":"1","phase":"CONNECTING","reason":format!("startup/{reason}")}})).unwrap();
        let encoded = oxp_wire::encode(&message).unwrap();
        consumer
            .receive(oxp_wire::decode(&encoded).unwrap(), hub.as_ref())
            .unwrap();
        let value = page.receive();
        assert_eq!(value["type"], "session");
        assert_eq!(value["session"]["reason"], format!("startup/{reason}"));
    }
    consumer
        .receive(
            Message::json(json!({"type":"host-ui"})).unwrap(),
            hub.as_ref(),
        )
        .unwrap();
    assert_eq!(page.receive(), json!({"type":"host-ui"}));
    let settings = DisplaySettings {
        frame_rate: FrameRatePreference::Fps50,
        ..DisplaySettings::default()
    };
    consumer
        .receive(
            Message::json(oxp_wire::display_settings(&settings)).unwrap(),
            hub.as_ref(),
        )
        .unwrap();
    assert_eq!(
        page.receive()["features"],
        json!(["display-settings", "host-ui"])
    );
    assert_eq!(page.receive()["frameRate"], "FPS_50");
    page.send(&json!({"type":"display-settings","resolution":"native","screenWidth":200}));
    let mut stats: Value = serde_json::from_str(include_str!(
        "../../../spec/vectors/wire/stats-session.json"
    ))
    .unwrap();
    stats["session"] = json!(-42);
    page.send(&stats);
    let end = Instant::now() + Duration::from_secs(3);
    let mut settings_seen = false;
    let mut stats_seen = false;
    while !settings_seen || !stats_seen {
        let message = rx
            .recv_timeout(end.saturating_duration_since(Instant::now()))
            .unwrap();
        let Message::Json(value) = message else {
            continue;
        };
        oxp_wire::validate_json(&value).unwrap();
        match value["type"].as_str() {
            Some("display-settings") => {
                assert_eq!(value["frameRate"], "FPS_50");
                settings_seen = true;
            }
            Some("stats") => {
                assert_eq!(value["session"], "-42");
                stats_seen = true;
            }
            _ => {}
        }
    }
    hub.close();
}
