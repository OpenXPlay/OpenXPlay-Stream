// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use crate::{
    interfaces::*,
    model::*,
    protocol::{certificate, control, *},
    require,
    uibc::{key as decode_key, UibcDecoder},
    wfd::*,
    ClinkStream, Result,
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
pub trait SessionCallbacks: Send + Sync {
    fn capabilities(&self, caps: OutputCapabilities);
    fn control(&self, event: ControlEvent);
    fn play(&self, playing: bool);
    fn keyframe(&self);
    fn link_stalled(&self, _stalled: bool) {}
}
#[derive(Clone)]
pub struct SessionOptions {
    pub ports: ClinkPorts,
    pub output_id: String,
    pub device_id: String,
    pub device_name: String,
    /// Monotonic session clock, injectable for timeout and record-window tests.
    pub clock: Arc<dyn Fn() -> Instant + Send + Sync>,
}
impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            ports: Default::default(),
            output_id: "clink".into(),
            device_id: "openxplay-stream".into(),
            device_name: "OpenXPlay".into(),
            clock: Arc::new(Instant::now),
        }
    }
}
#[derive(Clone, Debug)]
pub struct SessionSnapshot {
    pub head_unit_id: String,
    pub playing: bool,
    pub mode: WfdMode,
    pub decoders: Vec<DecoderSupport>,
    pub rtp_port: u16,
    pub media_audio: bool,
}
impl Default for SessionSnapshot {
    fn default() -> Self {
        Self {
            head_unit_id: String::new(),
            playing: false,
            mode: Default::default(),
            decoders: Vec::new(),
            rtp_port: 15550,
            media_audio: false,
        }
    }
}
#[derive(Default)]
struct Microphone {
    desired: Option<MicRequest>,
    applied: Option<MicRequest>,
    bytes: Vec<u8>,
    clock: Option<oxp_core::audio::AudioClock>,
}
/// Writes in progress per stream, with the time each one started.
type PendingWrites = BTreeMap<usize, (Arc<dyn ClinkStream>, Instant)>;
struct AudioTransport {
    key: [u8; 16],
    sockets: BTreeMap<u16, Arc<dyn ClinkStream>>,
}
const RECEIVE_TIMEOUT: Duration = Duration::from_secs(15);
const STALL_THRESHOLD: Duration = Duration::from_secs(5);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);

