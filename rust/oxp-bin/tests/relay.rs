// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use oxp_bin::{replay_frames, video_format, Consumer, FileTrust};
use oxp_clink::{ClinkConnection, ClinkLink, ClinkOutput, PinProvider, TrustStore};
use oxp_wire::{Header, Kind, Message};
use serde_json::json;
use std::{
    io::Write,
    os::unix::{fs::PermissionsExt, net::UnixStream},
    sync::Arc,
};
struct FakeLink;
impl ClinkLink for FakeLink {
    fn connect(
        &self,
        _: Arc<dyn Fn(Option<String>) + Send + Sync>,
    ) -> oxp_clink::Result<ClinkConnection> {
        Err("unused fake link".into())
    }
    fn disconnect(&self) -> oxp_clink::Result<()> {
        Ok(())
    }
}
struct NoPin;
impl PinProvider for NoPin {
    fn request_pin(&self, _: &str) -> Option<String> {
        None
    }
}
fn avcc() -> Vec<u8> {
    include_str!("../../../spec/vectors/avc-baseline-640x480.hex")
        .split_whitespace()
        .map(|s| u8::from_str_radix(s, 16).unwrap())
        .collect()
}
#[test]
fn producer_to_consumer_socket_pair() {
    let dir = std::env::temp_dir().join(format!("oxp-relay-test-{}", std::process::id()));
    let output = ClinkOutput::new(
        Arc::new(FakeLink),
        Arc::new(NoPin),
        Arc::new(FileTrust::new(&dir).unwrap()),
    )
    .unwrap();
    let format = oxp_clink::model::VideoFormat::create(
        0,
        oxp_media::VideoCodec::H264,
        avcc().into(),
        Some(30.0),
    )
    .unwrap();
    let (mut producer, mut consumer) = UnixStream::pair().unwrap();
    let worker = std::thread::spawn(move || {
        for m in [
            Message::hello("producer", "replay", "default").unwrap(),
            Message::Json(json!({"type":"session","session":{"id":"1","phase":"ACTIVE"}})),
            video_format(&format),
            Message::Binary {
                header: Header {
                    kind: Kind::Video,
                    stream: 0,
                    flags: 1,
                    sequence: 0,
                    pts_us: 0,
                    sender_clock_us: 0,
                },
                payload: vec![0, 0, 0, 2, 0x65, 0x80],
            },
            Message::Json(json!({"type":"video-format","screen":0,"format":null})),
        ] {
            oxp_wire::write(&mut producer, &m).unwrap();
        }
        producer.flush().unwrap();
    });
    assert!(
        matches!(oxp_wire::read(&mut consumer).unwrap(),Some(Message::Json(v)) if v["type"]=="hello")
    );
    let mut state = Consumer::default();
    let mut count = 0;
    while let Some(m) = oxp_wire::read(&mut consumer).unwrap() {
        state.receive(m, &output).unwrap();
        count += 1;
    }
    assert_eq!(count, 4);
    assert!(state
        .receive(
            Message::Binary {
                header: Header {
                    kind: Kind::Video,
                    stream: 0,
                    flags: 0,
                    sequence: 1,
                    pts_us: 0,
                    sender_clock_us: 0
                },
                payload: vec![]
            },
            &output
        )
        .is_err());
    worker.join().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn trust_round_trip_and_permissions() {
    let dir = std::env::temp_dir().join(format!("oxp-trust-test-{}", std::process::id()));
    let trust = FileTrust::new(&dir).unwrap();
    assert!(trust.load_identity().unwrap().is_none());
    let identity = oxp_clink::protocol::AuthIdentity::generate();
    trust.save_identity(&identity).unwrap();
    assert_eq!(
        trust.load_identity().unwrap().unwrap().public_key(),
        identity.public_key()
    );
    trust
        .save_peer_key("head-unit", &identity.public_key())
        .unwrap();
    assert_eq!(
        trust.load_peer_key("head-unit").unwrap().unwrap(),
        identity.public_key()
    );
    assert_eq!(
        std::fs::metadata(dir.join("trust.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    trust.forget_peer("head-unit").unwrap();
    assert!(trust.load_peer_key("head-unit").unwrap().is_none());
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn replay_groups_slices_and_keeps_parameter_sets() {
    let config = oxp_media::AvcConfig::parse(&avcc()).unwrap();
    let mut data = Vec::new();
    for n in [
        &config.sps[0][..],
        &config.pps[0][..],
        &[0x65, 0x80][..],
        &[0x65, 0x40][..],
        &[0x41, 0x80][..],
    ] {
        data.extend([0, 0, 0, 1]);
        data.extend(n);
    }
    let (bytes, frames) = replay_frames(&data).unwrap();
    assert_eq!(bytes, avcc());
    assert_eq!(frames.len(), 2);
    assert!(frames[0].1);
    assert!(!frames[1].1);
    assert_eq!(
        oxp_media::nal::units(&frames[0].0, oxp_media::nal::Format::Avcc)
            .unwrap()
            .len(),
        4
    );
    assert!(replay_frames(&[0, 0, 1, 0x65, 0x80]).is_err());
}

#[test]
fn replay_binary_handshake_loop_and_controls() {
    use std::process::{Command, Stdio};
    let dir = std::env::temp_dir().join(format!("oxp-replay-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let config = oxp_media::AvcConfig::parse(&avcc()).unwrap();
    let mut data = Vec::new();
    for n in [
        &config.sps[0][..],
        &config.pps[0][..],
        &[0x65, 0x80][..],
        &[0x41, 0x80][..],
    ] {
        data.extend([0, 0, 0, 1]);
        data.extend(n);
    }
    let file = dir.join("sample.h264");
    std::fs::write(&file, data).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_oxp-replay"))
        .args(["--stdio", "--file", file.to_str().unwrap(), "--fps", "10"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = child.stdout.take().unwrap();
    assert!(
        matches!(oxp_wire::read(&mut output).unwrap(),Some(Message::Json(v)) if v["type"]=="hello" && v["role"]=="producer")
    );
    oxp_wire::write(
        &mut input,
        &Message::hello("consumer", "fake", "test-client").unwrap(),
    )
    .unwrap();
    for kind in ["input-capabilities", "session", "video-format"] {
        assert!(
            matches!(oxp_wire::read(&mut output).unwrap(),Some(Message::Json(v)) if v["type"]==kind)
        );
    }
    let first = oxp_wire::read(&mut output).unwrap().unwrap();
    assert!(matches!(first,Message::Binary{ref header,..} if header.flags==1 && header.pts_us==0));
    oxp_wire::write(
        &mut input,
        &Message::Json(json!({"type":"keyframe","screen":0})),
    )
    .unwrap();
    oxp_wire::write(
        &mut input,
        &Message::Json(
            serde_json::from_str(include_str!("../../../spec/vectors/wire/capabilities.json"))
                .unwrap(),
        ),
    )
    .unwrap();
    for (kind, payload) in [
        (Kind::Touch, oxp_wire::encode_touch(&[]).unwrap()),
        (Kind::Key, oxp_wire::encode_key(1, true).unwrap()),
    ] {
        oxp_wire::write(
            &mut input,
            &Message::Binary {
                header: Header {
                    kind,
                    stream: 0,
                    flags: 0,
                    sequence: 0,
                    pts_us: 0,
                    sender_clock_us: 0,
                },
                payload,
            },
        )
        .unwrap();
    }
    assert!(
        matches!(oxp_wire::read(&mut output).unwrap(),Some(Message::Binary{header,..}) if header.flags==1 && header.pts_us==100000)
    );
    drop(input);
    // The producer sees EOF during the next paced interval.
    let status = child.wait().unwrap();
    assert!(status.success());
    let mut logs = String::new();
    std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut logs).unwrap();
    let types: Vec<String> = logs
        .lines()
        .map(|line| {
            serde_json::from_str::<serde_json::Value>(line).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(types, ["capabilities", "touch", "key"]);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn wire_consumer_drives_common_sink_and_hub() {
    use oxp_core::{model::*, Hub, InputSink, ProjectionOutput};
    use std::sync::Mutex;
    #[derive(Default)]
    struct Capture {
        host: Mutex<Option<Arc<dyn OutputHost>>>,
        events: Mutex<Vec<String>>,
    }
    impl ProjectionOutput for Capture {
        fn id(&self) -> &str {
            "capture"
        }
        fn open(&self, h: Arc<dyn OutputHost>) -> oxp_core::Result<()> {
            *self.host.lock().unwrap() = Some(h);
            Ok(())
        }
        fn on_session(&self, s: &SessionState, _: &SessionConfig) {
            self.events
                .lock()
                .unwrap()
                .push(format!("session:{}", s.id));
        }
        fn on_video_format(&self, _: u32, _: Option<Arc<VideoFormat>>) {
            self.events.lock().unwrap().push("format".into());
        }
        fn on_video_frame(&self, _: VideoFrame) {
            self.events.lock().unwrap().push("frame".into());
        }
        fn close(&self) {}
    }
    #[derive(Default)]
    struct Upstream(Mutex<Vec<ControlEvent>>);
    impl OutputHost for Upstream {
        fn capabilities(&self, _: OutputCapabilities) {}
        fn stats(&self, _: OutputStats) {}
        fn control(&self, e: ControlEvent) {
            self.0.lock().unwrap().push(e);
        }
    }
    let upstream = Arc::new(Upstream::default());
    let hub = Hub::new(upstream.clone());
    let output = Arc::new(Capture::default());
    hub.add_output(output.clone()).unwrap();
    let mut consumer = Consumer::default();
    consumer
        .receive(
            Message::Json(json!({"type":"session","session":{"id":"42","phase":"ACTIVE"}})),
            hub.as_ref(),
        )
        .unwrap();
    let format = VideoFormat::create(0, VideoCodec::H264, avcc().into(), None).unwrap();
    consumer
        .receive(video_format(&format), hub.as_ref())
        .unwrap();
    consumer
        .receive(
            Message::Binary {
                header: Header {
                    kind: Kind::Video,
                    stream: 0,
                    flags: 1,
                    sequence: 0,
                    pts_us: 0,
                    sender_clock_us: 0,
                },
                payload: vec![0, 0, 0, 2, 0x65, 0x80],
            },
            hub.as_ref(),
        )
        .unwrap();
    assert_eq!(
        *output.events.lock().unwrap(),
        ["session:42", "format", "frame"]
    );
    let event = ControlEvent::Key {
        code: KeyCode::Home,
        down: true,
        origin: Origin {
            output_id: "capture".into(),
            client_id: "receiver".into(),
        },
    };
    output
        .host
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .control(event.clone());
    assert_eq!(*upstream.0.lock().unwrap(), [event]);
    hub.input_capabilities(InputCapabilities {
        touch: false,
        ..Default::default()
    });
    output
        .host
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .control(ControlEvent::Touch {
            screen: 0,
            contacts: Vec::new(),
            origin: Origin {
                output_id: "capture".into(),
                client_id: "receiver".into(),
            },
        });
    assert_eq!(upstream.0.lock().unwrap().len(), 1);
    hub.close();
}
#[test]
fn relay_help_and_rejected_options_are_json_logged() {
    use std::process::Command;
    let exe = env!("CARGO_BIN_EXE_oxp-relay");
    let help = Command::new(exe).arg("--help").output().unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8(help.stdout)
        .unwrap()
        .contains("--tls-self-signed"));
    for args in [
        vec!["--stdio"],
        vec![
            "--stdio",
            "--web-port",
            "0",
            "--web-root",
            ".",
            "--no-tls",
            "--tls-self-signed",
            "state",
        ],
        vec![
            "--stdio",
            "--web-port",
            "0",
            "--web-root",
            ".",
            "--tls-cert",
            "cert",
        ],
        vec![
            "--stdio",
            "--web-port",
            "0",
            "--web-root",
            ".",
            "--no-tls",
            "--tls-san",
            "extra",
        ],
        vec!["--tcp", "1234"],
        vec!["--stdio", "--unix", "path"],
        vec!["--stdio", "--clink-head-unit", "127.0.0.1"],
    ] {
        let out = Command::new(exe).args(args).output().unwrap();
        assert!(!out.status.success());
        for line in String::from_utf8(out.stderr).unwrap().lines() {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(v["type"], "error");
        }
    }
}

#[test]
fn consumer_audio_session_and_format_fences() {
    use oxp_core::{model::*, InputSink};
    use std::sync::Mutex;
    #[derive(Default)]
    struct Capture(Mutex<Vec<AudioPacket>>);
    impl InputSink for Capture {
        fn session(&self, _: &SessionState) {}
        fn video_format(&self, _: u32, _: Option<Arc<VideoFormat>>) {}
        fn video_frame(&self, _: VideoFrame) {}
        fn audio_format(&self, _: u32, _: Option<Arc<AudioFormat>>) {}
        fn audio_packet(&self, p: AudioPacket) {
            self.0.lock().unwrap().push(p);
        }
        fn microphone(&self, _: u32, _: Option<MicRequest>) {}
    }
    let capture = Capture::default();
    let mut consumer = Consumer::default();
    let packet = || Message::Binary {
        header: Header {
            kind: Kind::Audio,
            stream: 1,
            flags: 0,
            sequence: 0,
            pts_us: 123,
            sender_clock_us: 0,
        },
        payload: vec![1, 2],
    };
    assert!(consumer.receive(packet(), &capture).is_err());
    let session = |id, phase| {
        Message::Json(json!({"type":"session","session":{"id":format!("{id}"),"phase":phase}}))
    };
    consumer.receive(session(1, "ACTIVE"), &capture).unwrap();
    assert!(consumer.receive(packet(), &capture).is_err());
    let format = || {
        Message::audio_format(
            1,
            Some(&oxp_wire::AudioFormat {
                role: oxp_wire::AudioRole::Voice,
                codec: oxp_wire::AudioCodec::PcmS16Le,
                sample_rate: 16000,
                channels: 1,
                config: None,
            }),
        )
        .unwrap()
    };
    consumer.receive(format(), &capture).unwrap();
    consumer.receive(packet(), &capture).unwrap();
    consumer
        .receive(Message::audio_format(1, None).unwrap(), &capture)
        .unwrap();
    assert_eq!(capture.0.lock().unwrap().len(), 1);
    assert_eq!(capture.0.lock().unwrap()[0].session, 1);
    assert_eq!(capture.0.lock().unwrap()[0].data.as_ref(), &[1, 2]);
    consumer.receive(format(), &capture).unwrap();
    consumer.receive(session(0, "ACTIVE"), &capture).unwrap();
    consumer.receive(packet(), &capture).unwrap();
    consumer.receive(session(2, "ACTIVE"), &capture).unwrap();
    assert!(consumer.receive(packet(), &capture).is_err());
    consumer.receive(format(), &capture).unwrap();
    consumer
        .receive(Message::audio_format(1, None).unwrap(), &capture)
        .unwrap();
    assert!(consumer.receive(packet(), &capture).is_err());
    consumer.receive(session(2, "ENDED"), &capture).unwrap();
    assert!(consumer.receive(packet(), &capture).is_err());
}

#[test]
fn relay_events_use_stdout_for_unix_and_preserve_stdio_wire_transport() {
    use std::process::Command;
    let exe = env!("CARGO_BIN_EXE_oxp-relay");
    for transport in [vec!["--unix", "/unused"], vec!["--stdio"]] {
        let stdio = transport[0] == "--stdio";
        let out = Command::new(exe)
            .args(transport)
            .args(["--clink-link", "invalid"])
            .output()
            .unwrap();
        assert!(!out.status.success());
        let (events, other) = if stdio {
            (out.stderr, out.stdout)
        } else {
            (out.stdout, out.stderr)
        };
        assert!(other.is_empty());
        let value: serde_json::Value = serde_json::from_slice(&events).unwrap();
        assert_eq!(
            value,
            json!({"type":"error","message":"--clink-link must be static or ble"})
        );
    }
}

#[test]
fn carlink_repin_diagnostic_auto_confirmation_and_incorrect_pin_reach_stdout() {
    use oxp_clink::protocol::{AuthIdentity, FieldValue, Fields, Frame};
    use std::{
        collections::BTreeMap,
        io::Read,
        net::{TcpListener, TcpStream},
        process::{Child, Command, Stdio},
        time::{Duration, Instant},
    };
    struct Process(Child);
    impl Drop for Process {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    fn read_frame(socket: &mut TcpStream) -> Frame {
        let mut prefix = [0; 4];
        socket.read_exact(&mut prefix).unwrap();
        let mut bytes = vec![0; u32::from_be_bytes(prefix) as usize];
        bytes[..4].copy_from_slice(&prefix);
        socket.read_exact(&mut bytes[4..]).unwrap();
        Frame::decode(&bytes).unwrap()
    }
    fn reply(socket: &mut TcpStream, request: &Frame, fields: BTreeMap<u32, FieldValue>) {
        socket
            .write_all(
                &Frame {
                    source: 0,
                    kind: 2,
                    method: 2,
                    body: Fields(fields).encode().unwrap(),
                    ..request.clone()
                }
                .encode()
                .unwrap(),
            )
            .unwrap();
    }
    let dir = std::env::temp_dir().join(format!("oxp-clink-events-{}", std::process::id()));
    let store = FileTrust::new(&dir).unwrap();
    store
        .save_peer_key("127.0.0.1", &AuthIdentity::generate().public_key())
        .unwrap();
    let auth = TcpListener::bind("127.0.0.1:0").unwrap();
    auth.set_nonblocking(true).unwrap();
    let services = ["control", "media", "cert", "rtsp", "uibc", "rtp"];
    let reserved: Vec<_> = services
        .iter()
        .map(|_| TcpListener::bind("127.0.0.1:0").unwrap())
        .collect();
    let mut ports = format!("auth={}", auth.local_addr().unwrap().port());
    for (service, listener) in services.iter().zip(&reserved) {
        ports.push_str(&format!(
            ",{service}={}",
            listener.local_addr().unwrap().port()
        ));
    }
    let path = dir.join("wire.sock");
    let mut child = Process(
        Command::new(env!("CARGO_BIN_EXE_oxp-relay"))
            .args([
                "--unix",
                path.to_str().unwrap(),
                "--clink-head-unit",
                "127.0.0.1",
                "--clink-local",
                "127.0.0.1",
                "--state-dir",
                dir.to_str().unwrap(),
                "--pin",
                "123456",
                "--clink-transcode",
                "off",
                "--port",
                &ports,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut wire = loop {
        if let Ok(wire) = UnixStream::connect(&path) {
            break wire;
        }
        assert!(Instant::now() < deadline, "relay wire listener deadline");
        std::thread::sleep(Duration::from_millis(5));
    };
    wire.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    assert!(
        matches!(oxp_wire::read(&mut wire).unwrap(), Some(Message::Json(v)) if v["type"] == "hello")
    );
    drop(reserved);
    oxp_wire::write(
        &mut wire,
        &Message::hello("producer", "test", "default").unwrap(),
    )
    .unwrap();
    let mut socket = loop {
        if let Ok((socket, _)) = auth.accept() {
            break socket;
        }
        assert!(Instant::now() < deadline, "relay AUTH deadline");
        std::thread::sleep(Duration::from_millis(5));
    };
    socket.set_nonblocking(false).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let request = read_frame(&mut socket);
    assert_eq!(request.sequence, 1);
    assert!(!Fields::decode(&request.body).unwrap().0.contains_key(&2));
    // A quick response whose signature no longer matches the saved identity triggers repinning.
    reply(
        &mut socket,
        &request,
        BTreeMap::from([
            (1, FieldValue::Varint(1)),
            (4, FieldValue::Bytes(vec![0; 64])),
            (5, FieldValue::Bytes(vec![1])),
            (6, FieldValue::Bytes(vec![17; 32])),
            (7, FieldValue::Varint(0)),
        ]),
    );
    let request = read_frame(&mut socket);
    assert_eq!(request.sequence, 3);
    assert!(Fields::decode(&request.body).unwrap().0.contains_key(&2));
    reply(
        &mut socket,
        &request,
        BTreeMap::from([(1, FieldValue::Varint(1)), (7, FieldValue::Varint(2))]),
    );
    let request = read_frame(&mut socket);
    assert_eq!(request.sequence, 5);
    assert_eq!(Fields::decode(&request.body).unwrap().number(9).unwrap(), 1);
    reply(
        &mut socket,
        &request,
        BTreeMap::from([(1, FieldValue::Varint(1)), (7, FieldValue::Varint(1))]),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "relay shutdown deadline");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(!status.success());
    let mut stdout = String::new();
    child
        .0
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    let mut stderr = String::new();
    child
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(stderr.is_empty(), "{stderr}");
    let events: Vec<serde_json::Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let diagnostic =
        json!({"type":"diagnostic","message":"auth repin reason=auth_signature_failed"});
    assert_eq!(events.iter().filter(|e| **e == diagnostic).count(), 1);
    assert!(events.contains(&json!({"type":"clink-pin","result":"incorrect","client":"127.0.0.1"})));
    assert!(!stdout.contains("123456"));
    assert!(store.load_peer_key("127.0.0.1").unwrap().is_none());
    std::fs::remove_dir_all(dir).unwrap();
}
