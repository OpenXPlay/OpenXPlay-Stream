// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

use super::*;
use std::sync::mpsc::{channel, Receiver};

struct Host;
impl OutputHost for Host {
    fn capabilities(&self, _: OutputCapabilities) {}
    fn control(&self, _: ControlEvent) {}
    fn stats(&self, _: OutputStats) {}
}

fn observation(rx: &Receiver<(String, Value)>, kind: &str) -> (String, Value) {
    loop {
        let item = rx.recv_timeout(Duration::from_secs(8)).unwrap();
        if item.1["type"] == kind {
            return item;
        }
    }
}

#[test]
fn socket_close_reset_policy_and_hello_timeout_notify_once_with_the_diagnostic_id() {
    let (tx, rx) = channel::<(String, Value)>();
    let mut config = WebOutputConfig::new(
        "127.0.0.1:0".parse().unwrap(),
        PathBuf::new(),
        Pairing::Disabled,
        Tls::Disabled,
    );
    config.diagnostics = Some(Arc::new(move |id, v| {
        tx.send((id.to_owned(), v.clone())).unwrap();
    }));
    let web = WebOutput::new(config).unwrap();
    web.open(Arc::new(Host)).unwrap();
    let address = web.await_listening(Duration::from_secs(5)).unwrap();
    for (scenario, code, reason) in [
        ("close", 1000, "closed"),
        ("empty", 1005, "closed"),
        ("reset", 1006, "error"),
        ("policy", 1008, "closed"),
        ("timeout", 1008, "timeout"),
    ] {
        let tcp = TcpStream::connect(address).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(8))).unwrap();
        let mut ws = tungstenite::client(format!("ws://{address}/oxp/v0"), tcp)
            .unwrap()
            .0;
        let mut expected_id = None;
        if matches!(scenario, "close" | "empty" | "reset") {
            ws.send(Message::Text(
                include_str!("../../web-receiver/spec/vectors/web/hello.json").into(),
            ))
            .unwrap();
            assert!(matches!(ws.read().unwrap(), Message::Text(_)));
            ws.send(Message::Text(
                include_str!("../../web-receiver/spec/vectors/web/receiver-info.json").into(),
            ))
            .unwrap();
            expected_id = Some(observation(&rx, "receiver-info").0);
        }
        match scenario {
            "close" => {
                ws.close(Some(CloseFrame {
                    code: CloseCode::Normal,
                    reason: "private peer text".into(),
                }))
                .unwrap();
            }
            "empty" => {
                ws.close(None).unwrap();
            }
            "reset" => {
                ws.get_mut().shutdown(Shutdown::Both).unwrap();
            }
            "policy" => {
                ws.send(Message::Text("{\"type\":\"ping\",\"t0Us\":\"0\"}".into()))
                    .unwrap();
            }
            _ => {}
        }
        let (id, value) = observation(&rx, "receiver-disconnected");
        assert_eq!(
            value,
            json!({"type":"receiver-disconnected","code":code,"reason":reason}),
            "{scenario}"
        );
        if let Some(expected_id) = expected_id {
            assert_eq!(id, expected_id);
        }
        assert!(web.state.lock().unwrap().clients.is_empty());
    }
    web.host_queue
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .await_idle();
    assert!(rx
        .try_iter()
        .all(|(_, v)| v["type"] != "receiver-disconnected"));
    web.close();
}

#[test]
fn close_observations_survive_a_full_host_queue_and_never_trust_peer_json() {
    let queue = HostQueue::new(1).unwrap();
    let (release, blocked) = channel();
    let (started, ready) = channel();
    queue.essential(move || {
        started.send(()).unwrap();
        blocked.recv_timeout(Duration::from_secs(5)).unwrap();
    });
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    let (tx, rx) = channel();
    let diagnostics = queue.diagnostics(Arc::new(move |id, v| {
        tx.send((id.to_owned(), v.clone())).unwrap();
    }));
    diagnostics("browser-1", &json!({"type":"stats"}));
    diagnostics("browser-1", &Disconnect::closed(1000).json());
    release.send(()).unwrap();
    queue.await_idle();
    assert_eq!(
        rx.try_iter().collect::<Vec<_>>(),
        [("browser-1".into(), Disconnect::closed(1000).json())]
    );
    queue.shutdown();
    assert!(json::validate(
        &Disconnect::closed(1000).json(),
        Some(json::Direction::Client)
    )
    .is_err());
    assert_eq!(
        Disconnect::error(&io::Error::from(io::ErrorKind::TimedOut)).json(),
        json!({"type":"receiver-disconnected","code":1006,"reason":"timeout"})
    );
}

#[test]
fn stalled_writer_timeout_tracks_byte_progress_without_an_idle_disconnect() {
    let now = Instant::now();
    let mut progress = WriteProgress::default();
    assert!(!progress.timed_out(now + Duration::from_secs(60)));
    progress.attempted(now, &Err(io::ErrorKind::WouldBlock.into()), 10);
    assert!(!progress.timed_out(now + Duration::from_millis(4999)));
    assert!(progress.timed_out(now + Duration::from_secs(5)));
    progress.attempted(now + Duration::from_secs(4), &Ok(1), 10);
    assert!(!progress.timed_out(now + Duration::from_secs(60)));
    progress.attempted(
        now - Duration::from_secs(5),
        &Err(io::ErrorKind::WouldBlock.into()),
        10,
    );
    let config = WebOutputConfig::new(
        "127.0.0.1:0".parse().unwrap(),
        PathBuf::new(),
        Pairing::Disabled,
        Tls::Disabled,
    );
    let web = WebOutput::new(config.clone()).unwrap();
    let mut client = Client::new(config.queue_duration_us);
    client.write_progress = Some(Arc::new(Mutex::new(progress)));
    web.state
        .lock()
        .unwrap()
        .clients
        .insert("stalled".into(), client);
    let run = Runtime {
        active: AtomicBool::new(true),
        bound: Mutex::new(None),
        listening: Condvar::new(),
        connections: AtomicUsize::new(0),
    };
    let mut socket =
        WebSocket::from_raw_socket(std::io::Cursor::new(Vec::<u8>::new()), Role::Server, None);
    let disconnected = client_loop(
        &mut socket,
        "stalled",
        &config,
        &web.state,
        &run,
        &Mutex::new(HelloRateLimit::default()),
        "127.0.0.1".parse().unwrap(),
    )
    .unwrap();
    assert_eq!(
        disconnected.json(),
        json!({"type":"receiver-disconnected","code":1011,"reason":"timeout"})
    );
}
