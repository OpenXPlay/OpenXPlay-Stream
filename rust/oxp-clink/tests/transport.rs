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
struct Link(Arc<MemoryTransport>, AtomicBool);
impl ClinkLink for Link {
    fn connect(&self, _: Arc<dyn Fn(Option<String>) + Send + Sync>) -> Result<ClinkConnection> {
        Ok(ClinkConnection {
            head_unit_id: "memory-car".into(),
            local_ipv4: [127, 0, 0, 1].into(),
            transport: self.0.clone(),
            wired_pin: None,
            auto_confirm: false,
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
fn authenticate(peer: &MemoryStream) -> [u8; 16] {
    let request = Frame::decode(&peer.message()).unwrap();
    let fields = Fields::decode(&request.body).unwrap();
    let identity = SecretKey::random(&mut OsRng);
    let agreement = SecretKey::random(&mut OsRng);
    let public = identity.public_key().to_encoded_point(false).as_bytes()[1..].to_vec();
    let ephemeral = agreement.public_key().to_encoded_point(false).as_bytes()[1..].to_vec();
    let nonce = [17; 32];
    let phone_nonce = fields.bytes(6).unwrap();
    let phone_public = fields.bytes(2).unwrap();
    let phone_agreement = fields.bytes(4).unwrap();
    let proof_key = Sha256::digest([nonce.as_slice(), phone_nonce, b"123456"].concat());
    let proof = hmac(&proof_key, &[public.as_slice(), phone_public].concat());
    let transcript = [ephemeral.as_slice(), phone_agreement, &nonce, phone_nonce].concat();
    let signature: Signature = SigningKey::from(&identity).sign(&transcript);
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
    peer.write_all(&response.encode().unwrap()).unwrap();
    let confirm = Frame::decode(&peer.message()).unwrap();
    assert_eq!(confirm.method, 3);
    assert_eq!(
        SecureRecord::new(&key)
            .unwrap()
            .open(Fields::decode(&confirm.body).unwrap().bytes(1).unwrap())
            .unwrap(),
        b"127.0.0.1"
    );
    key
}
fn run_memory_session(close_peer: bool) {
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
    let link = Arc::new(Link(transport.clone(), AtomicBool::new(false)));
    let session = Arc::new(
        ClinkSession::new(
            link.clone(),
            Arc::new(Pin),
            Arc::new(Trust),
            Arc::new(()),
            Arc::new(Callbacks),
            SessionOptions::default(),
        )
        .unwrap(),
    );
    let worker_session = session.clone();
    let worker = thread::spawn(move || worker_session.run());
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
    let result = worker.join().unwrap();
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