struct LinkHealth {
    last_received: Instant,
    stalled: bool,
}
impl LinkHealth {
    fn new(now: Instant) -> Self {
        Self {
            last_received: now,
            stalled: false,
        }
    }
    fn poll(&mut self, now: Instant) -> Result<Option<String>> {
        let elapsed = now.duration_since(self.last_received);
        require(elapsed <= RECEIVE_TIMEOUT, "heartbeat_timeout")?;
        if elapsed >= STALL_THRESHOLD && !self.stalled {
            self.stalled = true;
            return Ok(Some(format!("link stalled ms={}", elapsed.as_millis())));
        }
        Ok(None)
    }
    fn received(&mut self, now: Instant) -> Option<String> {
        let elapsed = now.duration_since(self.last_received);
        self.last_received = now;
        if std::mem::take(&mut self.stalled) {
            Some(format!("link recovered after ms={}", elapsed.as_millis()))
        } else {
            None
        }
    }
}
struct Shared {
    stalled: AtomicBool,
    stopped: AtomicBool,
    stop_requested: AtomicBool,
    sequence: AtomicU32,
    audio: Mutex<Option<AudioTransport>>,
    acknowledgements: Mutex<BTreeMap<u32, (u16, mpsc::Sender<()>)>>,
    microphone: Mutex<Microphone>,
    write_locks: Mutex<BTreeMap<usize, Arc<Mutex<()>>>>,
    snapshot: Mutex<SessionSnapshot>,
    stream_size: Mutex<(u32, u32)>,
    resources: Mutex<Vec<Arc<dyn ClinkStream>>>,
    writes: Mutex<PendingWrites>,
    video: Mutex<Option<(u16, Arc<dyn ClinkStream>)>>,
    connection: Mutex<Option<ClinkConnection>>,
}
pub struct ClinkSession {
    link: Arc<dyn ClinkLink>,
    pin: Arc<dyn PinProvider>,
    trust: Arc<dyn TrustStore>,
    listener: Arc<dyn ClinkListener>,
    callbacks: Arc<dyn SessionCallbacks>,
    options: SessionOptions,
    shared: Arc<Shared>,
    started: AtomicBool,
}
enum Event {
    Accepted(u16, Arc<dyn ClinkStream>),
    Bytes(u16, Vec<u8>),
    Closed(u16, Option<String>),
    Lost(String),
}
impl ClinkSession {
    pub fn new(
        link: Arc<dyn ClinkLink>,
        pin: Arc<dyn PinProvider>,
        trust: Arc<dyn TrustStore>,
        listener: Arc<dyn ClinkListener>,
        callbacks: Arc<dyn SessionCallbacks>,
        options: SessionOptions,
    ) -> Result<Self> {
        options.ports.validate()?;
        let snapshot = SessionSnapshot {
            rtp_port: options.ports.rtp,
            ..Default::default()
        };
        Ok(Self {
            link,
            pin,
            trust,
            listener,
            callbacks,
            options,
            started: AtomicBool::new(false),
            shared: Arc::new(Shared {
                stalled: AtomicBool::new(false),
                stopped: AtomicBool::new(false),
                stop_requested: AtomicBool::new(false),
                sequence: AtomicU32::new(10),
                audio: Mutex::new(None),
                acknowledgements: Mutex::new(BTreeMap::new()),
                microphone: Mutex::new(Microphone::default()),
                write_locks: Mutex::new(BTreeMap::new()),
                snapshot: Mutex::new(snapshot),
                stream_size: Mutex::new((1280, 720)),
                resources: Mutex::new(Vec::new()),
                writes: Mutex::new(BTreeMap::new()),
                video: Mutex::new(None),
                connection: Mutex::new(None),
            }),
        })
    }
    pub fn stop(&self) {
        let _ = self.microphone(None);
        self.shared.stop_requested.store(true, Ordering::Release);
    }
    pub(crate) fn stream_size(&self) -> (u32, u32) {
        *self.shared.stream_size.lock().unwrap()
    }
    pub(crate) fn set_stream_size(&self, size: (u32, u32)) {
        *self.shared.stream_size.lock().unwrap() = size;
    }
    pub fn microphone(&self, request: Option<MicRequest>) -> Result<()> {
        if let Some(r) = &request {
            require(
                r.codec == AudioCodec::PcmS16le
                    && (8000..=48000).contains(&r.sample_rate)
                    && (1..=2).contains(&r.channels)
                    && r.frame_ms == 20
                    && r.sample_rate % 50 == 0,
                "invalid_microphone_request",
            )?;
        }
        let mut mic = self.shared.microphone.lock().unwrap();
        if same_mic(&mic.desired, &request) {
            return Ok(());
        }
        mic.clock = request
            .as_ref()
            .map(|r| oxp_core::audio::AudioClock::new(r.sample_rate).unwrap());
        mic.bytes.clear();
        mic.desired = request;
        Ok(())
    }
    pub fn audio_ready(&self) -> bool {
        self.is_playing()
            && self
                .shared
                .audio
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|a| a.sockets.contains_key(&self.options.ports.media))
    }
    fn send_audio(
        &self,
        service: u16,
        category: u8,
        method: u16,
        body: &[u8],
        format: u8,
        sync: bool,
    ) -> Result<()> {
        let (socket, key) = {
            let audio = self.shared.audio.lock().unwrap();
            let a = audio.as_ref().ok_or("missing_audio_transport")?;
            (
                a.sockets.get(&service).cloned().ok_or("missing_channel")?,
                a.key,
            )
        };
        let seq = self
            .shared
            .sequence
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        let (tx, rx) = mpsc::channel();
        if sync {
            self.shared
                .acknowledgements
                .lock()
                .unwrap()
                .insert(seq, (service, tx));
        }
        let result = (|| {
            let frame = Frame {
                sequence: seq,
                unix_seconds: now(),
                source: 1,
                format,
                kind: if sync { 3 } else { 0 },
                category,
                method,
                body: SecureRecord::new(&key)?.seal(body)?,
            };
            write(&self.shared, &socket, &frame.encode()?)?;
            if sync {
                let deadline = Instant::now() + Duration::from_secs(2);
                while !stopped(&self.shared) && !self.shared.stop_requested.load(Ordering::Acquire)
                {
                    match rx.recv_timeout(Duration::from_millis(10)) {
                        Ok(()) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            require(Instant::now() < deadline, "audio_ack_timeout")?
                        }
                        Err(_) => break,
                    }
                }
            }
            Ok(())
        })();
        self.shared.acknowledgements.lock().unwrap().remove(&seq);
        result
    }
    pub fn voice_state(&self, start: bool) -> Result<()> {
        self.send_audio(
            self.options.ports.control,
            1,
            8,
            &audio_fields(&[
                (1, 3),
                (2, if start { 0 } else { 1 }),
                (3, 16000),
                (4, 1),
                (5, 2),
            ])?,
            1,
            true,
        )
    }
    pub fn send_voice(&self, data: &[u8], wait_for_ack: bool) -> Result<()> {
        self.send_audio(self.options.ports.media, 4, 3, data, 0, wait_for_ack)
    }
    pub fn snapshot(&self) -> SessionSnapshot {
        self.shared.snapshot.lock().unwrap().clone()
    }
    pub fn is_playing(&self) -> bool {
        !stopped(&self.shared)
            && !self.shared.stop_requested.load(Ordering::Acquire)
            && self.shared.snapshot.lock().unwrap().playing
    }
    fn dial(&self, port: u16) -> Result<Arc<dyn ClinkStream>> {
        require(!stopped(&self.shared), "session_stopped")?;
        let a = self
            .shared
            .connection
            .lock()
            .unwrap()
            .clone()
            .ok_or("missing_connection")?;
        let socket = a.transport.open(port, Duration::from_secs(12))?;
        self.shared.resources.lock().unwrap().push(socket.clone());
        require(!stopped(&self.shared), "session_stopped")?;
        Ok(socket)
    }
    pub fn is_stalled(&self) -> bool {
        self.shared.stalled.load(Ordering::Acquire)
    }
    fn update_link(&self, health: &mut LinkHealth, now: Instant, received: bool) -> Result<()> {
        if let Some(message) = health.poll(now)? {
            self.link_transition(true, &message);
        }
        if received {
            if let Some(message) = health.received(now) {
                self.link_transition(false, &message);
            }
        }
        Ok(())
    }
    fn link_transition(&self, stalled: bool, message: &str) {
        self.shared.stalled.store(stalled, Ordering::Release);
        self.callbacks.link_stalled(stalled);
        self.listener.diagnostic(message);
        if !stalled {
            self.callbacks.keyframe();
        }
    }
    pub fn send_rtp(&self, packet: &[u8]) -> Result<bool> {
        if !self.is_playing() || self.is_stalled() {
            return Ok(false);
        }
        let destination = self.destination_port();
        let mut video = self.shared.video.lock().unwrap();
        if video.as_ref().is_some_and(|(p, _)| *p != destination) {
            if let Some((_, s)) = video.take() {
                let _ = s.shutdown();
            }
        }
        let result = (|| {
            if video.is_none() {
                *video = Some((destination, self.dial(destination)?));
            }
            write(&self.shared, &video.as_ref().unwrap().1, packet)
        })();
        match result {
            Ok(()) => Ok(true),
            Err(_) if !self.is_playing() => Ok(false),
            Err(e) => Err(e),
        }
    }
    fn destination_port(&self) -> u16 {
        self.shared.snapshot.lock().unwrap().rtp_port
    }
    fn listen(&self, service: u16, a: &ClinkConnection, tx: SyncSender<Event>) -> Result<()> {
        a.transport.offer(service)?;
        let transport = a.transport.clone();
        let shared = self.shared.clone();
        spawn(format!("clink-accept-{service}"), move || {
            let socket = loop {
                if stopped(&shared) {
                    return;
                }
                match transport.open(service, Duration::from_millis(100)) {
                    Ok(socket) => break socket,
                    Err(e)
                        if e.downcast_ref::<std::io::Error>().is_some_and(|e| {
                            matches!(
                                e.kind(),
                                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                            )
                        }) =>
                    {
                        continue
                    }
                    Err(e) => {
                        enqueue(&shared, &tx, Event::Lost(e.to_string()));
                        return;
                    }
                }
            };
            shared.resources.lock().unwrap().push(socket.clone());
            if stopped(&shared) {
                let _ = socket.shutdown();
                return;
            }
            drop(transport);
            enqueue(&shared, &tx, Event::Accepted(service, socket.clone()));
            let mut buffer = [0; 65536];
            let mut cause = None;
            while !stopped(&shared) {
                match socket.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => enqueue(&shared, &tx, Event::Bytes(service, buffer[..n].to_vec())),
                    Err(e) => {
                        cause = Some(e.to_string());
                        break;
                    }
                }
            }
            enqueue(&shared, &tx, Event::Closed(service, cause));
        });
        Ok(())
    }
    fn authenticate(
        &self,
        socket: &Arc<dyn ClinkStream>,
        identity: AuthIdentity,
        lost: &Mutex<Option<String>>,
    ) -> Result<([u8; 16], Vec<u8>)> {
        socket.set_read_timeout(Some(Duration::from_millis(250)))?;
        let a = self.shared.connection.lock().unwrap().clone().unwrap();
        let pinned = self.trust.load_peer_key(&a.head_unit_id)?;
        let request_pin = || {
            a.wired_pin
                .clone()
                .or_else(|| self.pin.request_pin(&a.head_unit_id))
                .ok_or("pin_cancelled")
        };
        let mut full = pinned.is_none();
        let mut pin = if full { Some(request_pin()?) } else { None };
        let mut auth = Auth::new(
            identity.clone(),
            self.options.device_id.clone(),
            self.options.device_name.clone(),
            a.local_ipv4.to_string(),
            pinned,
        )?;
        let mut sequence = 1u32;
        let mut attempt = auth.start(now(), sequence, pin.as_deref(), false, false)?;
        let mut decoder = FrameReader::default();
        let mut buffer = [0; 65536];
        let mut deadline = (self.options.clock)() + Duration::from_secs(20);
        let mut retries = 0;
        let mut repinned = false;
        while !stopped(&self.shared) && !self.shared.stop_requested.load(Ordering::Acquire) {
            if let Some(c) = lost.lock().unwrap().clone() {
                return Err(c.into());
            }
            for f in &attempt.frames {
                write(&self.shared, socket, &f.encode()?)?;
            }
            if let Some(k) = attempt.session_key {
                let peer = attempt
                    .events
                    .iter()
                    .find_map(|e| {
                        if let AuthEvent::PeerVerified(p) = e {
                            Some(p.clone())
                        } else {
                            None
                        }
                    })
                    .ok_or("missing_verified_peer")?;
                return Ok((k, peer));
            }
            require((self.options.clock)() < deadline, "auth_timeout")?;
            let n = match socket.read(&mut buffer) {
                Ok(0) => return Err("auth_closed".into()),
                Ok(n) => n,
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    ) =>
                {
                    0
                }
                Err(e) => return Err(e.into()),
            };
            attempt = AuthOutput::default();
            if n == 0 {
                continue;
            }
            for frame in decoder.feed(&buffer[..n])? {
                attempt = auth.receive(&frame, now())?;
                let failed = attempt.events.iter().find_map(|e| match e {
                    AuthEvent::Failed(reason) => Some(reason.clone()),
                    _ => None,
                });
                if let Some(reason) = failed {
                    if reason == "auth_pin_proof_failed" {
                        self.pin.pin_incorrect(&a.head_unit_id);
                    }
                    if !matches!(
                        reason.as_str(),
                        "peer_key_changed" | "auth_signature_failed"
                    ) || repinned
                    {
                        return Err(reason.into());
                    }
                    // A head unit may rebuild its pairing store at boot. Pair again once on this socket.
                    self.trust.forget_peer(&a.head_unit_id)?;
                    self.listener
                        .diagnostic(&format!("auth repin reason={reason}"));
                    repinned = true;
                    full = true;
                    pin = Some(request_pin()?);
                    sequence = sequence.wrapping_add(2);
                    auth = Auth::new(
                        identity.clone(),
                        self.options.device_id.clone(),
                        self.options.device_name.clone(),
                        a.local_ipv4.to_string(),
                        None,
                    )?;
                    attempt = auth.start(now(), sequence, pin.as_deref(), true, false)?;
                    deadline = (self.options.clock)() + Duration::from_secs(20);
                    break;
                }
                let result = attempt.events.iter().find_map(|e| {
                    if let AuthEvent::Result(r, _) = e {
                        Some(*r)
                    } else {
                        None
                    }
                });
                if result == Some(AuthResult::PinRequired) && full {
                    self.pin.pin_incorrect(&a.head_unit_id);
                }
                if result == Some(AuthResult::PinRequired) && !full {
                    let old = retries;
                    retries += 1;
                    if old == 0 {
                        full = true;
                        pin = Some(request_pin()?);
                        sequence = sequence.wrapping_add(2);
                        attempt = auth.start(now(), sequence, pin.as_deref(), true, false)?;
                        deadline = (self.options.clock)() + Duration::from_secs(20);
                    } else {
                        return Err("auth_pin_required".into());
                    }
                } else if result == Some(AuthResult::ConfirmationRequired) {
                    let old = retries;
                    retries += 1;
                    require(old < 3, "auth_confirmation_required")?;
                    require(
                        a.auto_confirm || self.pin.confirm_pairing(&a.head_unit_id),
                        "pairing_confirmation_cancelled",
                    )?;
                    sequence = sequence.wrapping_add(2);
                    attempt = auth.start(now(), sequence, pin.as_deref(), full, true)?;
                    deadline = (self.options.clock)() + Duration::from_secs(20);
                } else if result.is_some_and(|r| r != AuthResult::Success) {
                    return Err(match result.unwrap() {
                        AuthResult::Success => "auth_success",
                        AuthResult::PinRequired => "auth_pin_required",
                        AuthResult::ConfirmationRequired => "auth_confirmation_required",
                        AuthResult::InternalError => "auth_internal_error",
                        AuthResult::Rejected => "auth_rejected",
                    }
                    .into());
                }
            }
        }
        Err("session_stopped".into())
    }
    /// Blocking lifecycle worker; each instance owns one socket generation.
    pub fn run(&self) -> Result<()> {
        require(
            !self.started.swap(true, Ordering::AcqRel),
            "session_already_started",
        )?;
        if stopped(&self.shared) || self.shared.stop_requested.load(Ordering::Acquire) {
            return Ok(());
        }
        let shared = self.shared.clone();
        let watchdog = thread::Builder::new()
            .name("clink-watchdog".into())
            .spawn(move || {
                while !stopped(&shared) {
                    for (socket, t) in shared.writes.lock().unwrap().values() {
                        if t.elapsed() > RECEIVE_TIMEOUT {
                            let _ = socket.shutdown();
                        }
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                for socket in shared.resources.lock().unwrap().iter() {
                    let _ = socket.shutdown();
                }
            })?;
        let origin = Mutex::new(None);
        let result = self.run_inner(&origin);
        if let Err(e) = &result {
            if !stopped(&self.shared) && !self.shared.stop_requested.load(Ordering::Acquire) {
                self.listener.error(&e.to_string());
            }
        }
        self.shared.stopped.store(true, Ordering::Release);
        self.shared.acknowledgements.lock().unwrap().clear();
        self.shared.snapshot.lock().unwrap().playing = false;
        let _ = watchdog.join();
        self.shared.connection.lock().unwrap().take();
        self.callbacks.play(false);
        let origin = origin.lock().unwrap().take();
        if let Some(origin) = origin {
            self.callbacks.control(ControlEvent::Touch {
                screen: 0,
                contacts: Vec::new(),
                origin,
            });
        }
        if let Err(e) = self.link.disconnect() {
            self.listener.error(&e.to_string());
        }
        self.listener.state(ClinkState::Ended);
        result
    }
    fn run_inner(&self, origin_slot: &Mutex<Option<Origin>>) -> Result<()> {
        let ports = self.options.ports;
        let (tx, rx) = mpsc::sync_channel(128);
        let lost = Arc::new(Mutex::new(None));
        let lost_callback = lost.clone();
        self.listener.state(ClinkState::Connecting);
        let a = self.link.connect(Arc::new(move |cause| {
            *lost_callback.lock().unwrap() = Some(cause.unwrap_or_else(|| "link_lost".into()));
        }))?;
        require(!stopped(&self.shared), "session_stopped")?;
        self.shared.snapshot.lock().unwrap().head_unit_id = a.head_unit_id.clone();
        *self.shared.connection.lock().unwrap() = Some(a.clone());
        let origin = Origin {
            output_id: self.options.output_id.clone(),
            client_id: a.head_unit_id.clone(),
        };
        *origin_slot.lock().unwrap() = Some(origin.clone());
        let listener = self.listener.clone();
        let mut touch = UibcDecoder::new(origin.clone())
            .with_view_size(move |width, height| listener.view_size(width, height));
        let mut wfd = WfdSource::new(a.local_ipv4.to_string(), ports);
        for service in [
            ports.control,
            ports.cert,
            ports.media,
            ports.rtsp,
            ports.uibc,
        ] {
            self.listen(service, &a, tx.clone())?;
        }
        let identity = match self.trust.load_identity()? {
            Some(i) => i,
            None => {
                let i = AuthIdentity::generate();
                self.trust.save_identity(&i)?;
                i
            }
        };
        self.listener.state(ClinkState::Authenticating);
        let auth = self.dial(ports.auth)?;
        let (key, peer_key) = self.authenticate(&auth, identity, &lost)?;
        let _ = auth.shutdown();
        a.transport.authenticated()?;
        self.listener.state(ClinkState::Negotiating);
        *self.shared.audio.lock().unwrap() = Some(AudioTransport {
            key,
            sockets: BTreeMap::new(),
        });
        let mut owner = Owner {
            session: self,
            key,
            config_sequence: 0,
            certified: false,
            configured: false,
            peer_key,
            sockets: BTreeMap::new(),
            readers: BTreeMap::new(),
            ciphers: BTreeMap::new(),
            display: control::head_unit_fallback_display(),
            malformed_records: VecDeque::new(),
        };
        let mut rtsp = RtspReader::default();
        let mut input_cipher = SecureRecord::new(&key)?;
        let mut health = LinkHealth::new((self.options.clock)());
        let mut next_heartbeat = (self.options.clock)();
        let deadline = (self.options.clock)() + Duration::from_secs(10);
        let mut started_playing = false;
        while !stopped(&self.shared) && !self.shared.stop_requested.load(Ordering::Acquire) {
            if let Some(c) = lost.lock().unwrap().clone() {
                return Err(c.into());
            }
            let event = receive(&rx)?;
            if owner.sockets.contains_key(&ports.control) {
                self.update_link(
                    &mut health,
                    (self.options.clock)(),
                    matches!(&event, Some(Event::Bytes(_, bytes)) if !bytes.is_empty()),
                )?;
            }
            match event {
                Some(Event::Accepted(service, socket)) => {
                    owner.sockets.insert(service, socket.clone());
                    if let Some(a) = self.shared.audio.lock().unwrap().as_mut() {
                        a.sockets.insert(service, socket.clone());
                    }
                    if service == ports.control || service == ports.cert || service == ports.media {
                        owner.ciphers.insert(service, SecureRecord::new(&key)?);
                        owner.readers.insert(service, FrameReader::default());
                    }
                    if service == ports.control {
                        health = LinkHealth::new(Instant::now());
                        owner.config_sequence = owner.send(service, 1, 24, &[], 1, None, 1)?;
                        let zeros = Fields((1..=4).map(|n| (n, FieldValue::Varint(0))).collect())
                            .encode()?;
                        owner.send(service, 1, 9, &zeros, 0, None, 1)?;
                    } else if service == ports.rtsp {
                        write(&self.shared, &socket, &wfd.start().encode()?)?;
                    }
                }
                Some(Event::Bytes(service, bytes)) if service == ports.rtsp => {
                    for message in rtsp.feed(&bytes)? {
                        let responses = wfd.handle(&message, &owner.display)?;
                        for response in &responses {
                            write(
                                &self.shared,
                                owner.sockets.get(&ports.rtsp).ok_or("missing_rtsp")?,
                                &response.encode()?,
                            )?;
                        }
                        {
                            let mut s = self.shared.snapshot.lock().unwrap();
                            s.mode = wfd.mode;
                            s.decoders = wfd.decoders.clone();
                            s.rtp_port = wfd.rtp_port;
                            s.media_audio = wfd.audio;
                        }
                        owner.report(&wfd);
                        owner.update_play(&wfd);
                        if wfd.take_keyframe() {
                            self.callbacks.keyframe();
                        }
                        if message.line.starts_with("TEARDOWN ")
                            && responses
                                .first()
                                .is_some_and(|m| m.line == "RTSP/1.0 200 OK")
                        {
                            return Err("rtsp_teardown".into());
                        }
                    }
                }
                Some(Event::Bytes(service, bytes)) if service == ports.uibc => {
                    require(owner.certified, "certificate_required")?;
                    for plain in input_cipher.feed(&bytes)? {
                        if let Some(e) = touch.decode(&plain, self.stream_size())? {
                            self.callbacks.control(e);
                        }
                    }
                }
                Some(Event::Bytes(service, bytes)) => {
                    let frames = owner
                        .readers
                        .get_mut(&service)
                        .ok_or("missing_frame_reader")?
                        .feed(&bytes)?;
                    for frame in frames {
                        owner.handle(service, &frame, &wfd, &origin)?;
                    }
                }
                Some(Event::Closed(service, cause)) => {
                    if service != ports.cert || !owner.certified {
                        return Err(cause
                            .unwrap_or_else(|| "vehicle_channel_closed".into())
                            .into());
                    }
                }
                Some(Event::Lost(cause)) => return Err(cause.into()),
                None => {}
            }
            let now = (self.options.clock)();
            if owner.sockets.contains_key(&ports.control) {
                self.update_link(&mut health, now, false)?;
            }
            if owner.sockets.contains_key(&ports.control) && now >= next_heartbeat {
                // A wall-clock stamp the relay never reads back; a clock before 1970 sends zero
                // rather than ending the session.
                let body = Fields(BTreeMap::from([(
                    1,
                    FieldValue::Varint(
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64,
                    ),
                )]))
                .encode()?;
                owner.send(ports.control, 1, 1, &body, 0, None, 1)?;
                next_heartbeat = now + HEARTBEAT_INTERVAL;
            }
            owner.update_microphone()?;
            if self.is_playing() {
                started_playing = true;
            }
            require(started_playing || now < deadline, "projection_timeout")?;
        }
        self.microphone(None)?;
        owner.update_microphone()?;
        Ok(())
    }
}
fn same_mic(a: &Option<MicRequest>, b: &Option<MicRequest>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            a.stream == b.stream
                && a.codec == b.codec
                && a.sample_rate == b.sample_rate
                && a.channels == b.channels
                && a.frame_ms == b.frame_ms
        }
        _ => false,
    }
}
fn audio_fields(values: &[(u32, u64)]) -> Result<Vec<u8>> {
    Fields(
        values
            .iter()
            .map(|(k, v)| (*k, FieldValue::Varint(*v)))
            .collect(),
    )
    .encode()
}
struct Owner<'a> {
    session: &'a ClinkSession,
    key: [u8; 16],
    config_sequence: u32,
    certified: bool,
    configured: bool,
    peer_key: Vec<u8>,
    sockets: BTreeMap<u16, Arc<dyn ClinkStream>>,
    readers: BTreeMap<u16, FrameReader>,
    ciphers: BTreeMap<u16, SecureRecord>,
    display: DisplayInfo,
    malformed_records: VecDeque<Instant>,
}
impl Owner<'_> {
    fn update_microphone(&mut self) -> Result<()> {
        let change = {
            let mut mic = self.session.shared.microphone.lock().unwrap();
            if !self.certified
                || !self.configured
                || !self.sockets.contains_key(&self.session.options.ports.media)
                || same_mic(&mic.desired, &mic.applied)
            {
                return Ok(());
            }
            let previous = mic.applied.clone();
            mic.applied = mic.desired.clone();
            (mic.desired.clone(), previous)
        };
        let (request, previous) = change;
        let r = request.as_ref().or(previous.as_ref());
        self.send(
            self.session.options.ports.control,
            1,
            6,
            &audio_fields(&[
                (1, u64::from(request.is_some())),
                (2, r.map_or(16000, |r| r.sample_rate) as u64),
                (3, r.map_or(1, |r| r.channels) as u64),
                (4, 2),
            ])?,
            3,
            None,
            1,
        )?;
        Ok(())
    }
    fn microphone_frame(&self, body: &[u8], origin: &Origin) -> Result<()> {
        let packets = {
            let mut mic = self.session.shared.microphone.lock().unwrap();
            let Some(r) = mic.desired.clone() else {
                return Ok(());
            };
            if !same_mic(&mic.desired, &mic.applied) {
                return Ok(());
            }
            let sample_bytes = r.channels as usize * 2;
            require(
                !body.is_empty()
                    && body.len().is_multiple_of(sample_bytes)
                    && body.len() / sample_bytes <= r.sample_rate as usize / 10,
                "invalid_microphone_frame",
            )?;
            mic.bytes.extend_from_slice(body);
            let samples = r.sample_rate / 50;
            let size = samples as usize * sample_bytes;
            let mut packets = Vec::new();
            while mic.bytes.len() >= size {
                let data = mic.bytes.drain(..size).collect::<Vec<_>>().into();
                let pts_us = mic
                    .clock
                    .as_mut()
                    .unwrap()
                    .timestamp(samples, oxp_core::monotonic_nanos() / 1000)?;
                packets.push(ControlEvent::MicPacket {
                    stream: r.stream,
                    data,
                    pts_us,
                    origin: origin.clone(),
                });
            }
            packets
        };
        for p in packets {
            let accepted = {
                let mic = self.session.shared.microphone.lock().unwrap();
                mic.desired.is_some() && same_mic(&mic.desired, &mic.applied)
            };
            if accepted {
                self.session.callbacks.control(p);
            }
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)] // One argument per frame header field.
    fn send(
        &mut self,
        service: u16,
        category: u8,
        method: u16,
        body: &[u8],
        kind: u8,
        seq: Option<u32>,
        format: u8,
    ) -> Result<u32> {
        let number = match seq {
            Some(n) => n,
            None => self
                .session
                .shared
                .sequence
                .fetch_add(1, Ordering::AcqRel)
                .wrapping_add(1),
        };
        let socket = self.sockets.get(&service).ok_or("missing_channel")?;
        let frame = Frame {
            sequence: number,
            unix_seconds: now(),
            source: 1,
            format,
            kind,
            category,
            method,
            body: SecureRecord::new(&self.key)?.seal(body)?,
        };
        write(&self.session.shared, socket, &frame.encode()?)?;
        Ok(number)
    }
    fn report(&self, wfd: &WfdSource) {
        self.session.shared.snapshot.lock().unwrap().decoders = wfd.decoders.clone();
        let display = DisplayInfo {
            viewport: Some(Viewport {
                width_px: self.display.width_px,
                height_px: self.display.height_px,
            }),
            fullscreen: Some(true),
            fullscreen_preferred: None,
            ..self.display.clone()
        };
        self.session.callbacks.capabilities(OutputCapabilities {
            prefers_h264: false,
            latency: LatencyClass::Interactive,
            decoders: wfd.decoders.clone(),
            displays: BTreeMap::from([(0, display)]),
            touch: 2,
            keys: KeyCode::all(),
            audio: BTreeSet::from([AudioCodec::AacLc, AudioCodec::PcmS16le]),
            microphone: true,
        });
    }
    fn update_play(&self, wfd: &WfdSource) {
        let next = self.certified && self.configured && wfd.playing;
        let changed = {
            let mut s = self.session.shared.snapshot.lock().unwrap();
            let changed = s.playing != next;
            s.playing = next;
            changed
        };
        if changed {
            self.session.callbacks.play(next);
            if next {
                self.session.listener.state(ClinkState::Playing);
            }
        }
    }
    fn handle(
        &mut self,
        service: u16,
        frame: &Frame,
        wfd: &WfdSource,
        origin: &Origin,
    ) -> Result<()> {
        let ports = self.session.options.ports;
        require(
            frame.source == 0 && frame.format <= 1,
            "invalid_message_origin",
        )?;
        // Some cars acknowledge with an empty plaintext record, including voice_state.
        if frame.category == 7 && frame.kind == 2 && frame.body.is_empty() {
            self.acknowledge(service, frame.sequence);
            return Ok(());
        }
        let cipher = self.ciphers.get_mut(&service).ok_or("missing_cipher")?;
        let body = match cipher.open(&frame.body) {
            Ok(body) => body,
            Err(e) => {
                // Log only record headers; authentication and replay failures still count.
                let record = &frame.body;
                let number = |at| {
                    record
                        .get(at..at + 4)
                        .map(|b| i32::from_be_bytes(b.try_into().unwrap()))
                        .unwrap_or(-1)
                };
                let iv = number(0);
                let size = if iv >= 0 { number(iv as usize + 4) } else { -1 };
                self.session.listener.diagnostic(&format!(
                    "frame record_skipped service={service} category={} method={} kind={} bytes={} iv={iv} cipher={size} reason={e}",
                    frame.category, frame.method, frame.kind, record.len(),
                ));
                let now = (self.session.options.clock)();
                self.malformed_records.push_back(now);
                while self
                    .malformed_records
                    .front()
                    .is_some_and(|at| now.saturating_duration_since(*at) > Duration::from_secs(60))
                {
                    self.malformed_records.pop_front();
                }
                if self.malformed_records.len() > 10 {
                    return Err(e);
                }
                return Ok(());
            }
        };
        if frame.category == 7 {
            require(
                frame.format == 0 && frame.kind == 2 && body.is_empty(),
                "invalid_acknowledgement",
            )?;
            self.acknowledge(service, frame.sequence);
            return Ok(());
        }
        if service == ports.media {
            require(
                self.certified && frame.format == 0 && frame.category == 4 && frame.kind == 0,
                "invalid_media_frame",
            )?;
            if frame.method == 9 {
                self.microphone_frame(&body, origin)?;
            }
            return Ok(());
        }
        if service == ports.control && frame.category == 1 && frame.method == 16 {
            require(self.certified, "certificate_required")?;
            for down in [true, false] {
                self.session.callbacks.control(ControlEvent::Key {
                    code: KeyCode::Voice,
                    down,
                    origin: origin.clone(),
                });
            }
            if frame.kind == 3 {
                self.send(service, 7, 0, &[], 2, Some(frame.sequence), 0)?;
            }
            return Ok(());
        }
        require(frame.format == 1, "invalid_message_format")?;
        if service == ports.control {
            require(frame.category == 1, "invalid_control_category")?;
            match frame.method {
                1 => {
                    Fields::decode(&body)?;
                }
                2 => {
                    let request = Fields::decode(&body)?.number(1)?;
                    self.send(
                        service,
                        1,
                        3,
                        &control::server_response(match request {
                            3 => ports.uibc,
                            2 => ports.media,
                            _ => 0,
                        })?,
                        2,
                        Some(frame.sequence),
                        1,
                    )?;
                }
                24 => {
                    if !body.is_empty() {
                        self.display = control::display(&body, &self.display)?;
                        self.report(wfd);
                    }
                    let mut display = self.display.clone();
                    display.width_px = wfd.mode.width;
                    display.height_px = wfd.mode.height;
                    display.refresh_hz = wfd.mode.fps as f64;
                    self.send(
                        service,
                        1,
                        25,
                        &control::display_body(&display)?,
                        2,
                        Some(frame.sequence),
                        1,
                    )?;
                }
                25 => {
                    require(
                        frame.kind == 2 && frame.sequence == self.config_sequence,
                        "unexpected_configuration",
                    )?;
                    self.display = control::display(&body, &self.display)?;
                    self.configured = true;
                    self.report(wfd);
                }
                4 | 5 => self.session.callbacks.keyframe(),
                23 => {
                    self.session.stop();
                    return Ok(());
                }
                12 => {
                    require(self.certified, "certificate_required")?;
                    let f = Fields::decode(&body)?;
                    let code = f.number(2)?;
                    let action = f.number(1)?;
                    require(
                        code <= 65535 && action <= 2 && f.number(3)? <= 0xffffffff,
                        "invalid_key",
                    )?;
                    if let Some(event) = decode_key(code as u16, action as u8, origin.clone())? {
                        self.session.callbacks.control(event);
                    }
                }
                _ => {}
            }
        } else if service == ports.cert {
            require(
                frame.category == 6 && frame.method == 1 && !self.certified,
                "invalid_certificate_message",
            )?;
            let identity = certificate::check(&body)?;
            self.session.listener.diagnostic(&format!(
                "head_unit_certificate {}",
                serde_json::to_string(&identity)?,
            ));
            self.session
                .trust
                .save_peer_key(&origin.client_id, &self.peer_key)?;
            self.certified = true;
        }
        if frame.kind == 3 {
            self.send(service, 7, 0, &[], 2, Some(frame.sequence), 0)?;
        }
        self.update_play(wfd);
        Ok(())
    }
    fn acknowledge(&self, service: u16, sequence: u32) {
        if let Some((expected, tx)) = self
            .session
            .shared
            .acknowledgements
            .lock()
            .unwrap()
            .get(&sequence)
        {
            if *expected == service {
                let _ = tx.send(());
            }
        }
    }
}
impl Drop for Owner<'_> {
    fn drop(&mut self) {
        let _ = self.session.microphone(None);
        let _ = self.update_microphone();
    }
}
fn now() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as u32
}
fn stopped(shared: &Shared) -> bool {
    shared.stopped.load(Ordering::Acquire)
}
fn spawn(name: String, f: impl FnOnce() + Send + 'static) {
    thread::Builder::new()
        .name(name)
        .spawn(f)
        .expect("Unable to spawn Carlink worker");
}
fn enqueue(shared: &Shared, tx: &SyncSender<Event>, mut event: Event) {
    while !stopped(shared) {
        match tx.try_send(event) {
            Ok(()) => return,
            Err(mpsc::TrySendError::Full(e)) => {
                event = e;
                thread::sleep(Duration::from_millis(100));
            }
            Err(mpsc::TrySendError::Disconnected(_)) => return,
        }
    }
}
fn receive(rx: &Receiver<Event>) -> Result<Option<Event>> {
    match rx.recv_timeout(Duration::from_millis(100)) {
        Ok(e) => Ok(Some(e)),
        Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
        Err(_) => Err("session_workers_closed".into()),
    }
}
fn write(shared: &Shared, socket: &Arc<dyn ClinkStream>, bytes: &[u8]) -> Result<()> {
    require(!stopped(shared), "session_stopped")?;
    let id = Arc::as_ptr(socket) as *const () as usize;
    let lock = shared
        .write_locks
        .lock()
        .unwrap()
        .entry(id)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone();
    let _guard = lock.lock().unwrap();
    shared
        .writes
        .lock()
        .unwrap()
        .insert(id, (socket.clone(), Instant::now()));
    let result = socket.write_all(bytes);
    shared.writes.lock().unwrap().remove(&id);
    Ok(result?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use std::{io::Read, net::TcpStream};
    struct Pin;
    impl PinProvider for Pin {
        fn request_pin(&self, _: &str) -> Option<String> {
            Some("012345".into())
        }
    }
    #[derive(Default)]
    struct Trust {
        saved: Mutex<Vec<Vec<u8>>>,
    }
    impl TrustStore for Trust {
        fn load_identity(&self) -> Result<Option<AuthIdentity>> {
            Ok(None)
        }
        fn save_identity(&self, _: &AuthIdentity) -> Result<()> {
            Ok(())
        }
        fn load_peer_key(&self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn save_peer_key(&self, _: &str, key: &[u8]) -> Result<()> {
            self.saved.lock().unwrap().push(key.to_vec());
            Ok(())
        }
        fn forget_peer(&self, _: &str) -> Result<()> {
            Ok(())
        }
    }
    #[derive(Default)]
    struct Callbacks {
        play: Mutex<Vec<bool>>,
        controls: Mutex<Vec<ControlEvent>>,
    }
    impl SessionCallbacks for Callbacks {
        fn capabilities(&self, _: OutputCapabilities) {}
        fn control(&self, event: ControlEvent) {
            self.controls.lock().unwrap().push(event);
        }
        fn play(&self, p: bool) {
            self.play.lock().unwrap().push(p);
        }
        fn keyframe(&self) {}
    }
    fn pair() -> (Arc<dyn ClinkStream>, TcpStream) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let peer = listener.accept().unwrap().0;
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        (Arc::new(client), peer)
    }
    fn read_frame(socket: &mut TcpStream) -> Frame {
        let mut prefix = [0; 4];
        socket.read_exact(&mut prefix).unwrap();
        let n = u32::from_be_bytes(prefix) as usize;
        let mut b = vec![0; n];
        b[..4].copy_from_slice(&prefix);
        socket.read_exact(&mut b[4..]).unwrap();
        Frame::decode(&b).unwrap()
    }
    fn incoming(
        key: &[u8],
        sequence: u32,
        kind: u8,
        category: u8,
        method: u16,
        body: &[u8],
    ) -> Frame {
        Frame {
            sequence,
            unix_seconds: 0,
            source: 0,
            format: 1,
            kind,
            category,
            method,
            body: SecureRecord::new(key).unwrap().seal(body).unwrap(),
        }
    }
    #[derive(Default)]
    struct StallProbe {
        diagnostics: Mutex<Vec<String>>,
        capabilities: Mutex<Vec<OutputCapabilities>>,
        queue: Mutex<crate::video_delivery::VideoQueue>,
        keyframes: AtomicU32,
        writes: AtomicU32,
    }
    impl ClinkListener for StallProbe {
        fn diagnostic(&self, message: &str) {
            self.diagnostics.lock().unwrap().push(message.into());
        }
    }
    impl SessionCallbacks for StallProbe {
        fn capabilities(&self, caps: OutputCapabilities) {
            self.capabilities.lock().unwrap().push(caps);
        }
        fn control(&self, _: ControlEvent) {}
        fn play(&self, _: bool) {}
        fn keyframe(&self) {
            self.keyframes.fetch_add(1, Ordering::Relaxed);
        }
        fn link_stalled(&self, stalled: bool) {
            self.queue.lock().unwrap().set_stalled(stalled);
        }
    }
    impl ClinkStream for StallProbe {
        fn read(&self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::TimedOut.into())
        }
        fn write_all(&self, _: &[u8]) -> std::io::Result<()> {
            self.writes.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        fn set_read_timeout(&self, _: Option<Duration>) -> std::io::Result<()> {
            Ok(())
        }
        fn shutdown(&self) -> std::io::Result<()> {
            Ok(())
        }
    }
    fn stall_session(probe: Arc<StallProbe>) -> ClinkSession {
        let session = ClinkSession::new(
            Arc::new(StaticClinkLink(ClinkAddresses::new(
                std::net::Ipv4Addr::LOCALHOST,
                std::net::Ipv4Addr::LOCALHOST,
            ))),
            Arc::new(Pin),
            Arc::new(Trust::default()),
            probe.clone(),
            probe.clone(),
            SessionOptions::default(),
        )
        .unwrap();
        session.shared.snapshot.lock().unwrap().playing = true;
        *session.shared.video.lock().unwrap() = Some((session.options.ports.rtp, probe));
        session
    }
    fn record_owner(session: &ClinkSession) -> Owner<'_> {
        Owner {
            session,
            key: [7; 16],
            config_sequence: 0,
            certified: false,
            configured: false,
            peer_key: Vec::new(),
            sockets: BTreeMap::new(),
            readers: BTreeMap::new(),
            ciphers: [session.options.ports.control, session.options.ports.media]
                .into_iter()
                .map(|service| (service, SecureRecord::new(&[7; 16]).unwrap()))
                .collect(),
            display: DisplayInfo::default(),
            malformed_records: VecDeque::new(),
        }
    }
    fn origin() -> Origin {
        Origin {
            output_id: "clink".into(),
            client_id: "hu".into(),
        }
    }
    #[test]
    fn reported_display_uses_the_carlink_window_as_viewport_and_fullscreen() {
        let probe = Arc::new(StallProbe::default());
        let session = stall_session(probe.clone());
        let mut owner = record_owner(&session);
        owner.display = DisplayInfo {
            width_px: 1532,
            height_px: 1013,
            density_dpi: 240,
            refresh_hz: 30.0,
            fullscreen: Some(false),
            fullscreen_preferred: Some(true),
            safe_area: SafeArea {
                left: 3,
                top: 5,
                right: 7,
                bottom: 9,
            },
            ..Default::default()
        };
        let wfd = WfdSource::new("127.0.0.1".into(), session.options.ports);
        owner.report(&wfd);
        let caps = probe.capabilities.lock().unwrap();
        let expected = DisplayInfo {
            viewport: Some(Viewport {
                width_px: 1532,
                height_px: 1013,
            }),
            fullscreen: Some(true),
            fullscreen_preferred: None,
            ..owner.display.clone()
        };
        assert_eq!(caps[0].displays[&0], expected);
        assert_eq!((wfd.mode.width, wfd.mode.height), (1280, 720));
        assert_eq!(session.stream_size(), (1280, 720));
    }
    #[test]
    fn empty_plaintext_and_sealed_acknowledgements_complete_only_the_matching_service() {
        let session = stall_session(Arc::new(StallProbe::default()));
        let ports = session.options.ports;
        let mut owner = record_owner(&session);
        let wfd = WfdSource::new("127.0.0.1".into(), ports);
        let (tx, rx) = mpsc::channel();
        session
            .shared
            .acknowledgements
            .lock()
            .unwrap()
            .insert(42, (ports.control, tx));
        let mut ack = incoming(&[7; 16], 42, 2, 7, 0, &[]);
        ack.body.clear();
        owner.handle(ports.media, &ack, &wfd, &origin()).unwrap();
        assert!(rx.try_recv().is_err());
        // Kotlin accepts the plaintext exception in either supported format.
        for format in [0, 1] {
            ack.format = format;
            owner.handle(ports.control, &ack, &wfd, &origin()).unwrap();
            rx.try_recv().unwrap();
        }
        ack = incoming(&[7; 16], 42, 2, 7, 0, &[]);
        ack.format = 0;
        owner.handle(ports.control, &ack, &wfd, &origin()).unwrap();
        rx.try_recv().unwrap();
        ack.source = 1;
        ack.body.clear();
        assert!(owner.handle(ports.control, &ack, &wfd, &origin()).is_err());
        ack.source = 0;
        ack.format = 2;
        assert!(owner.handle(ports.control, &ack, &wfd, &origin()).is_err());
        assert!(owner.malformed_records.is_empty());
    }
    #[test]
    fn malformed_record_window_allows_ten_and_expires_only_after_sixty_seconds() {
        for (elapsed, fails) in [
            (Duration::from_secs(60), true),
            (Duration::from_secs(60) + Duration::from_nanos(1), false),
        ] {
            let probe = Arc::new(StallProbe::default());
            let mut session = stall_session(probe.clone());
            let start = Instant::now();
            let clock = Arc::new(Mutex::new(start));
            let time = clock.clone();
            session.options.clock = Arc::new(move || *time.lock().unwrap());
            let ports = session.options.ports;
            let mut owner = record_owner(&session);
            let wfd = WfdSource::new("127.0.0.1".into(), ports);
            let valid = incoming(&[7; 16], 1, 0, 1, 1, &[]);
            owner
                .handle(ports.control, &valid, &wfd, &origin())
                .unwrap();
            // A replay must be skipped and counted, never dispatched a second time.
            owner
                .handle(ports.control, &valid, &wfd, &origin())
                .unwrap();
            for i in 1..10 {
                let mut bad = incoming(&[7; 16], i, 0, 1, 1, &[]);
                if i % 2 == 0 {
                    bad.body.truncate(3);
                } else {
                    *bad.body.last_mut().unwrap() ^= 1;
                }
                owner.handle(ports.control, &bad, &wfd, &origin()).unwrap();
            }
            assert_eq!(owner.malformed_records.len(), 10);
            // A good frame neither consumes the budget nor clears recent failures.
            owner
                .handle(
                    ports.control,
                    &incoming(&[7; 16], 20, 0, 1, 1, &[]),
                    &wfd,
                    &origin(),
                )
                .unwrap();
            *clock.lock().unwrap() = start + elapsed;
            let mut bad = valid;
            bad.body = vec![0; 4];
            let result = owner.handle(ports.control, &bad, &wfd, &origin());
            assert_eq!(result.is_err(), fails);
            if let Err(e) = result {
                assert_eq!(e.to_string(), "invalid_cipher_record");
            }
            assert_eq!(owner.malformed_records.len(), if fails { 11 } else { 1 });
            let messages = probe.diagnostics.lock().unwrap();
            assert_eq!(messages.len(), 11);
            assert!(messages[0].ends_with("reason=replayed_cipher_record"));
            assert!(messages
                .iter()
                .any(|m| m.ends_with("reason=cipher_authentication_failed")));
            assert!(messages
                .iter()
                .all(|m| m.starts_with("frame record_skipped service=")));
        }
    }
    #[test]
    fn control_m4_and_m5_request_keyframes_and_acknowledge_sync_messages() {
        let probe = Arc::new(StallProbe::default());
        let session = stall_session(probe.clone());
        let ports = session.options.ports;
        let mut owner = record_owner(&session);
        let (socket, mut peer) = pair();
        owner.sockets.insert(ports.control, socket);
        let wfd = WfdSource::new("127.0.0.1".into(), ports);
        for method in [4, 5] {
            owner
                .handle(
                    ports.control,
                    &incoming(&[7; 16], method as u32, 3, 1, method, &[]),
                    &wfd,
                    &origin(),
                )
                .unwrap();
            let ack = read_frame(&mut peer);
            assert_eq!(
                (ack.sequence, ack.category, ack.kind),
                (method as u32, 7, 2)
            );
        }
        assert_eq!(probe.keyframes.load(Ordering::Relaxed), 2);
    }
    #[test]
    fn ten_second_stall_drops_video_and_recovers_once_with_keyframe() {
        let probe = Arc::new(StallProbe::default());
        let session = stall_session(probe.clone());
        let start = Instant::now();
        let mut health = LinkHealth::new(start);
        let frame = |key_frame| VideoFrame {
            screen: 0,
            session: 1,
            data: Arc::from([1u8]),
            pts_us: 0,
            key_frame,
            received_at_nanos: 0,
        };
        probe.queue.lock().unwrap().offer(frame(true));
        assert!(session.send_rtp(&[1]).unwrap());
        for second in 1..=10 {
            session
                .update_link(&mut health, start + Duration::from_secs(second), false)
                .unwrap();
            if second >= 5 {
                assert!(session.is_stalled());
                assert!(!session.send_rtp(&[2]).unwrap());
                let mut queue = probe.queue.lock().unwrap();
                queue.offer(frame(true));
                assert!(queue.poll().is_none());
            }
        }
        assert!(session.is_playing());
        assert_eq!(probe.writes.load(Ordering::Relaxed), 1);
        session
            .update_link(&mut health, start + Duration::from_secs(10), true)
            .unwrap();
        session
            .update_link(&mut health, start + Duration::from_secs(11), true)
            .unwrap();
        assert!(!session.is_stalled());
        assert_eq!(probe.keyframes.load(Ordering::Relaxed), 1);
        assert_eq!(
            *probe.diagnostics.lock().unwrap(),
            ["link stalled ms=5000", "link recovered after ms=10000"]
        );
        let mut queue = probe.queue.lock().unwrap();
        queue.offer(frame(false));
        assert!(queue.poll().is_none());
        queue.offer(frame(true));
        assert!(queue.poll().is_some());
        assert!(session.send_rtp(&[3]).unwrap());
        drop(queue);
        session
            .update_link(&mut health, start + Duration::from_secs(16), false)
            .unwrap();
        assert_eq!(
            probe.diagnostics.lock().unwrap().last().unwrap(),
            "link stalled ms=5000"
        );
    }
    #[test]
    fn sixteen_seconds_without_traffic_times_out_even_if_bytes_arrive_late() {
        for received in [false, true] {
            let probe = Arc::new(StallProbe::default());
            let session = stall_session(probe);
            let start = Instant::now();
            let mut health = LinkHealth::new(start);
            session
                .update_link(&mut health, start + Duration::from_secs(5), false)
                .unwrap();
            session
                .update_link(&mut health, start + Duration::from_secs(15), false)
                .unwrap();
            let error = session
                .update_link(&mut health, start + Duration::from_secs(16), received)
                .unwrap_err();
            assert_eq!(error.to_string(), "heartbeat_timeout");
        }
    }

    #[test]
    fn control_cert_sync_ack_trust_and_play_gates() {
        let trust = Arc::new(Trust::default());
        let callbacks = Arc::new(Callbacks::default());
        let probe = Arc::new(StallProbe::default());
        let session = ClinkSession::new(
            Arc::new(StaticClinkLink(ClinkAddresses::new(
                std::net::Ipv4Addr::LOCALHOST,
                std::net::Ipv4Addr::LOCALHOST,
            ))),
            Arc::new(Pin),
            trust.clone(),
            probe.clone(),
            callbacks.clone(),
            SessionOptions::default(),
        )
        .unwrap();
        let ports = session.options.ports;
        let key = [7; 16];
        let (control_socket, mut control_peer) = pair();
        let (cert_socket, mut cert_peer) = pair();
        let mut owner = Owner {
            session: &session,
            key,
            config_sequence: 77,
            certified: false,
            configured: false,
            peer_key: vec![9; 64],
            sockets: BTreeMap::from([(ports.control, control_socket), (ports.cert, cert_socket)]),
            readers: BTreeMap::new(),
            ciphers: BTreeMap::from([
                (ports.control, SecureRecord::new(&key).unwrap()),
                (ports.cert, SecureRecord::new(&key).unwrap()),
            ]),
            display: DisplayInfo::default(),
            malformed_records: VecDeque::new(),
        };
        let mut wfd = WfdSource::new("127.0.0.1".into(), ports);
        wfd.playing = true;
        let origin = Origin {
            output_id: "clink".into(),
            client_id: "hu".into(),
        };
        let display = DisplayInfo {
            width_px: 1072,
            height_px: 2320,
            density_dpi: 320,
            ..Default::default()
        };
        let body = control::display_body(&display).unwrap();
        owner
            .handle(
                ports.control,
                &incoming(&key, 55, 3, 1, 24, &body),
                &wfd,
                &origin,
            )
            .unwrap();
        let response = read_frame(&mut control_peer);
        assert_eq!(
            (55, 1, 25, 2),
            (
                response.sequence,
                response.category,
                response.method,
                response.kind
            )
        );
        let plain = SecureRecord::new(&key)
            .unwrap()
            .open(&response.body)
            .unwrap();
        let d = control::display(&plain, &DisplayInfo::default()).unwrap();
        assert_eq!((1280, 720, 320), (d.width_px, d.height_px, d.density_dpi));
        let ack = read_frame(&mut control_peer);
        assert_eq!(
            (55, 7, 0, 2, 0),
            (ack.sequence, ack.category, ack.method, ack.kind, ack.format)
        );
        assert!(SecureRecord::new(&key)
            .unwrap()
            .open(&ack.body)
            .unwrap()
            .is_empty());
        assert!(owner
            .handle(
                ports.control,
                &incoming(&key, 76, 2, 1, 25, &body),
                &wfd,
                &origin
            )
            .is_err());
        owner
            .handle(
                ports.control,
                &incoming(&key, 77, 2, 1, 25, &body),
                &wfd,
                &origin,
            )
            .unwrap();
        assert!(!session.is_playing());
        assert!(trust.saved.lock().unwrap().is_empty());
        let malformed = Fields(BTreeMap::from([(1, FieldValue::Bytes(b"bad!".to_vec()))]))
            .encode()
            .unwrap();
        assert!(owner
            .handle(
                ports.cert,
                &incoming(&key, 80, 3, 6, 1, &malformed),
                &wfd,
                &origin
            )
            .is_err());
        assert!(trust.saved.lock().unwrap().is_empty());
        let metadata=br#"{"short_name":"car","product_name":"model","vendor_name":"vendor","vendor_id":"v1","product_id":"p1"}"#;
        let envelope = serde_json::json!({"content":metadata.iter().map(|b|*b as i8).collect::<Vec<_>>(),"signature":vec![0;256],"version":1,"signAlgorithm":"SHA256withRSA","cryptoConfigIndex":"v1"});
        let cert = Fields(BTreeMap::from([(
            1,
            FieldValue::Bytes(
                base64::engine::general_purpose::STANDARD
                    .encode(envelope.to_string())
                    .into_bytes(),
            ),
        )]))
        .encode()
        .unwrap();
        owner
            .handle(
                ports.cert,
                &incoming(&key, 81, 3, 6, 1, &cert),
                &wfd,
                &origin,
            )
            .unwrap();
        assert_eq!(vec![vec![9; 64]], *trust.saved.lock().unwrap());
        let messages = probe.diagnostics.lock().unwrap();
        assert_eq!(messages.len(), 1);
        let identity: serde_json::Value =
            serde_json::from_str(messages[0].strip_prefix("head_unit_certificate ").unwrap())
                .unwrap();
        assert_eq!(
            identity,
            serde_json::json!({"short_name":"car","product_name":"model","vendor_name":"vendor","vendor_id":"v1","product_id":"p1"})
        );
        drop(messages);
        assert!(owner
            .handle(
                ports.cert,
                &incoming(&key, 81, 3, 6, 1, &cert),
                &wfd,
                &origin
            )
            .is_err());
        assert_eq!(probe.diagnostics.lock().unwrap().len(), 1);
        assert!(session.is_playing());
        assert_eq!(vec![true], *callbacks.play.lock().unwrap());
        let ack = read_frame(&mut cert_peer);
        assert_eq!(
            (81, 7, 2, 0),
            (ack.sequence, ack.category, ack.kind, ack.format)
        );
        let request = Fields(BTreeMap::from([(1, FieldValue::Varint(3))]))
            .encode()
            .unwrap();
        owner
            .handle(
                ports.control,
                &incoming(&key, 82, 1, 1, 2, &request),
                &wfd,
                &origin,
            )
            .unwrap();
        let response = read_frame(&mut control_peer);
        let body = SecureRecord::new(&key)
            .unwrap()
            .open(&response.body)
            .unwrap();
        let fields = Fields::decode(&body).unwrap();
        assert_eq!(1, fields.number(1).unwrap());
        assert_eq!(ports.uibc as u64, fields.number(2).unwrap());
        let key_body = Fields(BTreeMap::from([
            (1, FieldValue::Varint(0)),
            (2, FieldValue::Varint(66)),
        ]))
        .encode()
        .unwrap();
        owner
            .handle(
                ports.control,
                &incoming(&key, 83, 0, 1, 12, &key_body),
                &wfd,
                &origin,
            )
            .unwrap();
        assert_eq!(
            vec![ControlEvent::Key {
                code: KeyCode::Select,
                down: true,
                origin: origin.clone()
            }],
            *callbacks.controls.lock().unwrap()
        );
        let mut ack = incoming(&key, 84, 2, 7, 0, &[]);
        ack.format = 0;
        owner.handle(ports.control, &ack, &wfd, &origin).unwrap();
        ack = incoming(&key, 85, 3, 7, 0, &[]);
        ack.format = 0;
        assert!(owner.handle(ports.control, &ack, &wfd, &origin).is_err());
        owner
            .handle(
                ports.control,
                &incoming(&key, 86, 0, 1, 23, &[]),
                &wfd,
                &origin,
            )
            .unwrap();
        assert!(session.shared.stop_requested.load(Ordering::Acquire));
    }
}

