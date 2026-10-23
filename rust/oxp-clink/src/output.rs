// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use crate::{
    interfaces::*,
    media::{RtpPacketizer, RtpSender, RtpUnit, RtpWrite},
    model::*,
    placeholder::{BundledHevcPlaceholderVideo, BundledPlaceholderVideo, PlaceholderVideo},
    require,
    session::{ClinkSession, SessionCallbacks, SessionOptions},
    trace::{next_session, now_nanos, ClinkMediaTap, ClinkTrace, ClinkTraceEvent},
    video_delivery::{VideoQueue, VideoSwitch},
    voice_delivery::voice_role,
    Result,
};
use oxp_media::{
    nal::{self, Format},
    transcode_policy::CodecProfile,
    ts::VideoConfig,
    AvcConfig, HevcConfig, TsMuxer,
};
use std::{
    cell::RefCell,
    collections::{BTreeSet, VecDeque},
    rc::Rc,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, Weak,
    },
    thread,
    time::{Duration, Instant},
};
struct Handoff {
    input_session: Option<i64>,
    format: Option<Arc<VideoFormat>>,
    revision: u64,
    playing: bool,
    request_keyframe: bool,
    queue: VideoQueue,
    media_formats: Vec<(u32, Arc<AudioFormat>)>,
    media_stream: Option<u32>,
    audio: VecDeque<AudioPacket>,
    audio_bytes: usize,
    voice_streams: BTreeSet<u32>,
    microphone: Option<MicRequest>,
    diagnostics: VecDeque<String>,
    clock_mix: ClockMix,
}
impl Handoff {
    fn diagnostic(&mut self, message: String) {
        if self.diagnostics.len() < 32 {
            self.diagnostics.push_back(message);
        }
    }
}
struct RunTrace {
    sink: Arc<dyn ClinkTrace>,
    session: u64,
    rtp_packets: AtomicU64,
    rtp_bytes: AtomicU64,
    queue_overflows: AtomicU64,
}
impl RunTrace {
    fn video(&self, event: &str, source: &str) {
        self.sink.record(ClinkTraceEvent::Video {
            at_nanos: now_nanos(),
            session: self.session,
            event: event.into(),
            source: source.into(),
            codec: None,
            width: None,
            height: None,
            sps: Vec::new(),
            pps: Vec::new(),
        });
    }
    fn error(&self, error: &dyn std::fmt::Display) {
        // Rust has no exception class; use stable, bounded error display text.
        self.sink.record(ClinkTraceEvent::Error {
            at_nanos: now_nanos(),
            session: self.session,
            code: "output_failed".into(),
            exception_class: error.to_string().chars().take(120).collect(),
            location: "ClinkOutput".into(),
        });
    }
}
struct Run {
    trace: Option<Arc<RunTrace>>,
    active: AtomicBool,
    sink: Arc<dyn OutputSink>,
    handoff: Mutex<Handoff>,
    session: Mutex<Option<Arc<ClinkSession>>>,
    voice: crate::voice_delivery::VoiceDelivery,
    sender: RtpSender,
}
impl Run {
    fn new(sink: Arc<dyn OutputSink>) -> Self {
        Self {
            trace: None,
            active: AtomicBool::new(true),
            sink,
            handoff: Mutex::new(Handoff {
                input_session: None,
                format: None,
                revision: 0,
                playing: false,
                request_keyframe: false,
                queue: VideoQueue::default(),
                media_formats: Vec::new(),
                media_stream: None,
                audio: VecDeque::new(),
                audio_bytes: 0,
                voice_streams: BTreeSet::new(),
                microphone: None,
                diagnostics: VecDeque::new(),
                clock_mix: ClockMix::default(),
            }),
            session: Mutex::new(None),
            voice: Default::default(),
            sender: RtpSender::default(),
        }
    }
}
struct Callbacks {
    run: Weak<Run>,
}
impl SessionCallbacks for Callbacks {
    fn capabilities(&self, c: OutputCapabilities) {
        if let Some(run) = self.run.upgrade() {
            with_sink(&run, |sink| sink.capabilities(c));
        }
    }
    fn control(&self, e: ControlEvent) {
        if let Some(run) = self.run.upgrade() {
            let accepted = match &e {
                ControlEvent::MicPacket { stream, .. } => run
                    .handoff
                    .lock()
                    .unwrap()
                    .microphone
                    .as_ref()
                    .is_some_and(|r| r.stream == *stream),
                _ => true,
            };
            if accepted {
                with_sink(&run, |sink| sink.control(e));
            }
        }
    }
    fn play(&self, p: bool) {
        if let Some(run) = self.run.upgrade() {
            let mut h = run.handoff.lock().unwrap();
            h.playing = p;
            h.queue.reset();
            h.revision = h.revision.wrapping_add(1);
            if p {
                h.request_keyframe = true;
            }
        }
    }
    fn link_stalled(&self, stalled: bool) {
        if let Some(run) = self.run.upgrade() {
            let mut h = run.handoff.lock().unwrap();
            h.queue.set_stalled(stalled);
            h.audio.clear();
            h.audio_bytes = 0;
            h.revision = h.revision.wrapping_add(1);
        }
    }
    fn keyframe(&self) {
        if let Some(run) = self.run.upgrade() {
            run.handoff.lock().unwrap().request_keyframe = true;
        }
    }
}
/// Callbacks retain immutable shared frames in a bounded queue. Reopen after ENDED.
pub struct ClinkOutput {
    link: Arc<dyn ClinkLink>,
    pin: Arc<dyn PinProvider>,
    trust: Arc<dyn TrustStore>,
    placeholder: Arc<dyn PlaceholderVideo>,
    listener: Arc<dyn ClinkListener>,
    options: SessionOptions,
    current: Arc<Mutex<Option<Arc<Run>>>>,
    opened: Arc<AtomicBool>,
}
impl ClinkOutput {
    pub fn new(
        link: Arc<dyn ClinkLink>,
        pin: Arc<dyn PinProvider>,
        trust: Arc<dyn TrustStore>,
    ) -> Result<Self> {
        Ok(Self {
            link,
            pin,
            trust,
            placeholder: Arc::new(BundledPlaceholderVideo::new()?),
            listener: Arc::new(()),
            options: Default::default(),
            current: Arc::new(Mutex::new(None)),
            opened: Arc::new(AtomicBool::new(false)),
        })
    }
    pub fn with_options(mut self, options: SessionOptions) -> Result<Self> {
        options.ports.validate()?;
        self.options = options;
        Ok(self)
    }
    pub fn with_listener(mut self, listener: Arc<dyn ClinkListener>) -> Self {
        self.listener = listener;
        self
    }
    pub fn with_trace(mut self, trace: Arc<dyn ClinkTrace>) -> Self {
        self.options.trace = Some(trace);
        self
    }
    pub fn with_media_tap(mut self, tap: Arc<dyn ClinkMediaTap>) -> Self {
        self.options.media_tap = Some(tap);
        self
    }
    pub fn with_placeholder(mut self, placeholder: Arc<dyn PlaceholderVideo>) -> Self {
        self.placeholder = placeholder;
        self
    }
    pub fn open(&self, sink: Arc<dyn OutputSink>) -> Result<()> {
        require(
            self.opened
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok(),
            "output_already_open",
        )?;
        let mut run = Run::new(sink);
        let mut options = self.options.clone();
        if let Some(sink) = &options.trace {
            let session = next_session();
            options.trace_session = Some(session);
            sink.record(ClinkTraceEvent::Link {
                at_nanos: now_nanos(),
                session,
                stage: "hevc_mode".into(),
                fields: [(
                    "mode".into(),
                    match options.hevc {
                        ClinkHevcMode::Auto => "AUTO",
                        ClinkHevcMode::Off => "OFF",
                    }
                    .into(),
                )]
                .into(),
            });
            run.trace = Some(Arc::new(RunTrace {
                sink: sink.clone(),
                session,
                rtp_packets: AtomicU64::new(0),
                rtp_bytes: AtomicU64::new(0),
                queue_overflows: AtomicU64::new(0),
            }));
        }
        let run = Arc::new(run);
        *self.current.lock().unwrap() = Some(run.clone());
        let session = match ClinkSession::new(
            self.link.clone(),
            self.pin.clone(),
            self.trust.clone(),
            Arc::new(SessionListener(self.listener.clone())),
            Arc::new(Callbacks {
                run: Arc::downgrade(&run),
            }),
            options,
        ) {
            Ok(s) => Arc::new(s),
            Err(e) => {
                self.opened.store(false, Ordering::Release);
                *self.current.lock().unwrap() = None;
                return Err(e);
            }
        };
        {
            let h = run.handoff.lock().unwrap();
            session.microphone(h.microphone.clone())?;
            *run.session.lock().unwrap() = Some(session.clone());
        }
        let voice_run = run.clone();
        let voice_listener = self.listener.clone();
        let voice = thread::Builder::new()
            .name("clink-voice".into())
            .spawn(move || {
                let result = voice_run.voice.run(
                    || voice_run.active.load(Ordering::Acquire),
                    || voice_run.session.lock().unwrap().clone(),
                    |p| voice_run.handoff.lock().unwrap().input_session == Some(p.session),
                );
                if let Err(e) = result {
                    if let Some(t) = &voice_run.trace {
                        t.error(e.as_ref());
                    }
                    if voice_run.active.load(Ordering::Acquire) {
                        voice_listener.error(&e.to_string());
                    }
                    if let Some(s) = voice_run.session.lock().unwrap().as_ref() {
                        s.stop();
                    }
                }
            });
        if let Err(e) = voice {
            run.active.store(false, Ordering::Release);
            session.stop();
            *self.current.lock().unwrap() = None;
            self.opened.store(false, Ordering::Release);
            return Err(e.into());
        }
        let rtp_run = run.clone();
        let rtp = thread::Builder::new()
            .name("clink-rtp".into())
            .spawn(move || {
                rtp_run
                    .sender
                    .run(|| rtp_run.active.load(Ordering::Acquire))
            });
        if let Err(e) = rtp {
            run.active.store(false, Ordering::Release);
            session.stop();
            *self.current.lock().unwrap() = None;
            self.opened.store(false, Ordering::Release);
            return Err(e.into());
        }
        let media_run = run.clone();
        let placeholder = self.placeholder.clone();
        let listener = self.listener.clone();
        let id = self.options.output_id.clone();
        let media = thread::Builder::new()
            .name("clink-media".into())
            .spawn(move || {
                if let Err(e) =
                    media_worker(&media_run, placeholder.as_ref(), listener.as_ref(), &id)
                {
                    if let Some(t) = &media_run.trace {
                        t.error(e.as_ref());
                    }
                    if media_run.active.load(Ordering::Acquire) {
                        listener.error(&e.to_string());
                    }
                    if let Some(s) = media_run.session.lock().unwrap().as_ref() {
                        s.stop();
                    }
                    media_run.active.store(false, Ordering::Release);
                }
            });
        if let Err(e) = media {
            run.active.store(false, Ordering::Release);
            session.stop();
            *self.current.lock().unwrap() = None;
            self.opened.store(false, Ordering::Release);
            return Err(e.into());
        }
        let lifecycle_run = run.clone();
        let current = self.current.clone();
        let opened = self.opened.clone();
        let listener = self.listener.clone();
        if let Err(e) = thread::Builder::new()
            .name("clink-lifecycle".into())
            .spawn(move || {
                if lifecycle_run.active.load(Ordering::Acquire) {
                    let _ = session.run();
                }
                lifecycle_run.active.store(false, Ordering::Release);
                let mut c = current.lock().unwrap();
                if c.as_ref().is_some_and(|r| Arc::ptr_eq(r, &lifecycle_run)) {
                    *c = None;
                }
                drop(c);
                opened.store(false, Ordering::Release);
                listener.state(ClinkState::Ended);
            })
        {
            run.active.store(false, Ordering::Release);
            if let Some(s) = run.session.lock().unwrap().as_ref() {
                s.stop();
            }
            *self.current.lock().unwrap() = None;
            self.opened.store(false, Ordering::Release);
            return Err(e.into());
        }
        Ok(())
    }
    pub fn on_session(&self, state: &SessionState) {
        let run = self.current.lock().unwrap().clone();
        if let Some(run) = run {
            let mut h = run.handoff.lock().unwrap();
            if h.input_session != Some(state.id) || state.phase != SessionPhase::Active {
                h.input_session = if state.phase == SessionPhase::Ended {
                    None
                } else {
                    Some(state.id)
                };
                if let Some(t) = &run.trace {
                    t.video("reset", "input_session");
                }
                h.queue.reset();
                h.audio.clear();
                h.audio_bytes = 0;
                h.media_formats.clear();
                h.media_stream = None;
                h.voice_streams.clear();
                h.clock_mix = ClockMix::default();
                run.voice.reset();
                h.microphone = None;
                if let Some(s) = run.session.lock().unwrap().as_ref() {
                    let _ = s.microphone(None);
                }
                h.revision = h.revision.wrapping_add(1);
                h.request_keyframe = true;
            }
        }
    }
    pub fn on_video_format(&self, screen: u32, format: Option<Arc<VideoFormat>>) {
        if screen != 0 {
            return;
        }
        let run = self.current.lock().unwrap().clone();
        if let Some(run) = run {
            let mut h = run.handoff.lock().unwrap();
            if !same_format(&h.format, &format) {
                if let Some(t) = &run.trace {
                    let config = format
                        .as_ref()
                        .filter(|f| f.codec == VideoCodec::H264)
                        .and_then(|f| AvcConfig::parse(&f.config).ok());
                    let hex = |bytes: &Vec<u8>| {
                        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
                    };
                    t.sink.record(ClinkTraceEvent::Video {
                        at_nanos: now_nanos(),
                        session: t.session,
                        event: "format".into(),
                        source: String::new(),
                        codec: format.as_ref().map(|f| f.codec_string.clone()),
                        width: format.as_ref().map(|f| f.width),
                        height: format.as_ref().map(|f| f.height),
                        sps: config
                            .as_ref()
                            .map(|c| c.sps.iter().map(hex).collect())
                            .unwrap_or_default(),
                        pps: config
                            .as_ref()
                            .map(|c| c.pps.iter().map(hex).collect())
                            .unwrap_or_default(),
                    });
                }
                h.format = format;
                h.queue.reset();
                h.revision = h.revision.wrapping_add(1);
                h.request_keyframe = true;
            }
        }
    }
    pub fn on_video_frame(&self, frame: VideoFrame) {
        if frame.screen != 0 {
            return;
        }
        let run = self.current.lock().unwrap().clone();
        if let Some(run) = run {
            let mut h = run.handoff.lock().unwrap();
            if !run.active.load(Ordering::Acquire)
                || !h.playing
                || Some(frame.session) != h.input_session
                || h.format.is_none()
            {
                return;
            }
            h.clock_mix.video(frame.pts_us, Instant::now());
            let request = if let Some(t) = &run.trace {
                h.queue.offer_observed(frame, || {
                    t.queue_overflows.fetch_add(1, Ordering::Relaxed);
                })
            } else {
                h.queue.offer(frame)
            };
            if request {
                h.request_keyframe = true;
            }
        }
    }
    pub fn on_audio_format(&self, stream: u32, format: Option<Arc<AudioFormat>>) {
        if let Some(run) = self.current.lock().unwrap().clone() {
            let mut h = run.handoff.lock().unwrap();
            if !run.active.load(Ordering::Acquire) {
                return;
            }
            let previous = h.media_stream;
            h.media_formats.retain(|(s, _)| *s != stream);
            if let Some(f) = format.as_ref().filter(|f| f.role == AudioRole::Media) {
                if f.codec == AudioCodec::AacLc
                    && f.sample_rate == 48000
                    && f.channels == 2
                    && f.config.as_deref() == Some(&[0x11, 0x90])
                {
                    h.media_formats.push((stream, f.clone()));
                } else {
                    h.diagnostic(format!("unsupported_media_audio:{stream}"));
                }
            }
            h.media_stream = h.media_formats.last().map(|(s, _)| *s);
            if previous != h.media_stream || previous == Some(stream) {
                h.audio.clear();
                h.audio_bytes = 0;
            }
            h.voice_streams.remove(&stream);
            if format.as_ref().is_some_and(|f| {
                voice_role(f.role)
                    && f.codec == AudioCodec::PcmS16le
                    && (8000..=48000).contains(&f.sample_rate)
                    && (1..=2).contains(&f.channels)
            }) {
                h.voice_streams.insert(stream);
            }
            run.voice.format(stream, format);
        }
    }
    pub fn on_audio_packet(&self, packet: AudioPacket) {
        if let Some(run) = self.current.lock().unwrap().clone() {
            let mut h = run.handoff.lock().unwrap();
            if !run.active.load(Ordering::Acquire) || h.input_session != Some(packet.session) {
                return;
            }
            if h.media_stream == Some(packet.stream) {
                if !(1..=8184).contains(&packet.data.len()) {
                    return;
                }
                if let Some(skew) = h.clock_mix.audio(packet.pts_us, Instant::now()) {
                    h.diagnostic(format!("audio clock mixed skew_ms={}", skew / 1000));
                }
                while !h.audio.is_empty()
                    && media_queue_full(
                        h.audio.len(),
                        h.audio_bytes + packet.data.len(),
                        packet
                            .pts_us
                            .saturating_sub(h.audio.front().unwrap().pts_us),
                    )
                {
                    h.audio_bytes -= h.audio.pop_front().unwrap().data.len();
                }
                h.audio_bytes += packet.data.len();
                h.audio.push_back(packet);
            } else if h.voice_streams.contains(&packet.stream) {
                run.voice.packet(packet);
            }
        }
    }
    pub fn on_microphone(&self, stream: u32, request: Option<MicRequest>) {
        if let Some(run) = self.current.lock().unwrap().clone() {
            let mut h = run.handoff.lock().unwrap();
            if !run.active.load(Ordering::Acquire) {
                return;
            }
            if request.as_ref().is_some_and(|r| {
                r.stream != stream
                    || r.codec != AudioCodec::PcmS16le
                    || !(8000..=48000).contains(&r.sample_rate)
                    || !(1..=2).contains(&r.channels)
                    || r.frame_ms != 20
                    || r.sample_rate % 50 != 0
            }) {
                return;
            }
            if let Some(s) = run.session.lock().unwrap().as_ref() {
                if s.microphone(request.clone()).is_err() {
                    return;
                }
            }
            h.microphone = request;
        }
    }
    /// Shells wait off the callback thread so microphone stop reaches the peer before process exit.
    pub fn await_closed(&self, timeout: Duration) -> bool {
        let start = Instant::now();
        while self.opened.load(Ordering::Acquire) {
            if start.elapsed() >= timeout {
                return false;
            }
            thread::sleep(Duration::from_millis(2));
        }
        true
    }
    pub fn close(&self) {
        let run = self.current.lock().unwrap().clone();
        if let Some(run) = run {
            run.active.store(false, Ordering::Release);
            if let Some(s) = run.session.lock().unwrap().as_ref() {
                s.stop();
            }
        }
    }
}
impl Drop for ClinkOutput {
    fn drop(&mut self) {
        self.close();
    }
}
struct SessionListener(Arc<dyn ClinkListener>);
impl ClinkListener for SessionListener {
    fn state(&self, s: ClinkState) {
        if s != ClinkState::Ended {
            self.0.state(s);
        }
    }
    fn error(&self, e: &str) {
        self.0.error(e);
    }
    fn diagnostic(&self, message: &str) {
        self.0.diagnostic(message);
    }
    fn view_size(&self, width: u32, height: u32) {
        self.0.view_size(width, height);
    }
}
fn same_format(a: &Option<Arc<VideoFormat>>, b: &Option<Arc<VideoFormat>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            Arc::ptr_eq(a, b)
                || (a.codec == b.codec
                    && a.codec_string == b.codec_string
                    && a.width == b.width
                    && a.height == b.height
                    && a.config == b.config)
        }
        _ => false,
    }
}
fn with_sink(run: &Run, f: impl FnOnce(&dyn OutputSink)) {
    // The hub fences removed hosts; callbacks must not serialize against input delivery or close.
    if run.active.load(Ordering::Acquire) {
        f(run.sink.as_ref());
    }
}
fn media_worker(
    run: &Run,
    placeholder: &dyn PlaceholderVideo,
    listener: &dyn ClinkListener,
    id: &str,
) -> Result<()> {
    let placeholder_config = AvcConfig::parse(&placeholder.format().config)?;
    require(
        !placeholder.frames().is_empty()
            && placeholder.fps() > 0
            && nal::is_key_frame(&placeholder.frames()[0], VideoCodec::H264, Format::Avcc)?,
        "invalid_placeholder",
    )?;
    let hevc_placeholder = BundledHevcPlaceholderVideo::new()?;
    let hevc_placeholder_config = HevcConfig::parse(&hevc_placeholder.format().config)?;
    let audio_config = oxp_media::AudioSpecificConfig::new(2, 48000, 2)?;
    let mut gate = VideoSwitch::default();
    let mut revision = None;
    let mut generation: Option<Arc<ClinkSession>> = None;
    let mut format = None;
    let mut avc = None;
    let mut hevc = None;
    let mut session_codec = None;
    let mut placeholder_index = 0;
    let mut next_placeholder = Instant::now();
    let mut output_pts = -1i64;
    let mut video_clock = VideoClock::default();
    let mut keyframes = KeyframeRequests::default();
    let mut transport_pts = -1i64;
    let mut live_offset = 0i64;
    let mut statistics = MediaStatistics::default();
    let mut reported_at = None;
    let mut mismatch_reported = None;
    let mut path = RtpPath::default();
    let mut write: Option<(Arc<ClinkSession>, RtpWrite)> = None;
    // What the write still in flight after the sender's wait carries; its clocks restart when it lands.
    let mut late_video = false;
    let mut late_live = false;
    let mut adts = Vec::new();
    let mut muxer = None;
    let mut trace_stats = run.trace.as_ref().map(|_| TraceMedia::default());
    while run.active.load(Ordering::Acquire) {
        let session = run.session.lock().unwrap().clone();
        // While a late write is in flight, media stays in the bounded queues (video drops to the next
        // keyframe, audio drops oldest) exactly as when this thread blocked in the socket write.
        let mut writable = path.writable(&run.sender)?;
        if let Some(elapsed) = run.sender.take_recovered() {
            listener.diagnostic(&format!(
                "rtp send recovered after ms={}",
                elapsed.as_millis()
            ));
            let now = Instant::now();
            if late_video {
                video_clock.written_at = Some(now);
            }
            if late_live && video_clock.live_frame_at.is_some() {
                video_clock.live_frame_at = Some(now);
            }
            late_video = false;
            late_live = false;
        }
        let (frame, request, diagnostics) = {
            let mut h = run.handoff.lock().unwrap();
            let changed = match (&generation, &session) {
                (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
                (None, None) => false,
                _ => true,
            };
            if revision != Some(h.revision) || changed {
                if let Some(t) = &run.trace {
                    t.video(
                        if video_clock.live_frame_at.is_some() {
                            "freeze"
                        } else {
                            "reset"
                        },
                        "revision",
                    );
                }
                gate.play(h.playing);
                video_clock.revision(changed);
                if changed {
                    statistics.new_session(h.queue.dropped, path.send_stalls);
                    reported_at = None;
                    muxer = None;
                    session_codec = None;
                    keyframes = KeyframeRequests::default();
                }
                placeholder_index = 0;
                next_placeholder = Instant::now();
                revision = Some(h.revision);
                generation = session.clone();
                if !same_format(&format, &h.format) {
                    format = h.format.clone();
                    avc = None;
                    hevc = None;
                    mismatch_reported = None;
                }
            }
            if h.queue.take_gap() {
                if let Some(t) = &run.trace {
                    t.video("reset", "gap");
                    if gate.live {
                        t.video("freeze", "gap");
                    }
                    trace_stats.as_mut().unwrap().keyframe_source = "gap";
                }
                gate.reset();
                placeholder_index = 0;
                next_placeholder = Instant::now();
            }
            let frame = if writable { h.queue.poll() } else { None };
            let request = h.request_keyframe;
            if session
                .as_ref()
                .is_some_and(|s| s.is_playing() && !s.is_stalled())
            {
                h.request_keyframe = false;
            }
            (frame, request, std::mem::take(&mut h.diagnostics))
        };
        for message in diagnostics {
            listener.diagnostic(&message);
        }
        let session = match session {
            Some(s) if s.is_playing() && !s.is_stalled() => s,
            _ => {
                thread::sleep(Duration::from_millis(5));
                continue;
            }
        };
        let snapshot = session.snapshot();
        // The car keeps the decoder selected by the first TS stream type for this entire session.
        let session_codec = *session_codec.get_or_insert(if snapshot.hevc_enabled {
            VideoCodec::H265
        } else {
            VideoCodec::H264
        });
        let clip: &dyn PlaceholderVideo = if session_codec == VideoCodec::H265 {
            &hevc_placeholder
        } else {
            placeholder
        };
        let clip_config = if session_codec == VideoCodec::H265 {
            VideoConfig::Hevc(&hevc_placeholder_config)
        } else {
            VideoConfig::Avc(&placeholder_config)
        };
        let muxer = muxer.get_or_insert_with(|| {
            if let Some(t) = &run.trace {
                let codec = codec_name(session_codec);
                t.sink.record(ClinkTraceEvent::Video {
                    at_nanos: now_nanos(),
                    session: t.session,
                    event: "muxer_codec".into(),
                    source: format!("initial->{codec} pmtVersion=0"),
                    codec: Some(codec.into()),
                    width: None,
                    height: None,
                    sps: Vec::new(),
                    pps: Vec::new(),
                });
            }
            TsMuxer::with_options(
                path.sink(),
                oxp_media::ts::TsOptions {
                    audio_enabled: snapshot.media_audio,
                    // The head unit identifies key frames from the first NAL.
                    insert_aud: false,
                    video_codec: session_codec,
                    ..Default::default()
                },
            )
            .unwrap()
        });
        if !write
            .as_ref()
            .is_some_and(|(s, _)| Arc::ptr_eq(s, &session))
        {
            let target = session.clone();
            let trace = run.trace.clone();
            let to_session: RtpWrite = Arc::new(move |p: &[u8]| {
                let sent = target.send_rtp(p)?;
                if sent {
                    if let Some(t) = &trace {
                        t.rtp_packets.fetch_add(1, Ordering::Relaxed);
                        t.rtp_bytes
                            .fetch_add((p.len() - 2) as u64, Ordering::Relaxed);
                    }
                }
                Ok(sent)
            });
            write = Some((session.clone(), to_session));
        }
        let write = &write.as_ref().unwrap().1;
        require(
            (snapshot.mode.width, snapshot.mode.height)
                == (clip.format().width, clip.format().height),
            "placeholder_timing_mismatch",
        )?;
        let origin = Origin {
            output_id: id.into(),
            client_id: snapshot.head_unit_id.clone(),
        };
        // A request during a stalled send stays pending instead of being lost.
        if !writable {
            keyframes.defer(request);
        } else if keyframes.poll(request, Instant::now()) {
            if let Some(t) = &run.trace {
                let stats = trace_stats.as_mut().unwrap();
                t.video("keyframe_request", stats.keyframe_source);
                stats.keyframe_source = "internal";
            }
            with_sink(run, |sink| {
                sink.control(ControlEvent::KeyFrameRequest {
                    screen: 0,
                    origin: origin.clone(),
                })
            });
        }
        let had_frame = frame.is_some();
        if let Some(frame) = frame {
            if let Some(f) = format.as_ref().filter(|f| f.codec == session_codec) {
                let config = match f.codec {
                    VideoCodec::H264 => {
                        if avc.is_none() {
                            avc = Some(AvcConfig::parse(&f.config)?);
                        }
                        VideoConfig::Avc(avc.as_ref().unwrap())
                    }
                    VideoCodec::H265 => {
                        if hevc.is_none() {
                            hevc = Some(HevcConfig::parse(&f.config)?);
                        }
                        VideoConfig::Hevc(hevc.as_ref().unwrap())
                    }
                };
                // The same acceptance rule the transcoding policy uses.
                let source = match config {
                    VideoConfig::Avc(c) => CodecProfile::avc(c.profile, c.compatibility, c.level),
                    VideoConfig::Hevc(c) => CodecProfile {
                        codec: VideoCodec::H265,
                        profile: c.profile,
                        constraints: 0,
                        level: c.level,
                        high_tier: c.high_tier,
                    },
                };
                let supported = snapshot.decoders.iter().any(|d| {
                    CodecProfile::parse(&d.codec_string).is_some_and(|d| d.accepts(&source))
                });
                if !supported && mismatch_reported.as_deref() != Some(f.codec_string.as_str()) {
                    mismatch_reported = Some(f.codec_string.clone());
                    listener.profile_mismatch(
                        &f.codec_string,
                        &snapshot
                            .decoders
                            .iter()
                            .map(|d| d.codec_string.clone())
                            .collect::<Vec<_>>(),
                    );
                }
                let was_live = gate.live;
                if gate.accept(frame.key_frame) {
                    if !was_live {
                        live_offset = (output_pts + 1_000_000 / clip.fps() as i64)
                            .max(0)
                            .max(frame.pts_us + video_clock.audio_offset.unwrap_or(-frame.pts_us))
                            - frame.pts_us;
                        let previous = live_offset;
                        live_offset =
                            reanchor_video(frame.pts_us, live_offset, output_pts, transport_pts);
                        if let Some(t) = &run.trace {
                            let lead = (output_pts + 1)
                                .max(transport_pts + 1)
                                .max(frame.pts_us + previous)
                                - transport_pts;
                            let input = frame.pts_us;
                            let audio_offset = video_clock.audio_offset.unwrap_or(-1);
                            let detail = format!(
                                concat!(
                                    "in={input} out={output_pts} tr={transport_pts} ",
                                    "off={live_offset} aoff={audio_offset} lead={lead}"
                                ),
                                input = input,
                                output_pts = output_pts,
                                transport_pts = transport_pts,
                                live_offset = live_offset,
                                audio_offset = audio_offset,
                                lead = lead,
                            );
                            t.video(if lead > 500_000 { "reanchor" } else { "live" }, &detail);
                        }
                    }
                    let pts = (output_pts + 1)
                        .max(transport_pts + 1)
                        .max(frame.pts_us + live_offset);
                    let result = path.emit(&run.sender, write, listener, pts, || {
                        muxer.write_video(&frame.data, config, pts, frame.key_frame)
                    })?;
                    output_pts = pts;
                    transport_pts = pts;
                    if let Some(stats) = &mut trace_stats {
                        if result == Emit::Dropped {
                            stats.failed_video += 1;
                        }
                        if result != Emit::Dropped {
                            stats.video_bytes += frame.data.len() as u64;
                            stats.keyframes += u64::from(frame.key_frame);
                        }
                    }
                    if !statistics.video(result, Some(f), frame.data.len()) {
                        if let Some(t) = &run.trace {
                            t.video("freeze", "unit_dropped");
                            trace_stats.as_mut().unwrap().keyframe_source = "gap";
                        }
                        gate.reset();
                        keyframes.defer(true);
                    } else {
                        session.set_stream_size((f.width, f.height));
                        let now = Instant::now();
                        video_clock.live_frame_at = Some(now);
                        video_clock.written_at = Some(now);
                    }
                    if result == Emit::Late {
                        writable = false;
                        late_video = true;
                        late_live = true;
                    }
                }
            } else {
                if let Some(t) = &run.trace {
                    t.sink.record(ClinkTraceEvent::Video {
                        at_nanos: now_nanos(),
                        session: t.session,
                        event: "codec_mismatch".into(),
                        source: format!(
                            "expected={} actual={}",
                            codec_name(session_codec),
                            format
                                .as_ref()
                                .map(|f| codec_name(f.codec))
                                .unwrap_or("null")
                        ),
                        codec: format.as_ref().map(|f| f.codec_string.clone()),
                        width: None,
                        height: None,
                        sps: Vec::new(),
                        pps: Vec::new(),
                    });
                    trace_stats.as_mut().unwrap().keyframe_source = "codec_mismatch";
                }
                statistics.total.dropped_frames += 1;
                gate.reset();
                keyframes.defer(true);
                if mismatch_reported.as_deref() != Some("codec_mismatch") {
                    mismatch_reported = Some("codec_mismatch".into());
                    listener.diagnostic(&format!(
                        "codec mismatch expected={session_codec:?} actual={:?}",
                        format.as_ref().map(|f| f.codec)
                    ));
                }
            }
        }
        let now = Instant::now();
        if writable
            && gate.placeholder()
            && now >= next_placeholder
            && video_clock.placeholder_due(now)
        {
            let data = &clip.frames()[placeholder_index];
            let pts = (output_pts + 1_000_000 / clip.fps() as i64)
                .max(0)
                .max(transport_pts + 1);
            let key = placeholder_index == 0;
            let result = path.emit(&run.sender, write, listener, pts, || {
                muxer.write_video(data, clip_config, pts, key)
            })?;
            output_pts = pts;
            transport_pts = pts;
            if let Some(stats) = &mut trace_stats {
                if result == Emit::Dropped {
                    stats.failed_video += 1;
                }
                if result != Emit::Dropped {
                    stats.video_bytes += data.len() as u64;
                    stats.keyframes += u64::from(key);
                }
            }
            if !statistics.video(result, None, data.len()) {
                placeholder_index = 0;
            } else {
                session.set_stream_size((clip.format().width, clip.format().height));
                video_clock.written_at = Some(Instant::now());
                placeholder_index = (placeholder_index + 1) % clip.frames().len();
            }
            if result == Emit::Late {
                writable = false;
                late_video = true;
            }
            next_placeholder = now + Duration::from_nanos(1_000_000_000 / clip.fps() as u64);
        }
        if snapshot.media_audio {
            for _ in 0..4 {
                if !writable {
                    break;
                }
                let audio = {
                    let mut h = run.handoff.lock().unwrap();
                    if !gate.live && video_clock.audio_offset.is_none() {
                        if let Some(p) = h.audio.front() {
                            video_clock.audio_offset = Some(output_pts.max(0) - p.pts_us);
                        }
                    }
                    let offset = if gate.live {
                        live_offset
                    } else {
                        video_clock.audio_offset.unwrap_or(0)
                    };
                    poll_audio(&mut h, offset, video_clock.pts(output_pts, Instant::now()))
                };
                if let Some(p) = audio {
                    let mapped = p.pts_us
                        + if gate.live {
                            live_offset
                        } else {
                            video_clock.audio_offset.unwrap_or(0)
                        };
                    if mapped >= 0 && mapped >= transport_pts - 50_000 {
                        let pts = mapped.max(transport_pts);
                        adts.clear();
                        adts.extend_from_slice(&oxp_media::Adts::build_header(
                            &audio_config,
                            p.data.len(),
                        )?);
                        adts.extend_from_slice(&p.data);
                        let result = path.emit(&run.sender, write, listener, pts, || {
                            muxer.write_audio(&adts, pts)
                        })?;
                        if let Some(stats) = &mut trace_stats {
                            stats.last_audio_pts = pts;
                            if result != Emit::Dropped {
                                stats.audio_frames += 1;
                            }
                        }
                        transport_pts = pts;
                        if result == Emit::Late {
                            writable = false;
                        }
                    }
                }
            }
        }
        if !run.active.load(Ordering::Acquire) {
            break;
        }
        if reported_at.is_none_or(|t: Instant| now.duration_since(t) >= Duration::from_secs(1)) {
            let dropped = run.handoff.lock().unwrap().queue.dropped;
            if let Some(t) = &run.trace {
                let stats = trace_stats.as_mut().unwrap();
                if stats
                    .clock_at
                    .is_none_or(|at| now.duration_since(at) >= Duration::from_secs(2))
                {
                    stats.clock_at = Some(now);
                    t.video(
                        "clock",
                        &format!(
                            "v={output_pts} a={} t={transport_pts} off={live_offset}",
                            stats.last_audio_pts
                        ),
                    );
                }
                t.sink.record(ClinkTraceEvent::Media {
                    at_nanos: now_nanos(),
                    session: t.session,
                    live_frames: statistics.total.live_frames,
                    placeholder_frames: statistics.total.placeholder_frames,
                    dropped_frames: dropped + stats.failed_video,
                    keyframes: stats.keyframes,
                    video_bytes: stats.video_bytes,
                    rtp_packets: t.rtp_packets.load(Ordering::Relaxed),
                    rtp_bytes: t.rtp_bytes.load(Ordering::Relaxed),
                    send_stalls: path.send_stalls,
                    audio_frames: stats.audio_frames,
                    queue_overflows: t.queue_overflows.load(Ordering::Relaxed),
                });
            }
            listener.statistics(statistics.snapshot(dropped, path.send_stalls));
            with_sink(run, |sink| {
                sink.stats(OutputStats {
                    origin,
                    screen: 0,
                    decode_time_us: None,
                    decode_queue: 0,
                    dropped,
                    rendered: 0,
                    end_to_end_latency_us: None,
                    audio_buffered_us: None,
                    audio_underruns: 0,
                })
            });
            reported_at = Some(now);
        }
        if !had_frame {
            thread::sleep(Duration::from_millis(2));
        }
    }
    Ok(())
}

fn codec_name(codec: VideoCodec) -> &'static str {
    match codec {
        VideoCodec::H264 => "H264",
        VideoCodec::H265 => "H265",
    }
}
struct TraceMedia {
    keyframes: u64,
    failed_video: u64,
    video_bytes: u64,
    audio_frames: u64,
    last_audio_pts: i64,
    clock_at: Option<Instant>,
    keyframe_source: &'static str,
}
impl Default for TraceMedia {
    fn default() -> Self {
        Self {
            failed_video: 0,
            keyframes: 0,
            video_bytes: 0,
            audio_frames: 0,
            last_audio_pts: -1,
            clock_at: None,
            keyframe_source: "internal",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Emit {
    Sent,
    /// Still in flight after the sender's wait.
    Late,
    /// Beyond [RtpUnit::MAX_BYTES]; nothing was sent.
    Dropped,
}
#[derive(Default)]
struct MediaStatistics {
    total: ClinkStatistics,
    base: ClinkStatistics,
    queue_drop_base: u64,
    stall_base: u64,
}
impl MediaStatistics {
    fn new_session(&mut self, queue_drops: u64, send_stalls: u64) {
        self.base = self.total;
        self.queue_drop_base = queue_drops;
        self.stall_base = send_stalls;
        self.total.codec = None;
        self.total.width = 0;
        self.total.height = 0;
    }
    fn video(&mut self, result: Emit, format: Option<&VideoFormat>, bytes: usize) -> bool {
        if result == Emit::Dropped {
            self.total.dropped_frames += 1;
            return false;
        }
        if let Some(format) = format {
            self.total.live_frames += 1;
            self.total.live_bytes += bytes as u64;
            self.total.codec = Some(format.codec);
            self.total.width = format.width;
            self.total.height = format.height;
        } else {
            self.total.placeholder_frames += 1;
        }
        true
    }
    fn snapshot(&self, queue_drops: u64, send_stalls: u64) -> ClinkStatistics {
        ClinkStatistics {
            live_frames: self.total.live_frames - self.base.live_frames,
            placeholder_frames: self.total.placeholder_frames - self.base.placeholder_frames,
            dropped_frames: queue_drops - self.queue_drop_base + self.total.dropped_frames
                - self.base.dropped_frames,
            live_bytes: self.total.live_bytes - self.base.live_bytes,
            send_stalls: send_stalls - self.stall_base,
            ..self.total
        }
    }
}
/// The media worker's side of the RTP path: the muxer streams TS packets straight into the one
/// reusable unit, which [RtpSender] writes on its own thread; nothing is retained per packet.
struct RtpPath {
    rtp: RtpPacketizer,
    /// None while the writer holds the unit.
    unit: Rc<RefCell<Option<RtpUnit>>>,
    send_stalls: u64,
}
impl Default for RtpPath {
    fn default() -> Self {
        Self {
            rtp: RtpPacketizer::default(),
            unit: Rc::new(RefCell::new(Some(RtpUnit::default()))),
            send_stalls: 0,
        }
    }
}
impl RtpPath {
    /// The muxer's packet sink; it borrows each TS packet into the unit.
    fn sink(&self) -> impl FnMut(&[u8; 188]) {
        let unit = self.unit.clone();
        move |p: &[u8; 188]| {
            if let Some(u) = unit.borrow_mut().as_mut() {
                u.append(p);
            }
        }
    }
    /// True when no write is in flight, so the unit may be refilled. Surfaces a writer failure.
    fn writable(&self, sender: &RtpSender) -> Result<bool> {
        if self.unit.borrow().is_none() {
            *self.unit.borrow_mut() = sender.reclaim()?;
        }
        Ok(self.unit.borrow().is_some())
    }
    /// Runs one muxer call into the unit and hands it to `sender`. Only while [Self::writable].
    fn emit(
        &mut self,
        sender: &RtpSender,
        write: &RtpWrite,
        listener: &dyn ClinkListener,
        pts: i64,
        mux: impl FnOnce() -> oxp_media::Result<()>,
    ) -> Result<Emit> {
        self.rtp.start(
            self.unit
                .borrow_mut()
                .as_mut()
                .ok_or("rtp_write_in_flight")?,
            pts,
        )?;
        mux()?;
        let unit = self.unit.borrow_mut().take().unwrap();
        if !self.rtp.finish(&unit) {
            listener.diagnostic(&format!("rtp unit_dropped limit={}", unit.max_bytes()));
            *self.unit.borrow_mut() = Some(unit);
            return Ok(Emit::Dropped);
        }
        match sender.submit(unit, write.clone())? {
            Some(unit) => {
                *self.unit.borrow_mut() = Some(unit);
                Ok(Emit::Sent)
            }
            None => {
                self.send_stalls += 1;
                listener.diagnostic(&format!(
                    "rtp send stalled ms={}",
                    sender.wait().as_millis()
                ));
                Ok(Emit::Late)
            }
        }
    }
}
/// Audio and live video share one mapping onto the transport clock, so their input timestamps must
/// come from one clock. Media audio far from the running video clock is held until the queue drops
/// it (ahead) or dropped as late (behind), and anchoring live video to it made the PTS jump by hours
/// on 2026-10-15. Reported once per input session.
#[derive(Default)]
struct ClockMix {
    video: Option<(i64, Instant)>,
    reported: bool,
}
impl ClockMix {
    /// Above the audio queue's 1.5 s span plus the phone's video batches of up to ~1 s; the video
    /// clock keeps running while the screen is static.
    const MAX_SKEW_US: u64 = 5_000_000;
    fn video(&mut self, pts_us: i64, now: Instant) {
        self.video = Some((pts_us, now));
    }
    /// The skew from the running video clock the first time media audio strays beyond the limit.
    fn audio(&mut self, pts_us: i64, now: Instant) -> Option<i64> {
        let (video_pts, at) = self.video?;
        let elapsed = now
            .saturating_duration_since(at)
            .as_micros()
            .min(i64::MAX as u128) as i64;
        let skew = pts_us.saturating_sub(video_pts.saturating_add(elapsed));
        if self.reported || skew.unsigned_abs() <= Self::MAX_SKEW_US {
            return None;
        }
        self.reported = true;
        Some(skew)
    }
}

#[derive(Default)]
struct KeyframeRequests {
    requested_at: Option<Instant>,
    pending: bool,
}
/// Keep the first live frame within 500 ms of the shared transport clock.
fn reanchor_video(input_pts: i64, offset: i64, output_pts: i64, transport_pts: i64) -> i64 {
    let lead = (output_pts + 1)
        .max(transport_pts + 1)
        .max(input_pts + offset)
        - transport_pts;
    if lead > 500_000 {
        offset - (lead - 1)
    } else {
        offset
    }
}
impl KeyframeRequests {
    fn defer(&mut self, request: bool) {
        self.pending |= request;
    }
    fn poll(&mut self, request: bool, now: Instant) -> bool {
        self.pending |= request;
        if self.pending
            && self
                .requested_at
                .is_none_or(|t| now.duration_since(t) >= Duration::from_secs(1))
        {
            self.pending = false;
            self.requested_at = Some(now);
            true
        } else {
            false
        }
    }
}
#[derive(Default)]
struct VideoClock {
    audio_offset: Option<i64>,
    written_at: Option<Instant>,
    live_frame_at: Option<Instant>,
}
impl VideoClock {
    fn revision(&mut self, session_changed: bool) {
        // Revisions keep the input clock; only a new socket session resets the anchor.
        if session_changed {
            self.audio_offset = None;
            self.live_frame_at = None;
        }
    }
    fn pts(&self, output_pts: i64, now: Instant) -> i64 {
        // Video arrives in batches, but audio must advance even while the screen is static.
        output_pts
            + self
                .written_at
                .map_or(0, |t| now.duration_since(t).as_micros() as i64)
    }
    fn placeholder_due(&self, now: Instant) -> bool {
        self.live_frame_at
            .is_none_or(|t| now.duration_since(t) > Duration::from_secs(3))
    }
}
fn poll_audio(h: &mut Handoff, offset: i64, clock_pts: i64) -> Option<AudioPacket> {
    if h.audio
        .front()
        .is_some_and(|p| p.pts_us + offset <= clock_pts + 50_000)
    {
        let p = h.audio.pop_front().unwrap();
        h.audio_bytes -= p.data.len();
        Some(p)
    } else {
        None
    }
}

impl oxp_core::ProjectionOutput for ClinkOutput {
    fn id(&self) -> &str {
        "clink"
    }
    fn open(&self, host: Arc<dyn oxp_core::OutputHost>) -> Result<()> {
        ClinkOutput::open(self, host)
    }
    fn on_session(&self, state: &SessionState, _: &oxp_core::model::SessionConfig) {
        ClinkOutput::on_session(self, state);
    }
    fn on_video_format(&self, screen: u32, format: Option<Arc<VideoFormat>>) {
        ClinkOutput::on_video_format(self, screen, format);
    }
    fn on_video_frame(&self, frame: VideoFrame) {
        ClinkOutput::on_video_frame(self, frame);
    }
    fn on_audio_format(&self, s: u32, f: Option<Arc<AudioFormat>>) {
        ClinkOutput::on_audio_format(self, s, f);
    }
    fn on_audio_packet(&self, p: AudioPacket) {
        ClinkOutput::on_audio_packet(self, p);
    }
    fn on_microphone(&self, s: u32, r: Option<MicRequest>) {
        ClinkOutput::on_microphone(self, s, r);
    }
    fn close(&self) {
        ClinkOutput::close(self);
    }
}

fn media_queue_full(packet_count: usize, total_bytes: usize, span_us: i64) -> bool {
    packet_count >= 96 || total_bytes > 524_288 || span_us > 1_500_000
}

#[cfg(test)]
mod audio_tests {
    use super::*;
    struct Host;
    impl OutputHost for Host {
        fn capabilities(&self, _: OutputCapabilities) {}
        fn control(&self, _: ControlEvent) {}
        fn stats(&self, _: OutputStats) {}
    }
    fn test_output() -> (ClinkOutput, Arc<Run>) {
        let output = ClinkOutput::new(
            Arc::new(StaticClinkLink(ClinkAddresses::new(
                std::net::Ipv4Addr::LOCALHOST,
                std::net::Ipv4Addr::LOCALHOST,
            ))),
            Arc::new(Pin),
            Arc::new(Trust),
        )
        .unwrap();
        let run = Arc::new(Run::new(Arc::new(Host)));
        *output.current.lock().unwrap() = Some(run.clone());
        (output, run)
    }
    #[derive(Default)]
    struct Trace(Mutex<Vec<ClinkTraceEvent>>);
    impl ClinkTrace for Trace {
        fn record(&self, event: ClinkTraceEvent) {
            self.0.lock().unwrap().push(event);
        }
    }
    #[test]
    fn traced_format_decodes_parameter_sets_once_and_reset_keeps_run_session() {
        let (output, old) = test_output();
        let trace = Arc::new(Trace::default());
        let mut run = Run::new(Arc::new(Host));
        run.trace = Some(Arc::new(RunTrace {
            sink: trace.clone(),
            session: 77,
            rtp_packets: AtomicU64::new(0),
            rtp_bytes: AtomicU64::new(0),
            queue_overflows: AtomicU64::new(0),
        }));
        *output.current.lock().unwrap() = Some(Arc::new(run));
        let format = Arc::new(output.placeholder.format().clone());
        output.on_video_format(0, Some(format.clone()));
        output.on_video_format(0, Some(Arc::new(format.as_ref().clone())));
        output.on_video_format(0, None);
        let events = trace.0.lock().unwrap();
        assert_eq!(events.len(), 2);
        let value = events[0].to_json();
        assert_eq!(value["session"], 77);
        assert_eq!(value["event"], "format");
        assert_eq!(value["codec"], format.codec_string);
        assert!(!value["sps"].as_array().unwrap().is_empty());
        assert!(!value["pps"].as_array().unwrap().is_empty());
        let reset = events[1].to_json();
        assert_eq!(reset["codec"], serde_json::Value::Null);
        assert_eq!(reset["sps"], serde_json::json!([]));
        assert!(old.trace.is_none());
    }
    #[test]
    fn traced_queue_distinguishes_waiting_drops_from_overflows() {
        let mut queue = VideoQueue::new(2, 600_000, 2).unwrap();
        let mut overflows = 0;
        for key_frame in [false, true, false] {
            queue.offer_observed(
                VideoFrame {
                    screen: 0,
                    session: 1,
                    data: Arc::from([0, 0]),
                    pts_us: 0,
                    key_frame,
                    received_at_nanos: 0,
                },
                || overflows += 1,
            );
        }
        assert_eq!(overflows, 1);
        assert_eq!(queue.dropped, 3);
    }
    #[test]
    fn only_voice_and_telephony_are_delivered_to_the_voice_channel() {
        let (output, run) = test_output();
        for (stream, role) in [
            AudioRole::Media,
            AudioRole::Alert,
            AudioRole::Voice,
            AudioRole::Telephony,
            AudioRole::Other,
        ]
        .into_iter()
        .enumerate()
        {
            let stream = stream as u32;
            output.on_audio_format(
                stream,
                Some(Arc::new(AudioFormat {
                    stream,
                    role,
                    codec: AudioCodec::PcmS16le,
                    sample_rate: 16000,
                    channels: 1,
                    config: None,
                })),
            );
        }
        assert_eq!(
            run.handoff.lock().unwrap().voice_streams,
            BTreeSet::from([2, 3])
        );
        output.on_audio_format(2, None);
        assert_eq!(
            run.handoff.lock().unwrap().voice_streams,
            BTreeSet::from([3])
        );
    }
    #[test]
    fn equal_video_formats_in_distinct_arcs_keep_the_queue_and_revision() {
        let (output, run) = test_output();
        let a = Arc::new(output.placeholder.format().clone());
        let mut equal = a.as_ref().clone();
        equal.config = a.config.to_vec().into();
        equal.fps_hint = Some(60.0);
        let b = Arc::new(equal);
        assert!(!Arc::ptr_eq(&a, &b));
        output.on_video_format(0, Some(a.clone()));
        let revision = {
            let mut h = run.handoff.lock().unwrap();
            h.request_keyframe = false;
            h.queue.offer(VideoFrame {
                screen: 0,
                session: 1,
                data: output.placeholder.frames()[0].clone(),
                pts_us: 0,
                key_frame: true,
                received_at_nanos: 0,
            });
            h.revision
        };
        output.on_video_format(0, Some(b));
        let mut h = run.handoff.lock().unwrap();
        assert_eq!(h.revision, revision);
        assert!(!h.request_keyframe);
        assert!(h.queue.poll().is_some());
        drop(h);
        for field in 0..5 {
            let mut different = a.as_ref().clone();
            match field {
                0 => different.codec = VideoCodec::H265,
                1 => different.codec_string.push('0'),
                2 => different.width += 1,
                3 => different.height += 1,
                _ => different.config = Arc::from([1u8]),
            }
            assert!(!same_format(&Some(a.clone()), &Some(Arc::new(different))));
        }
        assert!(same_format(&None, &None));
        assert!(!same_format(&Some(a), &None));
        output.on_video_format(0, None);
        assert_eq!(run.handoff.lock().unwrap().revision, revision + 1);
    }
    #[test]
    fn session_diagnostics_reach_the_output_listener() {
        #[derive(Default)]
        struct Listener(Mutex<Vec<String>>);
        impl ClinkListener for Listener {
            fn diagnostic(&self, message: &str) {
                self.0.lock().unwrap().push(message.into());
            }
        }
        let listener = Arc::new(Listener::default());
        let session_listener = SessionListener(listener.clone());
        session_listener.diagnostic("link stalled ms=5000");
        session_listener.diagnostic("link recovered after ms=10000");
        assert_eq!(
            *listener.0.lock().unwrap(),
            ["link stalled ms=5000", "link recovered after ms=10000"]
        );
    }
    #[test]
    fn session_view_sizes_reach_the_output_listener() {
        #[derive(Default)]
        struct Listener(Mutex<Vec<(u32, u32)>>);
        impl ClinkListener for Listener {
            fn view_size(&self, width: u32, height: u32) {
                self.0.lock().unwrap().push((width, height));
            }
        }
        let listener = Arc::new(Listener::default());
        let session_listener = SessionListener(listener.clone());
        session_listener.view_size(1532, 1013);
        session_listener.view_size(1814, 1200);
        assert_eq!(*listener.0.lock().unwrap(), [(1532, 1013), (1814, 1200)]);
    }
    #[test]
    fn media_statistics_count_emits_drops_and_stalls_with_per_session_bases() {
        let clip = BundledPlaceholderVideo::new().unwrap();
        let mut stats = MediaStatistics::default();
        stats.new_session(7, 2);
        assert_eq!(stats.snapshot(7, 2), ClinkStatistics::default());
        assert!(stats.video(Emit::Sent, None, 100));
        assert_eq!(
            stats.snapshot(7, 2),
            ClinkStatistics {
                placeholder_frames: 1,
                ..Default::default()
            }
        );
        assert!(stats.video(Emit::Sent, Some(clip.format()), 200));
        assert!(stats.video(Emit::Late, Some(clip.format()), 300));
        assert!(!stats.video(Emit::Dropped, Some(clip.format()), 400));
        assert!(!stats.video(Emit::Dropped, None, 500));
        // Codec mismatch skips join failed emits; queue drops and stalls have separate bases.
        stats.total.dropped_frames += 1;
        let expected = ClinkStatistics {
            live_frames: 2,
            placeholder_frames: 1,
            dropped_frames: 6,
            live_bytes: 500,
            send_stalls: 2,
            codec: Some(VideoCodec::H264),
            width: 1280,
            height: 720,
        };
        assert_eq!(stats.snapshot(10, 4), expected);
        assert_eq!(stats.snapshot(10, 4), expected, "reports are cumulative");
        stats.new_session(10, 4);
        assert_eq!(stats.snapshot(10, 4), ClinkStatistics::default());
        assert!(stats.video(Emit::Late, None, 1000));
        assert_eq!(
            stats.snapshot(11, 5),
            ClinkStatistics {
                placeholder_frames: 1,
                dropped_frames: 1,
                send_stalls: 1,
                ..Default::default()
            }
        );
    }
    #[test]
    fn audio_anchor_survives_revisions_and_resets_for_a_new_session() {
        let now = Instant::now();
        let mut clock = VideoClock {
            audio_offset: Some(-8_000_000_000),
            written_at: Some(now),
            live_frame_at: Some(now),
        };
        let mut gate = VideoSwitch::default();
        for _ in 0..3 {
            gate.play(true);
            clock.revision(false);
            assert_eq!(clock.audio_offset, Some(-8_000_000_000));
            assert_eq!(clock.live_frame_at, Some(now));
            assert_eq!(8_000_020_000 + clock.audio_offset.unwrap(), 20_000);
        }
        clock.revision(true);
        assert!(clock.audio_offset.is_none());
        assert!(clock.live_frame_at.is_none());
    }
    #[test]
    fn first_live_video_reanchors_only_beyond_500_ms_of_transport_pts() {
        let input = 8_000_000_000;
        let transport = 1_000_000;
        for (lead, expected) in [
            (499_999, 499_999),
            (500_000, 500_000),
            (500_001, 1),
            (9_000_000, 1),
        ] {
            let offset = reanchor_video(input, transport + lead - input, transport - 1, transport);
            assert_eq!(input + offset - transport, expected);
        }
        // An audio clock already ahead of video is also a floor for the first live frame.
        let offset = reanchor_video(input, -input, 900_000, transport);
        assert_eq!(
            (900_001i64).max(transport + 1).max(input + offset),
            transport + 1
        );
    }
    #[test]
    fn keyframe_requests_are_deferred_coalesced_and_reset_with_session() {
        let now = Instant::now();
        let mut requests = KeyframeRequests::default();
        assert!(requests.poll(true, now));
        assert!(!requests.poll(true, now + Duration::from_millis(100)));
        assert!(!requests.poll(true, now + Duration::from_millis(500)));
        assert!(!requests.poll(false, now + Duration::from_millis(999)));
        assert!(requests.poll(false, now + Duration::from_secs(1)));
        assert!(!requests.poll(false, now + Duration::from_secs(2)));
        requests = KeyframeRequests::default();
        assert!(requests.poll(true, now + Duration::from_millis(1100)));
        requests.defer(true);
        assert!(!requests.poll(false, now + Duration::from_millis(1200)));
        assert!(requests.poll(false, now + Duration::from_millis(2100)));
        assert!(!requests.poll(false, now + Duration::from_secs(4)));
    }
    #[test]
    fn static_video_releases_audio_against_running_clock() {
        let run = Run::new(Arc::new(Host));
        let now = Instant::now();
        let clock = VideoClock {
            written_at: Some(now),
            live_frame_at: Some(now),
            ..Default::default()
        };
        let mut h = run.handoff.lock().unwrap();
        h.audio.push_back(AudioPacket {
            stream: 0,
            session: 1,
            pts_us: 1_200_000,
            data: Arc::from([1; 4]),
        });
        h.audio_bytes = 4;
        assert!(poll_audio(&mut h, -1_000_000, clock.pts(0, now)).is_none());
        assert!(poll_audio(
            &mut h,
            -1_000_000,
            clock.pts(0, now + Duration::from_millis(150))
        )
        .is_some());
        assert!(h.audio.is_empty());
        assert_eq!(h.audio_bytes, 0);
        assert_eq!(VideoClock::default().pts(-1, now), -1);
    }
    #[test]
    fn reset_freezes_live_frame_until_long_gap() {
        let now = Instant::now();
        let mut clock = VideoClock::default();
        let mut gate = VideoSwitch::default();
        gate.play(true);
        assert!(gate.placeholder() && clock.placeholder_due(now));
        assert!(gate.accept(true));
        clock.live_frame_at = Some(now);
        gate.reset();
        assert!(gate.placeholder());
        assert!(!clock.placeholder_due(now + Duration::from_secs(3)));
        assert!(clock.placeholder_due(now + Duration::from_millis(3001)));
        clock.live_frame_at = None;
        assert!(clock.placeholder_due(now));
    }
    #[test]
    fn host_can_reenter_output_and_wait_for_close_on_another_thread() {
        struct ReentrantHost {
            output: Arc<ClinkOutput>,
            calls: std::sync::atomic::AtomicUsize,
        }
        impl OutputHost for ReentrantHost {
            fn capabilities(&self, _: OutputCapabilities) {}
            fn stats(&self, _: OutputStats) {}
            fn control(&self, _: ControlEvent) {
                self.calls.fetch_add(1, Ordering::Relaxed);
                self.output.on_video_format(0, None);
                let run = self.output.current.lock().unwrap().clone().unwrap();
                assert!(run.handoff.try_lock().is_ok());
                assert!(run.session.try_lock().is_ok());
                let output = self.output.clone();
                let (tx, rx) = std::sync::mpsc::channel();
                let worker = thread::spawn(move || {
                    output.close();
                    tx.send(()).unwrap();
                });
                let closed = rx.recv_timeout(Duration::from_secs(1));
                assert!(closed.is_ok(), "host callback blocked output close");
                worker.join().unwrap();
            }
        }
        let output = Arc::new(
            ClinkOutput::new(
                Arc::new(StaticClinkLink(ClinkAddresses::new(
                    std::net::Ipv4Addr::LOCALHOST,
                    std::net::Ipv4Addr::LOCALHOST,
                ))),
                Arc::new(Pin),
                Arc::new(Trust),
            )
            .unwrap(),
        );
        let host = Arc::new(ReentrantHost {
            output: output.clone(),
            calls: Default::default(),
        });
        let run = Arc::new(Run::new(host.clone()));
        *output.current.lock().unwrap() = Some(run.clone());
        let callbacks = Callbacks {
            run: Arc::downgrade(&run),
        };
        let event = || ControlEvent::KeyFrameRequest {
            screen: 0,
            origin: Origin {
                output_id: "clink".into(),
                client_id: "test".into(),
            },
        };
        callbacks.control(event());
        assert!(!run.active.load(Ordering::Acquire));
        callbacks.control(event());
        assert_eq!(host.calls.load(Ordering::Relaxed), 1);
        *output.current.lock().unwrap() = None;
    }
    struct Pin;
    impl PinProvider for Pin {
        fn request_pin(&self, _: &str) -> Option<String> {
            None
        }
    }
    struct Trust;
    impl TrustStore for Trust {
        fn load_identity(&self) -> Result<Option<crate::protocol::AuthIdentity>> {
            Ok(None)
        }
        fn save_identity(&self, _: &crate::protocol::AuthIdentity) -> Result<()> {
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
    #[test]
    fn media_queue_limits_retain_one_second_bursts_and_drop_oldest() {
        let mut q = std::collections::VecDeque::<(usize, i64)>::new();
        let mut bytes = 0;
        let offer = |q: &mut std::collections::VecDeque<(usize, i64)>,
                     bytes: &mut usize,
                     packet: (usize, i64)| {
            while !q.is_empty()
                && media_queue_full(
                    q.len(),
                    *bytes + packet.0,
                    packet.1.saturating_sub(q.front().unwrap().1),
                )
            {
                *bytes -= q.pop_front().unwrap().0;
            }
            *bytes += packet.0;
            q.push_back(packet);
        };
        for n in 0..50 {
            offer(&mut q, &mut bytes, (400, n * 20_000));
        }
        assert_eq!(q.len(), 50);
        for n in 50..150 {
            offer(&mut q, &mut bytes, (400, n * 20_000));
        }
        assert_eq!(q.len(), 76);
        assert_eq!(q.front().unwrap().1, 74 * 20_000);
        q.clear();
        bytes = 0;
        for _ in 0..97 {
            offer(&mut q, &mut bytes, (1, 0));
        }
        assert_eq!(q.len(), 96);
        q.clear();
        bytes = 0;
        for _ in 0..65 {
            offer(&mut q, &mut bytes, (8184, 0));
        }
        assert_eq!(q.len(), 64);
        assert!(media_queue_full(0, 0, i64::MAX.saturating_sub(i64::MIN)));
        assert!(!media_queue_full(0, 0, i64::MIN.saturating_sub(i64::MAX)));
        assert!(!media_queue_full(95, 524_288, 1_500_000));
        assert!(media_queue_full(96, 0, 0));
        assert!(media_queue_full(0, 524_289, 0));
        assert!(media_queue_full(0, 0, 1_500_001));
    }

    #[test]
    fn newest_media_bounded_queue_video_independence_and_session_reset() {
        let o = ClinkOutput::new(
            Arc::new(StaticClinkLink(ClinkAddresses::new(
                std::net::Ipv4Addr::LOCALHOST,
                std::net::Ipv4Addr::LOCALHOST,
            ))),
            Arc::new(Pin),
            Arc::new(Trust),
        )
        .unwrap();
        let run = Arc::new(Run::new(Arc::new(Host)));
        *o.current.lock().unwrap() = Some(run.clone());
        o.on_session(&SessionState {
            id: 1,
            phase: SessionPhase::Active,
            reason: None,
        });
        let media = |stream| {
            Some(Arc::new(AudioFormat {
                stream,
                role: AudioRole::Media,
                codec: AudioCodec::AacLc,
                sample_rate: 48000,
                channels: 2,
                config: Some(Arc::from([0x11, 0x90])),
            }))
        };
        let packet = |stream, session, pts| AudioPacket {
            stream,
            session,
            data: vec![1; 400].into(),
            pts_us: pts,
        };
        o.on_audio_format(0, media(0));
        for n in 0..1000 {
            o.on_audio_packet(packet(0, 1, 8_000_000_000 + n * 21333));
        }
        {
            let h = run.handoff.lock().unwrap();
            assert!(h.audio.len() <= 96 && h.audio_bytes <= 524_288);
            assert!(h.audio.back().unwrap().pts_us - h.audio.front().unwrap().pts_us <= 1_500_000);
        }
        o.on_video_format(0, Some(Arc::new(o.placeholder.format().clone())));
        run.handoff.lock().unwrap().playing = true;
        o.on_video_frame(VideoFrame {
            screen: 0,
            session: 1,
            data: o.placeholder.frames()[0].clone(),
            pts_us: 0,
            key_frame: true,
            received_at_nanos: 0,
        });
        assert!(run.handoff.lock().unwrap().queue.poll().is_some());
        o.on_audio_format(3, media(3));
        o.on_audio_packet(packet(0, 1, 0));
        assert!(run.handoff.lock().unwrap().audio.is_empty());
        o.on_audio_packet(packet(3, 0, 0));
        assert!(run.handoff.lock().unwrap().audio.is_empty());
        o.on_audio_packet(packet(3, 1, 0));
        assert_eq!(run.handoff.lock().unwrap().audio.len(), 1);
        o.on_audio_format(
            4,
            Some(Arc::new(AudioFormat {
                stream: 4,
                role: AudioRole::Media,
                codec: AudioCodec::PcmS16le,
                sample_rate: 16000,
                channels: 1,
                config: None,
            })),
        );
        assert_eq!(
            run.handoff.lock().unwrap().diagnostics.front().unwrap(),
            "unsupported_media_audio:4"
        );
        o.on_audio_format(3, None);
        assert_eq!(run.handoff.lock().unwrap().media_stream, Some(0));
        o.on_microphone(
            2,
            Some(MicRequest {
                stream: 2,
                codec: AudioCodec::PcmS16le,
                sample_rate: 16000,
                channels: 1,
                frame_ms: 20,
            }),
        );
        o.on_session(&SessionState {
            id: 1,
            phase: SessionPhase::Ended,
            reason: None,
        });
        let h = run.handoff.lock().unwrap();
        assert!(h.audio.is_empty() && h.media_stream.is_none() && h.microphone.is_none());
        drop(h);
        o.close();
    }
    #[test]
    fn audio_on_another_clock_is_reported_once_per_session() {
        let now = Instant::now();
        let mut mix = ClockMix::default();
        assert!(mix.audio(3_600_000_000, now).is_none(), "no video yet");
        mix.video(10_000_000, now);
        // Queued audio and batched video stay within the limit; the video clock runs while the
        // screen is static.
        assert!(mix.audio(8_500_000, now).is_none());
        assert!(mix.audio(14_000_000, now).is_none());
        assert!(mix
            .audio(70_000_000, now + Duration::from_secs(60))
            .is_none());
        assert_eq!(Some(-6_000_000), mix.audio(4_000_000, now));
        assert!(mix.audio(3_600_000_000, now).is_none(), "reported once");

        let o = ClinkOutput::new(
            Arc::new(StaticClinkLink(ClinkAddresses::new(
                std::net::Ipv4Addr::LOCALHOST,
                std::net::Ipv4Addr::LOCALHOST,
            ))),
            Arc::new(Pin),
            Arc::new(Trust),
        )
        .unwrap();
        let run = Arc::new(Run::new(Arc::new(Host)));
        *o.current.lock().unwrap() = Some(run.clone());
        let media = Some(Arc::new(AudioFormat {
            stream: 0,
            role: AudioRole::Media,
            codec: AudioCodec::AacLc,
            sample_rate: 48000,
            channels: 2,
            config: Some(Arc::from([0x11, 0x90])),
        }));
        let packet = |session, pts_us| AudioPacket {
            stream: 0,
            session,
            data: vec![1; 400].into(),
            pts_us,
        };
        let frame = |session, pts_us| VideoFrame {
            screen: 0,
            session,
            data: o.placeholder.frames()[0].clone(),
            pts_us,
            key_frame: true,
            received_at_nanos: 0,
        };
        let mixed = |run: &Run| {
            run.handoff
                .lock()
                .unwrap()
                .diagnostics
                .iter()
                .filter_map(|d| d.strip_prefix("audio clock mixed skew_ms="))
                .map(|ms| ms.parse::<i64>().unwrap())
                .collect::<Vec<_>>()
        };
        let session = |id| SessionState {
            id,
            phase: SessionPhase::Active,
            reason: None,
        };
        o.on_session(&session(1));
        o.on_audio_format(0, media.clone());
        o.on_video_format(0, Some(Arc::new(o.placeholder.format().clone())));
        run.handoff.lock().unwrap().playing = true;
        o.on_video_frame(frame(1, 5_000_000));
        o.on_audio_packet(packet(1, 5_020_000));
        assert!(mixed(&run).is_empty());
        // An hour ahead: such audio would never be released, and anchoring video to it jumps the PTS.
        o.on_audio_packet(packet(1, 3_605_000_000));
        o.on_audio_packet(packet(1, 3_605_020_000));
        let skews = mixed(&run);
        assert_eq!(1, skews.len());
        assert!((3_599_000..=3_600_000).contains(&skews[0]), "{skews:?}");
        // A new input session re-arms the report.
        o.on_session(&session(2));
        o.on_audio_format(0, media);
        o.on_video_frame(frame(2, 20_000_000));
        o.on_audio_packet(packet(2, 10_000_000));
        let skews = mixed(&run);
        assert_eq!(2, skews.len());
        assert!((-10_001..=-9_999).contains(&skews[1]), "{skews:?}");
        o.close();
    }

    #[test]
    fn stalled_socket_releases_the_media_worker_within_one_second() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let client =
            Arc::new(std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap());
        // The head unit stops reading.
        let (mut peer, _) = listener.accept().unwrap();
        let sender = Arc::new(RtpSender::default());
        assert_eq!(Duration::from_secs(1), sender.wait());
        let active = Arc::new(AtomicBool::new(true));
        let writer = {
            let (sender, active) = (sender.clone(), active.clone());
            thread::spawn(move || sender.run(|| active.load(Ordering::Acquire)))
        };
        let socket = client.clone();
        let write: RtpWrite = Arc::new(move |p: &[u8]| -> Result<bool> {
            (&*socket).write_all(p)?;
            Ok(true)
        });
        let config =
            AvcConfig::parse(&BundledPlaceholderVideo::new().unwrap().format().config).unwrap();
        // One 1 MiB IDR access unit in AVCC framing.
        let mut frame = vec![0u8; 1 << 20];
        frame[..4].copy_from_slice(&((1u32 << 20) - 4).to_be_bytes());
        frame[4] = 0x65;
        let mut path = RtpPath::default();
        let mut muxer = TsMuxer::new(path.sink());
        let mut late = false;
        for n in 0..256 {
            assert!(path.writable(&sender).unwrap());
            let pts = n * 33_333;
            let started = Instant::now();
            let result = path
                .emit(&sender, &write, &(), pts, || {
                    muxer.write_video(&frame, &config, pts, true)
                })
                .unwrap();
            let elapsed = started.elapsed();
            assert!(
                elapsed < Duration::from_millis(1500),
                "the media worker blocked in a socket write for {elapsed:?}"
            );
            if result == Emit::Late {
                assert!(elapsed >= sender.wait());
                late = true;
                break;
            }
            assert_eq!(Emit::Sent, result);
        }
        assert!(late, "the socket never stalled");
        assert_eq!(path.send_stalls, 1);
        // The media worker keeps running: polling never blocks, and video waiting meanwhile drops at
        // the queue's 600 ms bound.
        let mut queue = VideoQueue::default();
        let started = Instant::now();
        for n in 0..120 {
            assert!(!path.writable(&sender).unwrap());
            queue.offer(VideoFrame {
                screen: 0,
                session: 1,
                data: vec![0; 1024].into(),
                pts_us: n * 33_333,
                key_frame: n % 30 == 0,
                received_at_nanos: 0,
            });
        }
        assert!(started.elapsed() < Duration::from_millis(500));
        assert!(queue.dropped > 0);
        let (retained, in_flight) = sender.retained();
        assert!(in_flight && retained <= RtpUnit::MAX_BYTES);
        // Once the head unit reads again the late write lands and the unit returns for reuse.
        let drain = thread::spawn(move || {
            let mut buffer = vec![0; 1 << 16];
            while matches!(peer.read(&mut buffer), Ok(n) if n > 0) {}
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        while !path.writable(&sender).unwrap() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(2));
        }
        assert!(sender.take_recovered().unwrap() >= sender.wait());
        assert_eq!(path.send_stalls, 1, "recovery does not count another stall");
        active.store(false, Ordering::Release);
        writer.join().unwrap();
        drop(write);
        client.shutdown(std::net::Shutdown::Both).unwrap();
        drain.join().unwrap();
    }
}
