// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

use oxp_web::{Pairing, Tls, WebOutput, WebOutputConfig};
use serde_json::Value;
use std::{
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn generate(state_dir: &Path) -> String {
    WebOutput::new(WebOutputConfig::new(
        "127.0.0.1:0".parse().unwrap(),
        state_dir.into(),
        Pairing::Disabled,
        Tls::SelfSigned {
            state_dir: state_dir.into(),
            sans: vec![],
        },
    ))
    .unwrap();
    oxp_web::tls::load(&Tls::Pem {
        cert: state_dir.join("cert.pem"),
        key: state_dir.join("key.pem"),
    })
    .unwrap()
    .unwrap()
    .fingerprint
}

#[test]
fn startup_events_report_the_served_default_and_independent_trusted_fingerprints() {
    let temp =
        Temp(std::env::temp_dir().join(format!("oxp-default-startup-{}", std::process::id())));
    let generated = generate(&temp.0.join("generated"));
    let user = generate(&temp.0.join("user"));
    let trusted = generate(&temp.0.join("trusted"));
    for (has_default, has_trusted) in [(false, false), (true, false), (true, true)] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_oxp-relay"));
        command
            .args([
                "--stdio",
                "--web-port",
                "0",
                "--web-bind",
                "127.0.0.1",
                "--web-transcode",
                "off",
                "--web-root",
            ])
            .arg(&temp.0)
            .arg("--tls-self-signed")
            .arg(temp.0.join("generated"));
        if has_default {
            command
                .arg("--tls-default-cert")
                .arg(temp.0.join("user/cert.pem"))
                .arg("--tls-default-key")
                .arg(temp.0.join("user/key.pem"));
        }
        if has_trusted {
            command
                .arg("--tls-cert")
                .arg(temp.0.join("trusted/cert.pem"))
                .arg("--tls-key")
                .arg(temp.0.join("trusted/key.pem"))
                .args(["--tls-name", "localhost"]);
        }
        let mut process = Process(
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let (send, receive) = mpsc::channel();
        let stderr = process.0.stderr.take().unwrap();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                if let Ok(event) = serde_json::from_str::<Value>(&line) {
                    let _ = send.send(event);
                }
            }
        });
        let mut input = process.0.stdin.take().unwrap();
        let mut output = process.0.stdout.take().unwrap();
        oxp_wire::write(
            &mut input,
            &oxp_wire::Message::hello("producer", "test", "default").unwrap(),
        )
        .unwrap();
        assert!(
            matches!(oxp_wire::read(&mut output).unwrap(), Some(oxp_wire::Message::Json(value)) if value["type"] == "hello")
        );
        let event = loop {
            let event = receive.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_ne!(event["type"], "error", "{event}");
            if event["type"] == "tls-fingerprint" {
                break event;
            }
        };
        assert_eq!(event["algorithm"], "SHA-256");
        assert_eq!(
            event["fingerprint"],
            if has_default {
                user.as_str()
            } else {
                generated.as_str()
            }
        );
        assert_eq!(
            event["defaultFingerprint"],
            if has_default {
                Value::from(user.clone())
            } else {
                Value::Null
            }
        );
        assert_eq!(
            event["trustedFingerprint"],
            if has_trusted {
                Value::from(trusted.clone())
            } else {
                Value::Null
            }
        );
        drop(process);
        reader.join().unwrap();
    }
}
