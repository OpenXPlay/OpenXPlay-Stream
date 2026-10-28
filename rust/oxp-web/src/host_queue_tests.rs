// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

use super::*;
use std::sync::mpsc::channel;

#[derive(Default)]
struct Host {
    controls: Mutex<Vec<ControlEvent>>,
    stats: Mutex<Vec<OutputStats>>,
}
impl OutputHost for Host {
    fn capabilities(&self, _: OutputCapabilities) {}
    fn control(&self, event: ControlEvent) {
        self.controls.lock().unwrap().push(event);
    }
    fn stats(&self, stats: OutputStats) {
        self.stats.lock().unwrap().push(stats);
    }
}

#[test]
fn a_blocked_host_does_not_stall_socket_io_and_reports_keep_the_latest_value() {
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let blocked = gate.clone();
    let (started, ready) = channel();
    let reports = Arc::new(Mutex::new(Vec::new()));
    let delivered = reports.clone();
    let mut config = WebOutputConfig::new(
        "127.0.0.1:0".parse().unwrap(),
        PathBuf::new(),
        Pairing::Disabled,
        Tls::Disabled,
    );
    config.diagnostics = Some(Arc::new(move |_, report| {
        if report["type"] == "stats" {
            let _ = started.send(());
            let (lock, wake) = &*blocked;
            let (open, _) = wake
                .wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(10), |open| !*open)
                .unwrap();
            assert!(*open, "host was never released");
            delivered.lock().unwrap().push(report.clone());
        }
    }));
    let web = WebOutput::new(config).unwrap();
    let host = Arc::new(Host::default());
    web.open(host.clone()).unwrap();
    let address = web.await_listening(Duration::from_secs(5)).unwrap();
    let tcp = TcpStream::connect(address).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    tcp.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut ws = tungstenite::client(format!("ws://{address}/oxp/v0"), tcp)
        .unwrap()
        .0;
    let send = |ws: &mut WebSocket<TcpStream>, value: Value| {
        ws.send(Message::Text(value.to_string().into())).unwrap()
    };
    send(
        &mut ws,
        serde_json::from_str(include_str!(
            "../../web-receiver/spec/vectors/web/hello.json"
        ))
        .unwrap(),
    );
    assert!(matches!(ws.read().unwrap(), Message::Text(_)));
    let mut stats: Value = serde_json::from_str(include_str!(
        "../../web-receiver/spec/vectors/web/stats.json"
    ))
    .unwrap();
    send(&mut ws, stats.clone());
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    for i in 0..2000 {
        stats["rendered"] = json!(i);
        send(&mut ws, stats.clone());
        if i % 100 == 0 {
            send(&mut ws, json!({"type":"keyframe","screen":0}));
        }
    }
    send(&mut ws, json!({"type":"ping","t0Us":"123"}));
    let Message::Text(reply) = ws.read().unwrap() else {
        panic!("Expected pong while host is blocked")
    };
    assert_eq!(
        serde_json::from_str::<Value>(&reply).unwrap()["type"],
        "pong"
    );
    let queue = web.host_queue.lock().unwrap().clone().unwrap();
    assert!(queue.size() <= 24, "{} callbacks waiting", queue.size());
    assert!(host.controls.lock().unwrap().is_empty());
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    queue.await_idle();
    assert_eq!(host.controls.lock().unwrap().len(), 20);
    let reports = reports.lock().unwrap();
    assert!(reports.len() <= 2, "{} diagnostics", reports.len());
    assert_eq!(reports.last().unwrap()["rendered"], 1999);
    assert_eq!(host.stats.lock().unwrap().last().unwrap().rendered, 1999);
    web.close();
}
