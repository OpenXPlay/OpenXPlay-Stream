// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

use oxp_clink::{
    model::*,
    protocol::*,
    session::{SessionCallbacks, SessionOptions},
    *,
};
use p256::{
    ecdsa::{signature::Signer, Signature, SigningKey},
    elliptic_curve::sec1::ToEncodedPoint,
    SecretKey,
};
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    thread,
    time::Duration,
};

#[derive(Default)]
struct Pipe {
    state: Mutex<(VecDeque<Vec<u8>>, bool)>,
    ready: Condvar,
}
struct MemoryStream {
    input: Arc<Pipe>,
    output: Arc<Pipe>,
    timeout: Mutex<Option<Duration>>,
}
impl MemoryStream {
    fn pair() -> (Arc<Self>, Arc<Self>) {
        let a = Arc::new(Pipe::default());
        let b = Arc::new(Pipe::default());
        (
            Arc::new(Self {
                input: a.clone(),
                output: b.clone(),
                timeout: Mutex::new(None),
            }),
            Arc::new(Self {
                input: b,
                output: a,
                timeout: Mutex::new(None),
            }),
        )
    }
    fn message(&self) -> Vec<u8> {
        let state = self.input.state.lock().unwrap();
        let (mut state, _) = self
            .input
            .ready
            .wait_timeout_while(state, Duration::from_secs(3), |s| s.0.is_empty() && !s.1)
            .unwrap();
        state.0.pop_front().expect("peer message deadline")
    }
}
impl ClinkStream for MemoryStream {
    fn read(&self, buffer: &mut [u8]) -> io::Result<usize> {
        let state = self.input.state.lock().unwrap();
        let timeout = self
            .timeout
            .lock()
            .unwrap()
            .unwrap_or(Duration::from_secs(3));
        let (mut state, _) = self
            .input
            .ready
            .wait_timeout_while(state, timeout, |s| s.0.is_empty() && !s.1)
            .unwrap();
        if let Some(message) = state.0.front_mut() {
            // Fragment every frame to exercise stream decoding independently of message boundaries.
            let n = buffer.len().min(message.len()).min(7);
            buffer[..n].copy_from_slice(&message[..n]);
            message.drain(..n);
            if message.is_empty() {
                state.0.pop_front();
            }
            Ok(n)
        } else if state.1 {
            Ok(0)
        } else {
            Err(io::ErrorKind::TimedOut.into())
        }
    }
    fn write_all(&self, message: &[u8]) -> io::Result<()> {
        let mut state = self.output.state.lock().unwrap();
        if state.1 {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        state.0.push_back(message.to_vec());
        self.output.ready.notify_all();
        Ok(())
    }
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        *self.timeout.lock().unwrap() = timeout;
        Ok(())
    }
    fn shutdown(&self) -> io::Result<()> {
        for pipe in [&self.input, &self.output] {
            pipe.state.lock().unwrap().1 = true;
            pipe.ready.notify_all();
        }
        Ok(())
    }
}
struct MemoryTransport {
    streams: Mutex<BTreeMap<u16, Arc<MemoryStream>>>,
    offers: Mutex<Vec<u16>>,
    authenticated: AtomicBool,
}
impl ClinkTransport for MemoryTransport {
    fn offer(&self, service: u16) -> Result<()> {
        self.offers.lock().unwrap().push(service);
        Ok(())
    }
    fn open(&self, service: u16, _: Duration) -> Result<Arc<dyn ClinkStream>> {
        Ok(self
            .streams
            .lock()
            .unwrap()
            .remove(&service)
            .ok_or("unknown_service")?)
    }
    fn authenticated(&self) -> Result<()> {
        self.authenticated.store(true, Ordering::Release);
        Ok(())
    }
}
struct Link(Arc<MemoryTransport>, AtomicBool, Option<String>, bool);
impl ClinkLink for Link {
    fn connect(&self, _: Arc<dyn Fn(Option<String>) + Send + Sync>) -> Result<ClinkConnection> {
        Ok(ClinkConnection {
            head_unit_id: "memory-car".into(),
            local_ipv4: [127, 0, 0, 1].into(),
            transport: self.0.clone(),
            wired_pin: self.2.clone(),
            auto_confirm: self.3,
        })
    }
    fn disconnect(&self) -> Result<()> {
        self.1.store(true, Ordering::Release);
        Ok(())
    }
}
struct Pin;
impl PinProvider for Pin {
    fn request_pin(&self, _: &str) -> Option<String> {
        Some("123456".into())
    }
}
struct Trust;
impl TrustStore for Trust {
    fn load_identity(&self) -> Result<Option<AuthIdentity>> {
        Ok(Some(AuthIdentity::generate()))
    }
    fn save_identity(&self, _: &AuthIdentity) -> Result<()> {
        Ok(())
    }
    fn load_peer_key(&self, _: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn save_peer_key(&self, _: &str, _: &[u8]) -> Result<()> {
        Ok(())
    }
    fn forget_peer(&self, _: &str) -> Result<()> {
        Ok(())
    }
}
struct Callbacks;
impl SessionCallbacks for Callbacks {
    fn capabilities(&self, _: OutputCapabilities) {}
    fn control(&self, _: ControlEvent) {}
    fn play(&self, _: bool) {}
    fn keyframe(&self) {}
}
fn auth_response(request: &Frame, identity: &SecretKey, pin: &str) -> (Frame, [u8; 16]) {
    let fields = Fields::decode(&request.body).unwrap();
    let agreement = SecretKey::random(&mut OsRng);
    let public = identity.public_key().to_encoded_point(false).as_bytes()[1..].to_vec();
    let ephemeral = agreement.public_key().to_encoded_point(false).as_bytes()[1..].to_vec();
    let nonce = [17; 32];
    let phone_nonce = fields.bytes(6).unwrap();
    let phone_public = fields.bytes(2).unwrap_or(&[]);
    let phone_agreement = fields.bytes(4).unwrap();
    let proof_key = Sha256::digest([nonce.as_slice(), phone_nonce, pin.as_bytes()].concat());
    let proof = hmac(&proof_key, &[public.as_slice(), phone_public].concat());
    let transcript = [ephemeral.as_slice(), phone_agreement, &nonce, phone_nonce].concat();
    let signature: Signature = SigningKey::from(identity).sign(&transcript);
    let shared = p256::ecdh::diffie_hellman(
        agreement.to_nonzero_scalar(),
        decode_public(phone_agreement).unwrap().as_affine(),
    );
    let salt = Sha256::digest([phone_nonce, &nonce].concat());
    let key: [u8; 16] = hmac(&hmac(&salt, shared.raw_secret_bytes()), b"session_key\x01")[..16]
        .try_into()
        .unwrap();
    let response = Frame {
        sequence: request.sequence,
        unix_seconds: 0,
        source: 0,
        format: 1,
        kind: 2,
        category: 3,
        method: 2,
        body: Fields(BTreeMap::from([
            (1, FieldValue::Varint(1)),
            (2, FieldValue::Bytes(public)),
            (3, FieldValue::Bytes(proof.to_vec())),
            (4, FieldValue::Bytes(ephemeral)),
            (5, FieldValue::Bytes(signature.to_der().as_bytes().to_vec())),
            (6, FieldValue::Bytes(nonce.to_vec())),
            (7, FieldValue::Varint(0)),
        ]))
        .encode()
        .unwrap(),
    };
    (response, key)
}
fn confirm(peer: &MemoryStream, key: &[u8; 16], sequence: u32) {
    let confirm = Frame::decode(&peer.message()).unwrap();
    assert_eq!((confirm.method, confirm.sequence), (3, sequence));
    assert_eq!(
        SecureRecord::new(key)
            .unwrap()
            .open(Fields::decode(&confirm.body).unwrap().bytes(1).unwrap())
            .unwrap(),
        b"127.0.0.1"
    );
}
fn authenticate(peer: &MemoryStream) -> [u8; 16] {
    let request = Frame::decode(&peer.message()).unwrap();
    let (response, key) = auth_response(&request, &SecretKey::random(&mut OsRng), "123456");
    peer.write_all(&response.encode().unwrap()).unwrap();
    confirm(peer, &key, request.sequence.wrapping_add(1));
    key
}
struct RunningSession {
    session: Arc<ClinkSession>,
    transport: Arc<MemoryTransport>,
    link: Arc<Link>,
    peers: BTreeMap<u16, Arc<MemoryStream>>,
    worker: Option<thread::JoinHandle<Result<()>>>,
}
impl RunningSession {
    fn new(
        pin: Arc<dyn PinProvider>,
        trust: Arc<dyn TrustStore>,
        listener: Arc<dyn ClinkListener>,
        wired_pin: Option<String>,
        auto_confirm: bool,
    ) -> Self {
        let ports = ClinkPorts::default();
        let mut streams = BTreeMap::new();
        let mut peers = BTreeMap::new();
        for service in [
            ports.auth,
            ports.control,
            ports.cert,
            ports.media,
            ports.rtsp,
            ports.uibc,
        ] {
            let (stream, peer) = MemoryStream::pair();
            streams.insert(service, stream);
            peers.insert(service, peer);
        }
        let transport = Arc::new(MemoryTransport {
            streams: Mutex::new(streams),
            offers: Mutex::new(Vec::new()),
            authenticated: AtomicBool::new(false),
        });
        let link = Arc::new(Link(
            transport.clone(),
            AtomicBool::new(false),
            wired_pin,
            auto_confirm,
        ));
        let session = Arc::new(
            ClinkSession::new(
                link.clone(),
                pin,
                trust,
                listener,
                Arc::new(Callbacks),
                SessionOptions::default(),
            )
            .unwrap(),
        );
        let worker_session = session.clone();
        let worker = thread::spawn(move || worker_session.run());
        Self {
            session,
            transport,
            link,
            peers,
            worker: Some(worker),
        }
    }
    fn finish(&mut self) -> Result<()> {
        self.worker.take().unwrap().join().unwrap()
    }
}
impl Drop for RunningSession {
    fn drop(&mut self) {
        self.session.stop();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join().unwrap();
        }
    }
}
fn run_memory_session(close_peer: bool) {
    let ports = ClinkPorts::default();
    let mut running =
        RunningSession::new(Arc::new(Pin), Arc::new(Trust), Arc::new(()), None, false);
    let session = running.session.clone();
    let transport = running.transport.clone();
    let link = running.link.clone();
    let peers = running.peers.clone();
    let key = authenticate(&peers[&ports.auth]);
    let config = Frame::decode(&peers[&ports.control].message()).unwrap();
    assert_eq!((config.category, config.method), (1, 24));
    assert!(SecureRecord::new(&key)
        .unwrap()
        .open(&config.body)
        .unwrap()
        .is_empty());
    assert!(peers[&ports.rtsp].message().starts_with(b"OPTIONS "));
    assert!(transport.authenticated.load(Ordering::Acquire));
    assert_eq!(
        *transport.offers.lock().unwrap(),
        [
            ports.control,
            ports.cert,
            ports.media,
            ports.rtsp,
            ports.uibc
        ]
    );
    assert_eq!(session.snapshot().head_unit_id, "memory-car");
    if close_peer {
        peers[&ports.rtsp].shutdown().unwrap();
    } else {
        session.stop();
    }
    let result = running.finish();
    if close_peer {
        assert!(result.is_err());
    } else {
        assert!(result.is_ok());
    }
    assert!(link.1.load(Ordering::Acquire));
    assert!(!session.is_playing());
    for peer in peers.values() {
        assert!(peer.input.state.lock().unwrap().1);
    }
}
#[test]
fn session_authenticates_and_stops_over_memory_transport() {
    run_memory_session(false);
}
#[test]
fn session_cleans_up_after_memory_peer_closes() {
    run_memory_session(true);
}

