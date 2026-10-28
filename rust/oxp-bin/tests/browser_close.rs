// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

use oxp_wire::Message;
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    os::unix::net::UnixStream,
    process::{Child, Command, Stdio},
    sync::mpsc::{channel, Receiver},
    time::{Duration, Instant},
};
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn event(rx: &Receiver<Value>, kind: &str) -> Value {
    let end = Instant::now() + Duration::from_secs(8);
    loop {
        let value = rx
            .recv_timeout(end.saturating_duration_since(Instant::now()))
            .unwrap();
        assert_ne!(value["type"], "error", "{value}");
        if value["type"] == kind {
            return value;
        }
    }
}
fn frame(socket: &mut TcpStream, opcode: u8, bytes: &[u8]) {
    let mut frame = vec![0x80 | opcode];
    if bytes.len() < 126 {
        frame.push(0x80 | bytes.len() as u8);
    } else {
        frame.push(0x80 | 126);
        frame.extend((bytes.len() as u16).to_be_bytes());
    }
    frame.extend([0; 4]);
    frame.extend(bytes);
    socket.write_all(&frame).unwrap();
}
#[test]
fn browser_close_reaches_relay_stdout_with_the_receiver_diagnostic_id() {
    let root = std::env::temp_dir().join(format!("oxp-browser-close-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let wire_path = root.join("wire.sock");
    let reservation = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reservation.local_addr().unwrap();
    drop(reservation);
    let mut child = Process(
        Command::new(env!("CARGO_BIN_EXE_oxp-relay"))
            .args([
                "--unix",
                wire_path.to_str().unwrap(),
                "--web-port",
                &address.port().to_string(),
                "--web-bind",
                "127.0.0.1",
                "--web-root",
                root.to_str().unwrap(),
                "--no-tls",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let stdout = child.0.stdout.take().unwrap();
    let (tx, rx) = channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let value: Value = serde_json::from_str(&line.unwrap()).unwrap();
            if tx.send(value).is_err() {
                break;
            }
        }
    });
    let end = Instant::now() + Duration::from_secs(8);
    let mut wire = loop {
        if let Ok(socket) = UnixStream::connect(&wire_path) {
            break socket;
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(5));
    };
    wire.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    assert!(
        matches!(oxp_wire::read(&mut wire).unwrap(), Some(Message::Json(v)) if v["type"] == "hello")
    );
    oxp_wire::write(
        &mut wire,
        &Message::hello("producer", "test", "default").unwrap(),
    )
    .unwrap();
    event(&rx, "pairing-url");
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    socket.write_all(b"GET /oxp/v0 HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n").unwrap();
    let mut response = Vec::new();
    while !response.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        socket.read_exact(&mut byte).unwrap();
        response.extend(byte);
        assert!(response.len() < 8192);
    }
    assert!(response.starts_with(b"HTTP/1.1 101"));
    frame(
        &mut socket,
        1,
        include_bytes!("../../web-receiver/spec/vectors/web/hello.json"),
    );
    frame(
        &mut socket,
        1,
        include_bytes!("../../web-receiver/spec/vectors/web/receiver-info.json"),
    );
    let info = event(&rx, "receiver-info");
    frame(&mut socket, 8, &1000u16.to_be_bytes());
    let closed = event(&rx, "receiver-disconnected");
    assert_eq!(
        closed,
        json!({"type":"receiver-disconnected","client":info["client"],"code":1000,"reason":"closed"})
    );
    assert!(info["client"].as_str().unwrap().starts_with("browser-"));
    drop(child);
    reader.join().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
