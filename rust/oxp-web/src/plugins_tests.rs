// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

use super::*;
use crate::{Pairing, Tls, WebOutput, WebOutputConfig};
use oxp_core::ProjectionOutput;
fn vectors(name: &str) -> Vec<Value> {
    serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../spec/vectors/plugins")
                .join(format!("{name}.json")),
        )
        .unwrap(),
    )
    .unwrap()
}
fn fixture() -> Value {
    vectors("schema")
        .into_iter()
        .find(|v| v["name"] == "complete manifest")
        .unwrap()["value"]
        .clone()
}
fn plugins() -> Plugins {
    Plugins {
        config: PluginsConfig {
            root: PathBuf::from("/does-not-exist"),
            enabled: vec![],
            state_dir: None,
            ..PluginsConfig::default()
        },
        manifests: vec![fixture()],
        data: Mutex::new(Data {
            values: BTreeMap::new(),
            writes: BTreeMap::new(),
            pending: BTreeSet::new(),
        }),
        latest: Mutex::new((
            0,
            TOPICS
                .into_iter()
                .map(|t| (t.into(), (0, Value::Null)))
                .collect(),
        )),
        connections: Mutex::new(VecDeque::new()),
        receivers: AtomicUsize::new(0),
    }
}
#[test]
fn copied_vectors() {
    for v in vectors("paths") {
        assert_eq!(safe_path(v["path"].as_str().unwrap()), v["allowed"], "{v}");
    }
    for v in vectors("csp") {
        let actual = headers(v["origin"].as_str().unwrap(), v["plugin"].as_str());
        for (key, value) in v["headers"].as_object().unwrap() {
            assert_eq!(&actual[key], value.as_str().unwrap());
        }
        assert_eq!(actual["Access-Control-Allow-Origin"], "null");
    }
    for v in vectors("schema") {
        let actual = match v["schema"].as_str().unwrap() {
            "manifest" => valid_manifest(&v["value"]),
            "channel-message" => valid_channel(&v["value"]),
            s if s.starts_with("topic-") => valid_topic(&s[6..], &v["value"]),
            _ => continue,
        };
        assert_eq!(actual, v["valid"], "{}", v["name"]);
    }
}
#[test]
fn disabled_has_no_context_or_files_and_uses_ordinary_static_routes() {
    use oxp_core::model::{ControlEvent, OutputCapabilities, OutputHost, OutputStats};
    use std::{io::Read, net::TcpStream, sync::Arc};

    struct Host;
    impl OutputHost for Host {
        fn capabilities(&self, _: OutputCapabilities) {}
        fn control(&self, _: ControlEvent) {}
        fn stats(&self, _: OutputStats) {}
    }

    let root = std::env::temp_dir().join(format!("oxp-plugins-off-{}", std::process::id()));
    assert!(!root.exists());
    let config = WebOutputConfig::new(
        "127.0.0.1:0".parse().unwrap(),
        root.clone(),
        Pairing::Disabled,
        Tls::Disabled,
    );
    assert!(config.plugins.is_none());
    let web = WebOutput::new(config).unwrap();
    assert!(!web.wants_context());
    web.open(Arc::new(Host)).unwrap();
    let address = web.await_listening(Duration::from_secs(5)).unwrap();
    let request = |method: &str, target: &str, upgrade: bool| {
        let mut socket = TcpStream::connect(address).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let headers = if upgrade {
            "Connection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n"
        } else {
            "Connection: close\r\n"
        };
        write!(
            socket,
            "{method} {target} HTTP/1.1\r\nHost: localhost\r\n{headers}\r\n"
        )
        .unwrap();
        let mut response = Vec::new();
        socket.read_to_end(&mut response).unwrap();
        assert!(!response.is_empty());
        response
    };
    for method in ["GET", "HEAD", "POST"] {
        for tail in [
            "index.json",
            "host/index.html",
            "channel",
            "p/org.test.clock/index.html",
            "artwork/x",
        ] {
            assert_eq!(
                request(method, &format!("/zz-missing/{tail}"), false),
                request(method, &format!("/plugins/{tail}"), false),
                "{method} /plugins/{tail}"
            );
        }
    }
    assert_eq!(
        request("GET", "/zz-missing/channel", true),
        request("GET", "/plugins/channel", true),
        "WebSocket upgrade /plugins/channel"
    );
    web.close();
    assert!(!root.exists());
}
#[test]
fn channel_protocol_storage_and_latest_only() {
    let p = plugins();
    let mut c = Channel::default();
    assert_eq!(
        c.receive(&p, "{}"),
        vec![json!({"type":"error","code":"protocol"})]
    );
    assert!(c.receive(&p, r#"{"type":"future"}"#).is_empty());
    assert_eq!(
        c.receive(&p, r#"{"type":"subscribe","topics":[]}"#)[0]["code"],
        "protocol"
    );
    assert_eq!(
        c.receive(
            &p,
            r#"{"type":"hello","spec":"oxp-web-plugin/0","host":"test"}"#
        ),
        vec![p.welcome()]
    );
    assert_eq!(
        c.receive(&p, r#"{"type":"subscribe","topics":["session"]}"#)[0]["value"],
        Value::Null
    );
    p.update(
        ContextUpdate::from_json(&json!({"topic":"session","value":{"carplay":"connected"}}))
            .unwrap(),
    );
    p.update(
        ContextUpdate::from_json(&json!({"topic":"session","value":{"carplay":"disconnected"}}))
            .unwrap(),
    );
    assert!(c.flush(&p, Instant::now()).is_empty());
    let now = Instant::now() + Duration::from_millis(251);
    let sent = c.flush(&p, now);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["value"]["carplay"], "disconnected");
    assert_eq!(sent[0]["seq"], 2);
    let id = fixture()["id"].clone();
    let set = json!({"type":"storage-set","plugin":id,"id":"1","value":{"hello":"world"}});
    assert_eq!(c.receive(&p, &set.to_string())[0]["type"], "storage-ok");
    let mut other = Channel {
        ready: true,
        ..Channel::default()
    };
    assert_eq!(other.receive(&p, &set.to_string())[0]["code"], "rate");
    assert_eq!(
        c.receive(
            &p,
            &json!({"type":"storage-get","plugin":id,"id":"2"}).to_string()
        )[0]["value"],
        set["value"]
    );
    assert_eq!(
        c.receive(
            &p,
            r#"{"type":"storage-get","plugin":"org.unknown","id":"x"}"#
        )[0]["code"],
        "unknown-plugin"
    );
    let permits: Vec<_> = (0..4).map(|_| p.reserve()).collect();
    let newest = p.reserve();
    assert!(permits[0].replaced.load(Ordering::Acquire));
    assert!(!newest.replaced.load(Ordering::Acquire));
    drop(permits);
    assert_eq!(p.connections.lock().unwrap().len(), 1);
}
#[test]
fn static_paths_and_persistence() {
    let root = std::env::temp_dir().join(format!("oxp-plugins-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let id = fixture()["id"].as_str().unwrap().to_owned();
    let package = root.join(format!("p/{id}"));
    std::fs::create_dir_all(&package).unwrap();
    std::fs::create_dir_all(root.join("host")).unwrap();
    std::fs::write(package.join("oxp-plugin.json"), fixture().to_string()).unwrap();
    std::fs::write(root.join("host/index.html"), "hello").unwrap();
    let config = PluginsConfig {
        root: root.clone(),
        enabled: vec![id.clone()],
        state_dir: Some(root.join("state")),
        ..PluginsConfig::default()
    };
    let p = Plugins::new(config.clone()).unwrap();
    assert_eq!(p.manifests.len(), 1);
    let req = |target: &str, method: &str| {
        let mut out = Vec::new();
        serve(
            Some(&p),
            &http::Request {
                method: method.into(),
                target: target.into(),
                headers: BTreeMap::new(),
            },
            "http://localhost",
            &mut out,
        )
        .unwrap();
        String::from_utf8(out).unwrap()
    };
    assert!(req("/plugins/host/index.html", "GET").ends_with("\r\n\r\nhello"));
    assert!(
        req("/plugins/host/index.html", "HEAD").ends_with("Content-Type-Options: nosniff\r\n\r\n")
    );
    for path in [
        "/plugins/host/../index.html",
        "/plugins/host/%2e%2e/index.html",
        "/plugins/p/org.disabled/oxp-plugin.json",
        "/plugins/host/.hidden.js",
    ] {
        assert!(req(path, "GET").starts_with("HTTP/1.1 404"));
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/etc/hosts", root.join("host/escape.txt")).unwrap();
        assert!(req("/plugins/host/escape.txt", "GET").starts_with("HTTP/1.1 404"));
    }
    assert_eq!(
        p.storage(&json!({"type":"storage-set","plugin":id,"id":"x","value":{"saved":1}}))["type"],
        "storage-ok"
    );
    let reopened = Plugins::new(config).unwrap();
    assert_eq!(
        reopened.storage(&json!({"type":"storage-get","plugin":id,"id":"y"}))["value"],
        json!({"saved":1})
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn receiver_hello_off_is_byte_identical_to_golden() {
    let config = WebOutputConfig::new(
        "127.0.0.1:0".parse().unwrap(),
        "/missing".into(),
        Pairing::Disabled,
        Tls::Disabled,
    );
    let web = WebOutput::new(config.clone()).unwrap();
    {
        let mut s = web.state.lock().unwrap();
        s.session = oxp_core::model::SessionState {
            id: 42,
            phase: oxp_core::model::SessionPhase::Active,
            reason: None,
        };
        s.clients.insert(
            "browser-1".into(),
            crate::Client::new(config.queue_duration_us),
        );
    }
    crate::handle(
        serde_json::from_str(include_str!("../../web-receiver/spec/vectors/web/hello.json")).unwrap(),
        "browser-1",
        &config,
        &web.state,
        0,
    )
    .unwrap();
    let actual = web
        .state
        .lock()
        .unwrap()
        .clients
        .get_mut("browser-1")
        .unwrap()
        .messages
        .pop_front()
        .unwrap();
    let golden: Value =
        serde_json::from_str(include_str!("../../web-receiver/spec/vectors/web/welcome.json")).unwrap();
    assert_eq!(actual.to_string().as_bytes(), golden.to_string().as_bytes());
}

#[test]
fn storage_permissions_quota_and_message_cap() {
    let mut p = plugins();
    let id = fixture()["id"].clone();
    p.manifests[0]["permissions"] = json!([]);
    assert_eq!(
        p.storage(&json!({"type":"storage-get","plugin":id,"id":"1"}))["code"],
        "permission"
    );
    p.manifests[0]["permissions"] = json!(["storage"]);
    let mut m = json!({"type":"storage-set","plugin":id,"id":"2","value":{"s":"x".repeat(LIMIT)}});
    assert_eq!(p.storage(&m)["code"], "quota");
    p.data.lock().unwrap().values.insert(
        id.as_str().unwrap().into(),
        json!({"s":"x".repeat(LIMIT-8)}),
    );
    assert_eq!(
        p.storage(&json!({"type":"storage-get","plugin":id,"id":"get"}))["code"],
        "quota"
    );
    m["value"] = json!({});
    p.data
        .lock()
        .unwrap()
        .pending
        .insert(id.as_str().unwrap().into());
    assert_eq!(p.storage(&m)["code"], "rate");
    p.data.lock().unwrap().pending.clear();
    assert_eq!(p.storage(&m)["type"], "storage-ok");
    p.data.lock().unwrap().writes.insert(
        id.as_str().unwrap().into(),
        Instant::now() - Duration::from_secs(2),
    );
    assert_eq!(p.storage(&m)["type"], "storage-ok");
    let mut c = Channel::default();
    assert_eq!(
        c.receive(
            &p,
            &json!({"type":"future","data":"x".repeat(LIMIT)}).to_string()
        )[0]["code"],
        "protocol"
    );
    let _permits: Vec<_> = (0..4).map(|_| p.reserve()).collect();
    let receiver = p.reserve_receiver(1).unwrap();
    assert!(p.reserve_receiver(1).is_none());
    drop(receiver);
    assert!(p.reserve_receiver(1).is_some());
    assert_eq!(
        origin(true, "RELAY.example:443").unwrap(),
        "https://relay.example"
    );
    assert_eq!(origin(false, "[::1]:8080").unwrap(), "http://[::1]:8080");
}

#[test]
fn subscribe_preserves_order_and_uses_catalog_sequence_for_every_snapshot() {
    let p = plugins();
    p.update(
        ContextUpdate::from_json(&json!({"topic":"session","value":{"carplay":"connected"}}))
            .unwrap(),
    );
    let mut c = Channel {
        ready: true,
        ..Channel::default()
    };
    let states = c.receive(
        &p,
        r#"{"type":"subscribe","topics":["session","activity","session"]}"#,
    );
    assert_eq!(
        states
            .iter()
            .map(|m| m["topic"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["session", "activity"]
    );
    assert!(states.iter().all(|m| m["seq"] == 1));
    assert!(c
        .flush(&p, Instant::now() + Duration::from_millis(251))
        .is_empty());
    assert!(c
        .receive(
            &p,
            r#"{"type":"subscribe","topics":["session","activity"]}"#
        )
        .is_empty());
    assert_eq!(
        c.flush(&p, Instant::now() + Duration::from_millis(251))
            .len(),
        2
    );
}

#[test]
fn partial_routes_are_advertised_and_delivered_without_clearing() {
    let p = plugins();
    let mut manifest = fixture();
    manifest["permissions"]
        .as_array_mut()
        .unwrap()
        .push(json!("route"));
    assert!(valid_manifest(&manifest));
    assert!(p.welcome()["topics"]
        .as_array()
        .unwrap()
        .contains(&json!("route")));
    let value = json!({"active":true,"destination":null,"etaMs":null,"distanceRemainingM":null,
        "maneuver":{"type":2,"road":null,"distanceM":null},"appName":null});
    p.update(ContextUpdate::from_json(&json!({"topic":"route","value":value})).unwrap());
    let mut channel = Channel {
        ready: true,
        ..Channel::default()
    };
    let states = channel.receive(&p, r#"{"type":"subscribe","topics":["route"]}"#);
    assert_eq!(states.len(), 1);
    assert_eq!(states[0]["value"], value);
    assert!(valid_channel(&states[0]));
}

#[test]
fn provider_vectors_and_default_topics() {
    let provider: Value = serde_json::from_str(include_str!(
        "../../../spec/vectors/plugins/provider-relay.json"
    ))
    .unwrap();
    let p = Plugins::new(PluginsConfig::default()).unwrap();
    assert_eq!(p.welcome()["topics"], provider["topics"]);
    for v in provider["vectors"].as_array().unwrap() {
        assert_eq!(
            valid_topic(v["topic"].as_str().unwrap(), &v["value"]),
            v["valid"],
            "{}",
            v["name"]
        );
    }
    for v in provider["messages"].as_array().unwrap() {
        assert_eq!(valid_channel(&v["value"]), v["valid"], "{}", v["name"]);
    }
    let mut channel = Channel {
        ready: true,
        ..Channel::default()
    };
    let value = json!({"appName":"Music"});
    p.update(ContextUpdate::from_json(&json!({"topic":"audioSource","value":value})).unwrap());
    assert_eq!(
        channel.receive(&p, r#"{"type":"subscribe","topics":["audioSource"]}"#)[0]["value"],
        value
    );
}

#[test]
fn configured_availability_filters_delivery_and_unknown_subscriptions() {
    for topics in [vec!["session", "activity", "route", "audioSource"], vec![]] {
        let p = Plugins::new(PluginsConfig {
            topics: topics.iter().map(|t| (*t).to_owned()).collect(),
            ..PluginsConfig::default()
        })
        .unwrap();
        let mut c = Channel::default();
        assert_eq!(
            c.receive(
                &p,
                r#"{"type":"hello","spec":"oxp-web-plugin/0","host":"test"}"#
            )[0]["topics"],
            json!(topics)
        );
        assert!(c.receive(&p, r#"{"type":"subscribe","topics":["nowPlaying","call","future","motion","parked","future2","future3"]}"#).is_empty());
        assert_eq!(c.topics, vec!["nowPlaying", "call"]);
        p.update(
            ContextUpdate::from_json(&json!({"topic":"call","value":{"state":"active"}})).unwrap(),
        );
        assert!(c
            .flush(&p, Instant::now() + Duration::from_secs(1))
            .is_empty());
        assert!(c
            .receive(&p, r#"{"type":"subscribe","topics":["future"]}"#)
            .is_empty());
        assert!(c.topics.is_empty());
        let states = c.receive(&p, r#"{"type":"subscribe","topics":["future","session"]}"#);
        assert_eq!(states.len(), usize::from(!topics.is_empty()));
        if let Some(state) = states.first() {
            assert_eq!(state["topic"], "session");
        }
    }
}

fn socket_channel(
    p: Arc<Plugins>,
    timing: ChannelTiming,
) -> (WebSocket<std::net::TcpStream>, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let (socket, _) = listener.accept().unwrap();
    let (ready, wait) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let run = Runtime {
            active: AtomicBool::new(true),
            bound: Mutex::new(None),
            listening: std::sync::Condvar::new(),
            connections: AtomicUsize::new(0),
        };
        let permit = p.reserve();
        ready.send(()).unwrap();
        let _ = p.channel_with_timing(
            Transport::Plain(crate::SocketIo {
                socket,
                deadline: None,
            }),
            &run,
            &permit,
            timing,
        );
    });
    wait.recv().unwrap();
    (
        WebSocket::from_raw_socket(client, Role::Client, None),
        worker,
    )
}
fn fast_timing() -> ChannelTiming {
    ChannelTiming {
        ping: Duration::from_millis(40),
        idle: Duration::from_millis(200),
        write: Duration::from_millis(100),
    }
}
fn channel_hello(ws: &mut WebSocket<std::net::TcpStream>) {
    ws.send(Message::text(
        r#"{"type":"hello","spec":"oxp-web-plugin/0","host":"test"}"#,
    ))
    .unwrap();
    loop {
        match ws.read().unwrap() {
            Message::Text(raw) => {
                assert_eq!(
                    serde_json::from_str::<Value>(&raw).unwrap()["type"],
                    "welcome"
                );
                return;
            }
            Message::Ping(_) => ws.flush().unwrap(),
            other => panic!("unexpected {other:?}"),
        }
    }
}
#[test]
fn silent_channel_times_out_and_releases_slot() {
    let p = Arc::new(plugins());
    let (mut ws, worker) = socket_channel(p.clone(), fast_timing());
    // Do not read: tungstenite would automatically answer pings on the next read.
    std::thread::sleep(Duration::from_millis(400));
    worker.join().unwrap();
    assert!(p.connections.lock().unwrap().is_empty());
    let mut input = Vec::new();
    std::io::Read::read_to_end(ws.get_mut(), &mut input).unwrap();
    assert!(input.windows(2).any(|b| b == [0x89, 0]));
}
#[test]
fn responsive_channel_survives_idle_timeout() {
    let p = Arc::new(plugins());
    let (mut ws, worker) = socket_channel(p, fast_timing());
    channel_hello(&mut ws);
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(600) {
        assert!(matches!(ws.read().unwrap(), Message::Ping(_)));
        ws.flush().unwrap();
    }
    ws.close(None).unwrap();
    worker.join().unwrap();
}
#[test]
fn fifth_channel_replaces_oldest_and_gets_welcome() {
    let p = Arc::new(plugins());
    let timing = ChannelTiming {
        ping: Duration::from_secs(10),
        idle: Duration::from_secs(30),
        ..fast_timing()
    };
    let mut channels: Vec<_> = (0..4)
        .map(|_| {
            let (mut ws, worker) = socket_channel(p.clone(), timing);
            channel_hello(&mut ws);
            (ws, worker)
        })
        .collect();
    let (mut newest, worker) = socket_channel(p.clone(), timing);
    channel_hello(&mut newest);
    match channels[0].0.read().unwrap() {
        Message::Close(Some(frame)) => {
            assert_eq!(
                frame.code,
                tungstenite::protocol::frame::coding::CloseCode::Away
            );
            assert_eq!(frame.reason, "replaced");
        }
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(p.connections.lock().unwrap().len(), 4);
    newest.close(None).unwrap();
    worker.join().unwrap();
    for (mut ws, worker) in channels {
        let _ = ws.close(None);
        worker.join().unwrap();
    }
    assert!(p.connections.lock().unwrap().is_empty());
}

#[test]
fn unavailable_channel_is_503_instead_of_static_404() {
    let p = plugins();
    let mut output = Vec::new();
    let request = http::Request {
        method: "HEAD".into(),
        target: "/plugins/channel".into(),
        headers: BTreeMap::new(),
    };
    serve(Some(&p), &request, "http://localhost", &mut output).unwrap();
    assert!(output.starts_with(b"HTTP/1.1 503 Service Unavailable\r\n"));
}

#[test]
fn every_complete_frame_including_continuations_signals_liveness() {
    let mut activity = FrameActivity::new();
    let old = Instant::now() - Duration::from_secs(1);
    for opcode in [0x01, 0x00, 0x89, 0x8a, 0x82] {
        activity.received = old;
        activity.observe(&[opcode, 0x81, 0, 0, 0, 0]);
        assert_eq!(activity.received, old);
        activity.observe(&[42]);
        assert!(activity.received > old);
    }
    for extended in [
        vec![0x82, 0xfe, 0, 126],
        vec![0x82, 0xff, 0, 0, 0, 0, 0, 1, 0, 0],
    ] {
        activity.received = old;
        let length = if extended[1] == 0xfe { 126 } else { 65536 };
        for byte in extended {
            activity.observe(&[byte]);
        }
        activity.observe(&[0, 0, 0, 0]);
        activity.observe(&vec![0; length - 1]);
        assert_eq!(activity.received, old);
        activity.observe(&[0]);
        assert!(activity.received > old);
    }
    activity.received = old;
    activity.observe(&[0x8a, 0x80, 0, 0, 0, 0]);
    assert!(activity.received > old);
}