#[derive(Default)]
struct Pairing {
    peer: Mutex<Option<Vec<u8>>>,
    events: Mutex<Vec<String>>,
    confirm: bool,
}
impl PinProvider for Pairing {
    fn request_pin(&self, _: &str) -> Option<String> {
        self.events.lock().unwrap().push("pin".into());
        Some("123456".into())
    }
    fn confirm_pairing(&self, _: &str) -> bool {
        self.events.lock().unwrap().push("confirm".into());
        self.confirm
    }
    fn pin_incorrect(&self, _: &str) {
        self.events.lock().unwrap().push("incorrect".into());
    }
}
impl TrustStore for Pairing {
    fn load_identity(&self) -> Result<Option<AuthIdentity>> {
        Ok(Some(AuthIdentity::generate()))
    }
    fn save_identity(&self, _: &AuthIdentity) -> Result<()> {
        Ok(())
    }
    fn load_peer_key(&self, _: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.peer.lock().unwrap().clone())
    }
    fn save_peer_key(&self, _: &str, _: &[u8]) -> Result<()> {
        self.events.lock().unwrap().push("save".into());
        Ok(())
    }
    fn forget_peer(&self, _: &str) -> Result<()> {
        *self.peer.lock().unwrap() = None;
        self.events.lock().unwrap().push("forget".into());
        Ok(())
    }
}
impl ClinkListener for Pairing {
    fn diagnostic(&self, message: &str) {
        self.events.lock().unwrap().push(message.into());
    }
}
fn result(peer: &MemoryStream, request: &Frame, code: u64) {
    peer.write_all(
        &Frame {
            body: Fields(BTreeMap::from([
                (1, FieldValue::Varint(1)),
                (7, FieldValue::Varint(code)),
            ]))
            .encode()
            .unwrap(),
            source: 0,
            kind: 2,
            method: 2,
            ..request.clone()
        }
        .encode()
        .unwrap(),
    )
    .unwrap();
}
fn repin_after_key_change(full: bool) {
    let old = SecretKey::random(&mut OsRng);
    let new = SecretKey::random(&mut OsRng);
    let pairing = Arc::new(Pairing {
        peer: Mutex::new(Some(
            old.public_key().to_encoded_point(false).as_bytes()[1..].to_vec(),
        )),
        confirm: true,
        ..Default::default()
    });
    let running = RunningSession::new(
        pairing.clone(),
        pairing.clone(),
        pairing.clone(),
        None,
        false,
    );
    let auth = &running.peers[&ClinkPorts::default().auth];
    let mut request = Frame::decode(&auth.message()).unwrap();
    assert_eq!(request.sequence, 1);
    assert!(!Fields::decode(&request.body).unwrap().0.contains_key(&2));
    if full {
        result(auth, &request, 1);
        request = Frame::decode(&auth.message()).unwrap();
        assert_eq!(request.sequence, 3);
    }
    let (response, _) = auth_response(&request, &new, "123456");
    auth.write_all(&response.encode().unwrap()).unwrap();
    let request = Frame::decode(&auth.message()).unwrap();
    assert_eq!(request.sequence, if full { 5 } else { 3 });
    let fields = Fields::decode(&request.body).unwrap();
    assert!(fields.0.contains_key(&2));
    assert_eq!(fields.number(9).unwrap(), 0);
    // Repinning can itself require confirmation; every new request advances by two.
    result(auth, &request, 2);
    let confirmed = Frame::decode(&auth.message()).unwrap();
    assert_eq!(confirmed.sequence, request.sequence + 2);
    assert_eq!(
        Fields::decode(&confirmed.body).unwrap().number(9).unwrap(),
        1
    );
    let (response, key) = auth_response(&confirmed, &new, "123456");
    auth.write_all(&response.encode().unwrap()).unwrap();
    confirm(auth, &key, confirmed.sequence + 1);
    running.peers[&ClinkPorts::default().control].message();
    assert!(running.transport.authenticated.load(Ordering::Acquire));
    let events = pairing.events.lock().unwrap();
    let reason = if full {
        "peer_key_changed"
    } else {
        "auth_signature_failed"
    };
    let mut expected = Vec::new();
    if full {
        expected.push("pin".to_owned());
    }
    expected.extend([
        "forget".to_owned(),
        format!("auth repin reason={reason}"),
        "pin".to_owned(),
        "confirm".to_owned(),
    ]);
    assert_eq!(*events, expected);
    // Trust is saved only after the later certificate check, never just after AUTH.
    assert!(pairing.peer.lock().unwrap().is_none());
}
#[test]
fn quick_auth_repins_once_on_the_same_transport_after_key_change() {
    repin_after_key_change(false);
}
#[test]
fn full_auth_repins_after_pin_retry_and_advances_confirmation_sequences() {
    repin_after_key_change(true);
}
#[test]
fn repin_is_attempted_only_once_even_if_the_new_signature_also_fails() {
    let old = SecretKey::random(&mut OsRng);
    let new = SecretKey::random(&mut OsRng);
    let pairing = Arc::new(Pairing {
        peer: Mutex::new(Some(
            old.public_key().to_encoded_point(false).as_bytes()[1..].to_vec(),
        )),
        ..Default::default()
    });
    let mut running = RunningSession::new(
        pairing.clone(),
        pairing.clone(),
        pairing.clone(),
        None,
        false,
    );
    let auth = &running.peers[&ClinkPorts::default().auth];
    let request = Frame::decode(&auth.message()).unwrap();
    let (response, _) = auth_response(&request, &new, "123456");
    auth.write_all(&response.encode().unwrap()).unwrap();
    let request = Frame::decode(&auth.message()).unwrap();
    assert_eq!(request.sequence, 3);
    let (mut response, _) = auth_response(&request, &new, "123456");
    let mut fields = Fields::decode(&response.body).unwrap();
    fields.0.insert(5, FieldValue::Bytes(vec![1]));
    response.body = fields.encode().unwrap();
    auth.write_all(&response.encode().unwrap()).unwrap();
    assert_eq!(
        running.finish().unwrap_err().to_string(),
        "auth_signature_failed"
    );
    assert_eq!(
        *pairing.events.lock().unwrap(),
        ["forget", "auth repin reason=auth_signature_failed", "pin"]
    );
}
#[test]
fn incorrect_pin_is_reported_for_both_result_code_and_proof_failure_without_retrying() {
    for proof_failure in [false, true] {
        let pairing = Arc::new(Pairing::default());
        let mut running = RunningSession::new(
            pairing.clone(),
            pairing.clone(),
            pairing.clone(),
            None,
            false,
        );
        let auth = &running.peers[&ClinkPorts::default().auth];
        let request = Frame::decode(&auth.message()).unwrap();
        if proof_failure {
            let (response, _) = auth_response(&request, &SecretKey::random(&mut OsRng), "654321");
            auth.write_all(&response.encode().unwrap()).unwrap();
        } else {
            result(auth, &request, 1);
        }
        assert_eq!(
            running.finish().unwrap_err().to_string(),
            if proof_failure {
                "auth_pin_proof_failed"
            } else {
                "auth_pin_required"
            }
        );
        assert_eq!(*pairing.events.lock().unwrap(), ["pin", "incorrect"]);
        assert!(!running.transport.authenticated.load(Ordering::Acquire));
    }
}
#[test]
fn wired_pin_and_auto_confirm_use_the_connections_existing_consent() {
    let pairing = Arc::new(Pairing::default());
    let running = RunningSession::new(
        pairing.clone(),
        pairing.clone(),
        pairing.clone(),
        Some("123456".into()),
        true,
    );
    let auth = &running.peers[&ClinkPorts::default().auth];
    let request = Frame::decode(&auth.message()).unwrap();
    result(auth, &request, 2);
    let request = Frame::decode(&auth.message()).unwrap();
    assert_eq!(request.sequence, 3);
    assert_eq!(Fields::decode(&request.body).unwrap().number(9).unwrap(), 1);
    let (response, key) = auth_response(&request, &SecretKey::random(&mut OsRng), "123456");
    auth.write_all(&response.encode().unwrap()).unwrap();
    confirm(auth, &key, 4);
    running.peers[&ClinkPorts::default().control].message();
    assert!(pairing.events.lock().unwrap().is_empty());
}