#[cfg(test)]
mod audio_tests {
    use super::*;
    use crate::voice_delivery::VoiceDelivery;
    use std::sync::atomic::AtomicU64;
    use std::{io::Read, net::TcpStream};
    struct Pin;
    impl PinProvider for Pin {
        fn request_pin(&self, _: &str) -> Option<String> {
            Some("012345".into())
        }
    }
    struct Trust;
    impl TrustStore for Trust {
        fn load_identity(&self) -> Result<Option<AuthIdentity>> {
            Ok(None)
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
    #[derive(Default)]
    struct Callbacks {
        events: Mutex<Vec<ControlEvent>>,
    }
    impl SessionCallbacks for Callbacks {
        fn capabilities(&self, _: OutputCapabilities) {}
        fn control(&self, e: ControlEvent) {
            self.events.lock().unwrap().push(e);
        }
        fn play(&self, _: bool) {}
        fn keyframe(&self) {}
    }
    fn session(callbacks: Arc<Callbacks>) -> Arc<ClinkSession> {
        Arc::new(
            ClinkSession::new(
                Arc::new(StaticClinkLink(ClinkAddresses::new(
                    std::net::Ipv4Addr::LOCALHOST,
                    std::net::Ipv4Addr::LOCALHOST,
                ))),
                Arc::new(Pin),
                Arc::new(Trust),
                Arc::new(()),
                callbacks,
                SessionOptions::default(),
            )
            .unwrap(),
        )
    }
    fn owner(session: &ClinkSession, key: [u8; 16]) -> Owner<'_> {
        Owner {
            session,
            key,
            config_sequence: 0,
            certified: true,
            configured: true,
            peer_key: Vec::new(),
            sockets: BTreeMap::new(),
            readers: BTreeMap::new(),
            ciphers: BTreeMap::from([
                (
                    session.options.ports.media,
                    SecureRecord::new(&key).unwrap(),
                ),
                (
                    session.options.ports.control,
                    SecureRecord::new(&key).unwrap(),
                ),
            ]),
            display: DisplayInfo::default(),
            malformed_records: VecDeque::new(),
        }
    }
    fn incoming(
        key: &[u8],
        sequence: u32,
        category: u8,
        method: u16,
        kind: u8,
        format: u8,
        body: &[u8],
    ) -> Frame {
        Frame {
            sequence,
            unix_seconds: 0,
            source: 0,
            format,
            kind,
            category,
            method,
            body: SecureRecord::new(key).unwrap().seal(body).unwrap(),
        }
    }
    fn origin() -> Origin {
        Origin {
            output_id: "clink".into(),
            client_id: "sim".into(),
        }
    }
    #[test]
    fn raw_microphone_split_join_limits_stop_and_voice_edges() {
        let callbacks = Arc::new(Callbacks::default());
        let session = session(callbacks.clone());
        let key = [7; 16];
        let mut owner = owner(&session, key);
        let ports = session.options.ports;
        let wfd = WfdSource::new("127.0.0.1".into(), ports);
        let request = MicRequest {
            stream: 2,
            codec: AudioCodec::PcmS16le,
            sample_rate: 24000,
            channels: 2,
            frame_ms: 20,
        };
        session.microphone(Some(request.clone())).unwrap();
        session.shared.microphone.lock().unwrap().applied = Some(request);
        owner
            .handle(
                ports.media,
                &incoming(&key, 1, 4, 9, 0, 0, &vec![7; 960]),
                &wfd,
                &origin(),
            )
            .unwrap();
        assert!(callbacks.events.lock().unwrap().is_empty());
        owner
            .handle(
                ports.media,
                &incoming(&key, 2, 4, 9, 0, 0, &vec![9; 4800]),
                &wfd,
                &origin(),
            )
            .unwrap();
        let e = callbacks.events.lock().unwrap();
        assert_eq!(e.len(), 3);
        let mut timestamps = Vec::new();
        for (i, e) in e.iter().enumerate() {
            let ControlEvent::MicPacket {
                stream,
                data,
                pts_us,
                origin: o,
            } = e
            else {
                panic!()
            };
            assert_eq!(*stream, 2);
            assert_eq!(o, &origin());
            assert_eq!(data.len(), 1920);
            if i == 0 {
                assert_eq!(&data[..960], vec![7; 960]);
                assert_eq!(&data[960..], vec![9; 960]);
            } else {
                assert_eq!(data.as_ref(), vec![9; 1920]);
            }
            timestamps.push(*pts_us);
        }
        assert_eq!(timestamps[1] - timestamps[0], 20000);
        assert_eq!(timestamps[2] - timestamps[1], 20000);
        drop(e);
        for size in [1, 9604] {
            assert!(owner
                .handle(
                    ports.media,
                    &incoming(&key, 3, 4, 9, 0, 0, &vec![0; size]),
                    &wfd,
                    &origin()
                )
                .is_err());
        }
        assert!(owner
            .handle(
                ports.media,
                &incoming(&key, 4, 4, 9, 0, 1, &[0; 4]),
                &wfd,
                &origin()
            )
            .is_err());
        session.microphone(None).unwrap();
        owner
            .handle(
                ports.media,
                &incoming(&key, 5, 4, 9, 0, 0, &[0; 4]),
                &wfd,
                &origin(),
            )
            .unwrap();
        assert_eq!(callbacks.events.lock().unwrap().len(), 3);
        for format in [0, 1] {
            owner
                .handle(
                    ports.control,
                    &incoming(&key, 6, 1, 16, 0, format, &[1, 2, 3]),
                    &wfd,
                    &origin(),
                )
                .unwrap();
        }
        let e = callbacks.events.lock().unwrap();
        assert_eq!(
            &e[3..],
            &[true, false, true, false].map(|down| ControlEvent::Key {
                code: KeyCode::Voice,
                down,
                origin: origin()
            })
        );
    }
    fn pair() -> (Arc<dyn ClinkStream>, TcpStream) {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let a = TcpStream::connect(l.local_addr().unwrap()).unwrap();
        let b = l.accept().unwrap().0;
        b.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        (Arc::new(a), b)
    }
    fn read_frame(peer: &mut TcpStream) -> Frame {
        let mut p = [0; 4];
        peer.read_exact(&mut p).unwrap();
        let mut b = vec![0; u32::from_be_bytes(p) as usize];
        b[..4].copy_from_slice(&p);
        peer.read_exact(&mut b[4..]).unwrap();
        Frame::decode(&b).unwrap()
    }
    /// A minimal authenticated peer exercises the real encrypted sender and ACK matching.
    #[test]
    fn voice_tail_ack_delay_media_port_and_microphone_command() {
        voice_tail_delivery(false);
    }
    #[test]
    fn voice_format_replacement_drains_pending_pcm_before_immediate_null() {
        voice_tail_delivery(true);
    }
    fn voice_tail_delivery(replace: bool) {
        let callbacks = Arc::new(Callbacks::default());
        let s = session(callbacks);
        let ports = s.options.ports;
        let key = [8; 16];
        let (control, mut cp) = pair();
        let (media, mut mp) = pair();
        let (video, mut vp) = pair();
        s.shared.snapshot.lock().unwrap().playing = true;
        *s.shared.video.lock().unwrap() = Some((ports.rtp, video));
        *s.shared.audio.lock().unwrap() = Some(AudioTransport {
            key,
            sockets: BTreeMap::from([
                (ports.control, control.clone()),
                (ports.media, media.clone()),
            ]),
        });
        let mut endpoint = owner(&s, key);
        endpoint.sockets = BTreeMap::from([(ports.control, control), (ports.media, media)]);
        let mut wfd = WfdSource::new("127.0.0.1".into(), ports);
        wfd.playing = true;
        endpoint
            .handle(
                ports.control,
                &incoming(&key, 77, 1, 2, 1, 1, &audio_fields(&[(1, 2)]).unwrap()),
                &wfd,
                &origin(),
            )
            .unwrap();
        let f = read_frame(&mut cp);
        let b = SecureRecord::new(&key).unwrap().open(&f.body).unwrap();
        assert_eq!(
            Fields::decode(&b).unwrap().number(2).unwrap(),
            ports.media as u64
        );
        let req = MicRequest {
            stream: 2,
            codec: AudioCodec::PcmS16le,
            sample_rate: 16000,
            channels: 1,
            frame_ms: 20,
        };
        s.microphone(Some(req)).unwrap();
        endpoint.update_microphone().unwrap();
        let f = read_frame(&mut cp);
        assert_eq!((f.method, f.kind), (6, 3));
        let b = SecureRecord::new(&key).unwrap().open(&f.body).unwrap();
        assert_eq!(Fields::decode(&b).unwrap().number(1).unwrap(), 1);
        s.microphone(None).unwrap();
        endpoint.update_microphone().unwrap();
        let f = read_frame(&mut cp);
        let b = SecureRecord::new(&key).unwrap().open(&f.body).unwrap();
        assert_eq!(Fields::decode(&b).unwrap().number(1).unwrap(), 0);
        let clock = Instant::now();
        let commands = Arc::new(Mutex::new(Vec::<(Vec<u8>, u64)>::new()));
        let frames = Arc::new(Mutex::new(Vec::<(u8, Vec<u8>)>::new()));
        let ack_at = Arc::new(AtomicU64::new(0));
        let active = Arc::new(AtomicBool::new(true));
        let (tail_tx, tail_rx) = mpsc::channel();
        let cs = s.clone();
        let cc = commands.clone();
        let ca = active.clone();
        let control_peer = thread::spawn(move || {
            let mut owner = owner(&cs, key);
            let wfd = WfdSource::new("127.0.0.1".into(), ports);
            for _ in 0..2 {
                let f = read_frame(&mut cp);
                assert_eq!((f.category, f.method, f.kind, f.format), (1, 8, 3, 1));
                let b = SecureRecord::new(&key).unwrap().open(&f.body).unwrap();
                cc.lock()
                    .unwrap()
                    .push((b, clock.elapsed().as_nanos() as u64));
                owner
                    .handle(
                        ports.control,
                        &incoming(&key, f.sequence, 7, 0, 2, 0, &[]),
                        &wfd,
                        &origin(),
                    )
                    .unwrap();
            }
            ca.store(false, Ordering::Release);
        });
        let ms = s.clone();
        let mf = frames.clone();
        let ma = ack_at.clone();
        let media_peer = thread::spawn(move || {
            let mut owner = owner(&ms, key);
            let wfd = WfdSource::new("127.0.0.1".into(), ports);
            for _ in 0..6 {
                let f = read_frame(&mut mp);
                assert_eq!((f.category, f.method, f.format), (4, 3, 0));
                let b = SecureRecord::new(&key).unwrap().open(&f.body).unwrap();
                mf.lock().unwrap().push((f.kind, b));
                if f.kind == 3 {
                    tail_tx.send(()).unwrap();
                    thread::sleep(Duration::from_millis(150));
                    ma.store(clock.elapsed().as_nanos() as u64, Ordering::Release);
                    owner
                        .handle(
                            ports.media,
                            &incoming(&key, f.sequence, 7, 0, 2, 0, &[]),
                            &wfd,
                            &origin(),
                        )
                        .unwrap();
                }
            }
        });
        let voice = Arc::new(VoiceDelivery::default());
        let format = Arc::new(AudioFormat {
            stream: 1,
            role: AudioRole::Voice,
            codec: AudioCodec::PcmS16le,
            sample_rate: 16000,
            channels: 1,
            config: None,
        });
        voice.format(1, Some(format.clone()));
        let data = (0..320 * 8)
            .flat_map(|_| 1234i16.to_le_bytes())
            .collect::<Vec<_>>();
        if replace {
            voice.packet(AudioPacket {
                stream: 1,
                session: 1,
                data: data[..data.len() / 2].into(),
                pts_us: 0,
            });
            voice.format(
                1,
                Some(Arc::new(AudioFormat {
                    role: AudioRole::Telephony,
                    ..format.as_ref().clone()
                })),
            );
            voice.packet(AudioPacket {
                stream: 1,
                session: 1,
                data: data[data.len() / 2..].into(),
                pts_us: 80_000,
            });
        } else {
            voice.packet(AudioPacket {
                stream: 1,
                session: 1,
                data: data.clone().into(),
                pts_us: 0,
            });
        }
        voice.format(1, None);
        let vs = s.clone();
        let va = active.clone();
        let worker = thread::spawn(move || {
            voice
                .run(
                    || va.load(Ordering::Acquire),
                    || Some(vs.clone()),
                    |p| p.session == 1,
                )
                .unwrap()
        });
        tail_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(s.send_rtp(&[1, 2, 3, 4]).unwrap());
        let mut b = [0; 4];
        vp.read_exact(&mut b).unwrap();
        assert_eq!(b, [1, 2, 3, 4]);
        worker.join().unwrap();
        control_peer.join().unwrap();
        media_peer.join().unwrap();
        let c = commands.lock().unwrap();
        assert_eq!(
            c.iter()
                .map(|(b, _)| Fields::decode(b).unwrap().number(2).unwrap())
                .collect::<Vec<_>>(),
            [0, 1]
        );
        assert!(c[1].1 - ack_at.load(Ordering::Acquire) >= 20_000_000);
        let f = frames.lock().unwrap();
        assert_eq!(
            f.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
            [0, 0, 0, 0, 0, 3]
        );
        assert_eq!(f.first().unwrap().1.len(), 1920);
        assert_eq!(f.last().unwrap().1.len(), 640);
        assert_eq!(
            f.iter().flat_map(|(_, b)| b.clone()).collect::<Vec<_>>(),
            data
        );
    }
}