mod media_output {
    use super::*;
    use base64::{engine::general_purpose::STANDARD, Engine};
    use oxp_clink::{placeholder::*, wfd::*};
    use oxp_media::{nal, AvcConfig};
    use std::time::Instant;

    #[derive(Default)]
    struct Probe {
        displays: Mutex<Vec<DisplayInfo>>,
        touches: Mutex<Vec<Vec<TouchContact>>>,
        views: Mutex<Vec<(u32, u32)>>,
        reports: Mutex<Vec<ClinkStatistics>>,
        errors: Mutex<Vec<String>>,
    }
    impl ClinkListener for Probe {
        fn view_size(&self, width: u32, height: u32) {
            self.views.lock().unwrap().push((width, height));
        }
        fn statistics(&self, stats: ClinkStatistics) {
            self.reports.lock().unwrap().push(stats);
        }
        fn error(&self, error: &str) {
            self.errors.lock().unwrap().push(error.into());
        }
    }
    impl OutputHost for Probe {
        fn capabilities(&self, caps: OutputCapabilities) {
            self.displays
                .lock()
                .unwrap()
                .push(caps.displays[&0].clone());
        }
        fn control(&self, event: ControlEvent) {
            if let ControlEvent::Touch { contacts, .. } = event {
                self.touches.lock().unwrap().push(contacts);
            }
        }
        fn stats(&self, _: OutputStats) {}
    }
    fn await_condition(label: &str, condition: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(6);
        while !condition() {
            assert!(Instant::now() < deadline, "timed out: {label}");
            thread::sleep(Duration::from_millis(2));
        }
    }
    struct Output {
        output: ClinkOutput,
        transport: Arc<MemoryTransport>,
        probe: Arc<Probe>,
    }
    impl Output {
        fn new() -> Self {
            let transport = Arc::new(MemoryTransport {
                streams: Mutex::new(BTreeMap::new()),
                offers: Mutex::new(Vec::new()),
                authenticated: AtomicBool::new(false),
            });
            let probe = Arc::new(Probe::default());
            let output = ClinkOutput::new(
                Arc::new(Link(transport.clone(), AtomicBool::new(false), None, false)),
                Arc::new(Pin),
                Arc::new(Trust),
            )
            .unwrap()
            .with_listener(probe.clone());
            Self {
                output,
                transport,
                probe,
            }
        }
        fn connect(&self) -> (BTreeMap<u16, Arc<MemoryStream>>, [u8; 16]) {
            let ports = ClinkPorts::default();
            let mut peers = BTreeMap::new();
            for service in [
                ports.auth,
                ports.control,
                ports.cert,
                ports.media,
                ports.rtsp,
                ports.uibc,
                ports.rtp,
            ] {
                let (stream, peer) = MemoryStream::pair();
                stream
                    .set_read_timeout(Some(Duration::from_secs(30)))
                    .unwrap();
                self.transport
                    .streams
                    .lock()
                    .unwrap()
                    .insert(service, stream);
                peers.insert(service, peer);
            }
            self.probe.reports.lock().unwrap().clear();
            self.output.open(self.probe.clone()).unwrap();
            let key = authenticate(&peers[&ports.auth]);
            let config = Frame::decode(&peers[&ports.control].message()).unwrap();
            assert_eq!(config.method, 24);
            let display = DisplayInfo {
                width_px: 1532,
                height_px: 1013,
                density_dpi: 240,
                refresh_hz: 30.0,
                ..Default::default()
            };
            let send = |service, sequence, kind, category, method, body: &[u8]| {
                peers[&service]
                    .write_all(
                        &Frame {
                            sequence,
                            unix_seconds: 0,
                            source: 0,
                            format: 1,
                            kind,
                            category,
                            method,
                            body: SecureRecord::new(&key).unwrap().seal(body).unwrap(),
                        }
                        .encode()
                        .unwrap(),
                    )
                    .unwrap();
            };
            send(
                ports.control,
                config.sequence,
                2,
                1,
                25,
                &control::display_body(&display).unwrap(),
            );
            let metadata = br#"{"short_name":"car","product_name":"model","vendor_name":"vendor","vendor_id":"v1","product_id":"p1"}"#;
            let envelope = serde_json::json!({"content":metadata.iter().map(|b|*b as i8).collect::<Vec<_>>(),
                "signature":vec![0;256],"version":1,"signAlgorithm":"SHA256withRSA","cryptoConfigIndex":"v1"});
            let cert = Fields(BTreeMap::from([(
                1,
                FieldValue::Bytes(STANDARD.encode(envelope.to_string()).into_bytes()),
            )]))
            .encode()
            .unwrap();
            send(ports.cert, 80, 3, 6, 1, &cert);
            let rtsp = &peers[&ports.rtsp];
            for _ in 0..4 {
                let request = RtspReader::default()
                    .feed(&rtsp.message())
                    .unwrap()
                    .remove(0);
                let body = if request.line.starts_with("GET_PARAMETER ") {
                    include_str!("../../../kotlin/clink/src/test/resources/head-unit-wfd.txt")
                } else {
                    ""
                };
                rtsp.write_all(
                    &RtspMessage {
                        line: "RTSP/1.0 200 OK".into(),
                        headers: BTreeMap::from([("cseq".into(), request.headers["cseq"].clone())]),
                        body: body.into(),
                    }
                    .encode()
                    .unwrap(),
                )
                .unwrap();
            }
            rtsp.write_all(
                &RtspMessage {
                    line: "SETUP * RTSP/1.0".into(),
                    headers: BTreeMap::from([
                        ("cseq".into(), "101".into()),
                        (
                            "transport".into(),
                            format!("RTP/AVP/TCP;unicast;client_port={}", ports.rtp),
                        ),
                    ]),
                    body: String::new(),
                }
                .encode()
                .unwrap(),
            )
            .unwrap();
            let setup = RtspReader::default()
                .feed(&rtsp.message())
                .unwrap()
                .remove(0);
            assert_eq!(setup.line, "RTSP/1.0 200 OK");
            rtsp.write_all(
                &RtspMessage {
                    line: "PLAY * RTSP/1.0".into(),
                    headers: BTreeMap::from([
                        ("cseq".into(), "102".into()),
                        ("session".into(), setup.headers["session"].clone()),
                    ]),
                    body: String::new(),
                }
                .encode()
                .unwrap(),
            )
            .unwrap();
            let play = RtspReader::default()
                .feed(&rtsp.message())
                .unwrap()
                .remove(0);
            assert_eq!(play.line, "RTSP/1.0 200 OK");
            self.stats(|stats| stats.placeholder_frames > 0);
            (peers, key)
        }
        fn stats(&self, predicate: impl Fn(&ClinkStatistics) -> bool) -> ClinkStatistics {
            await_condition("media statistics", || {
                self.probe.reports.lock().unwrap().iter().any(&predicate)
            });
            *self
                .probe
                .reports
                .lock()
                .unwrap()
                .iter()
                .find(|s| predicate(s))
                .unwrap()
        }
        fn touch(
            &self,
            peers: &BTreeMap<u16, Arc<MemoryStream>>,
            key: &[u8; 16],
            view: (u32, u32),
            point: (u32, u32),
        ) -> Vec<TouchContact> {
            let count = self.probe.touches.lock().unwrap().len();
            let touch = serde_json::json!({"type":1,"action":2,"count":1,"trackID0":7,
                "x0":point.0,"y0":point.1,"width":view.0,"height":view.1});
            peers[&ClinkPorts::default().uibc]
                .write_all(
                    &SecureRecord::new(key)
                        .unwrap()
                        .seal(touch.to_string().as_bytes())
                        .unwrap(),
                )
                .unwrap();
            await_condition("touch", || self.probe.touches.lock().unwrap().len() > count);
            self.probe.touches.lock().unwrap().last().unwrap().clone()
        }
        fn frame(&self, data: Arc<[u8]>, pts_us: i64, key_frame: bool) {
            self.output.on_video_frame(VideoFrame {
                screen: 0,
                session: 1,
                data,
                pts_us,
                key_frame,
                received_at_nanos: 0,
            });
        }
        fn close(&self) {
            self.output.close();
            assert!(self.output.await_closed(Duration::from_secs(3)));
        }
    }
    impl Drop for Output {
        fn drop(&mut self) {
            self.output.close();
            self.output.await_closed(Duration::from_secs(3));
        }
    }
    fn live_clip() -> (Arc<VideoFormat>, Arc<[u8]>) {
        let bytes =
            include_bytes!("../../../kotlin/core/src/test/resources/media/baseline-640x360.h264");
        let units = nal::units(bytes, nal::Format::AnnexB).unwrap();
        let sps = units.iter().find(|n| n[0] & 31 == 7).unwrap();
        let pps = units.iter().find(|n| n[0] & 31 == 8).unwrap();
        let config = AvcConfig::from_parameter_sets(sps, pps).unwrap();
        let format = VideoFormat::create(0, VideoCodec::H264, config.into(), None).unwrap();
        let mut data = Vec::new();
        for n in units {
            let kind = n[0] & 31;
            if kind == 9 && !data.is_empty() {
                break;
            }
            if [1, 5, 6].contains(&kind) {
                data.extend_from_slice(&(n.len() as u32).to_be_bytes());
                data.extend_from_slice(n);
            }
        }
        (Arc::new(format), data.into())
    }
    #[test]
    fn output_tracks_emitted_sizes_views_and_statistics_across_reconnect() {
        let test = Output::new();
        let placeholder = BundledPlaceholderVideo::new().unwrap();
        let (live, data) = live_clip();
        assert_eq!((live.width, live.height), (640, 360));
        let center = vec![TouchContact::new(0, 0.5, 0.5, true).unwrap()];
        for generation in 0..2 {
            let (peers, key) = test.connect();
            let stats = test.stats(|s| s.placeholder_frames > 0);
            assert_eq!(stats.live_frames, 0);
            assert_eq!(stats.live_bytes, 0);
            assert_eq!(stats.dropped_frames, 0);
            assert_eq!(stats.send_stalls, 0);
            assert_eq!((stats.codec, stats.width, stats.height), (None, 0, 0));
            for view in [(1532, 1013), (1532, 1013), (1814, 1200), (1532, 1013)] {
                assert_eq!(test.touch(&peers, &key, view, (640, 360)), center);
            }
            assert_eq!(
                *test.probe.views.lock().unwrap(),
                [(1532, 1013), (1814, 1200), (1532, 1013)].repeat(generation + 1)
            );
            assert_eq!(
                test.probe.displays.lock().unwrap().last().unwrap(),
                &DisplayInfo {
                    width_px: 1532,
                    height_px: 1013,
                    density_dpi: 240,
                    refresh_hz: 30.0,
                    viewport: Some(Viewport {
                        width_px: 1532,
                        height_px: 1013
                    }),
                    fullscreen: Some(true),
                    ..Default::default()
                }
            );
            if generation == 1 {
                test.close();
                break;
            }
            test.output.on_session(&SessionState {
                id: 1,
                phase: SessionPhase::Active,
                reason: None,
            });
            test.output.on_video_format(0, Some(live.clone()));
            test.frame(data.clone(), 0, true);
            let first = test.stats(|s| s.live_frames == 1);
            assert_eq!(first.live_bytes, data.len() as u64);
            assert_eq!(
                (first.codec, first.width, first.height),
                (Some(VideoCodec::H264), 640, 360)
            );
            assert_eq!(test.touch(&peers, &key, (1532, 1013), (320, 180)), center);
            test.output
                .on_video_format(0, Some(Arc::new(placeholder.format().clone())));
            // Announcing a format cannot change touch scaling before any frame of it is sent.
            assert_eq!(test.touch(&peers, &key, (1532, 1013), (320, 180)), center);
            test.frame(placeholder.frames()[0].clone(), 33_333, true);
            let second = test.stats(|s| s.live_frames == 2);
            assert_eq!((second.width, second.height), (1280, 720));
            assert_eq!(test.touch(&peers, &key, (1532, 1013), (640, 360)), center);
            test.output.on_video_format(0, Some(live.clone()));
            test.frame(data.clone(), 66_666, true);
            let third = test.stats(|s| s.live_frames == 3);
            assert_eq!((third.width, third.height), (640, 360));
            assert_eq!(
                third.live_bytes,
                (2 * data.len() + placeholder.frames()[0].len()) as u64
            );
            test.output.on_video_format(0, None);
            test.stats(|s| s.placeholder_frames > third.placeholder_frames);
            assert_eq!(test.touch(&peers, &key, (1532, 1013), (640, 360)), center);
            let mut unsupported = live.as_ref().clone();
            unsupported.codec = VideoCodec::H265;
            test.output.on_video_format(0, Some(Arc::new(unsupported)));
            test.frame(data.clone(), 99_999, true);
            test.frame(data.clone(), 133_332, true);
            let skipped = test.stats(|s| s.dropped_frames >= third.dropped_frames + 2);
            assert_eq!(skipped.dropped_frames, third.dropped_frames + 2);
            assert_eq!(skipped.live_frames, third.live_frames);
            assert_eq!(skipped.live_bytes, third.live_bytes);
            assert_eq!(
                (skipped.codec, skipped.width, skipped.height),
                (Some(VideoCodec::H264), 640, 360)
            );
            assert_eq!(
                *test.probe.errors.lock().unwrap(),
                ["Carlink v0 requires H.264 input"]
            );
            test.close();
        }
    }
}
