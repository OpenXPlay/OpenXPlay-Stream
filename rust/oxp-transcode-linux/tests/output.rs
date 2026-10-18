// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

use oxp_core::{model::*, OutputHost, ProjectionOutput};
use oxp_media::AvcConfig;
use oxp_transcode_linux::{
    fake::{avc_parameter_sets, FakeFactory, FakeStats, FakeTranscoder},
    TranscodeSupport, TranscodingOutput,
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

#[derive(Default)]
struct Upstream {
    controls: Mutex<Vec<ControlEvent>>,
    capabilities: Mutex<usize>,
}
impl Upstream {
    fn keyframes(&self) -> usize {
        self.controls
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, ControlEvent::KeyFrameRequest { .. }))
            .count()
    }
}
impl OutputHost for Upstream {
    fn capabilities(&self, _: OutputCapabilities) {
        *self.capabilities.lock().unwrap() += 1;
    }
    fn control(&self, e: ControlEvent) {
        self.controls.lock().unwrap().push(e);
    }
    fn stats(&self, _: OutputStats) {}
}

/// Stands in for the Carlink output: reports decoders and asks for keyframes like it does.
#[derive(Default)]
struct Inner {
    host: Mutex<Option<Arc<dyn OutputHost>>>,
    formats: Mutex<Vec<Option<Arc<VideoFormat>>>>,
    frames: Mutex<Vec<VideoFrame>>,
    sessions: Mutex<Vec<SessionPhase>>,
    closed: AtomicBool,
}
impl Inner {
    fn host(&self) -> Arc<dyn OutputHost> {
        self.host.lock().unwrap().clone().unwrap()
    }
    fn report(&self, decoders: &[&str]) {
        self.host().capabilities(OutputCapabilities {
            prefers_h264: false,
            latency: LatencyClass::Interactive,
            decoders: decoders
                .iter()
                .map(|d| DecoderSupport {
                    codec_string: (*d).into(),
                    hardware: true,
                })
                .collect(),
            displays: [(0, DisplayInfo::default())].into(),
            touch: 2,
            keys: Default::default(),
            audio: Default::default(),
            microphone: false,
        });
    }
    fn request_keyframe(&self) {
        self.host().control(ControlEvent::KeyFrameRequest {
            screen: 0,
            origin: Origin {
                output_id: "clink".into(),
                client_id: "head-unit".into(),
            },
        });
    }
    fn last_format(&self) -> Option<Arc<VideoFormat>> {
        self.formats.lock().unwrap().last().cloned().flatten()
    }
    fn frame_count(&self) -> usize {
        self.frames.lock().unwrap().len()
    }
}
impl ProjectionOutput for Inner {
    fn id(&self) -> &str {
        "clink"
    }
    fn open(&self, host: Arc<dyn OutputHost>) -> oxp_core::Result<()> {
        *self.host.lock().unwrap() = Some(host);
        Ok(())
    }
    fn on_session(&self, state: &SessionState, _: &SessionConfig) {
        self.sessions.lock().unwrap().push(state.phase);
    }
    fn on_video_format(&self, _: u32, format: Option<Arc<VideoFormat>>) {
        self.formats.lock().unwrap().push(format);
    }
    fn on_video_frame(&self, frame: VideoFrame) {
        self.frames.lock().unwrap().push(frame);
    }
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }
}

struct Rig {
    output: TranscodingOutput,
    inner: Arc<Inner>,
    upstream: Arc<Upstream>,
    stats: Arc<FakeStats>,
    diagnostics: Arc<Mutex<Vec<String>>>,
}
impl Rig {
    fn new(factory: FakeFactory) -> Self {
        let inner = Arc::new(Inner::default());
        let upstream = Arc::new(Upstream::default());
        let stats = factory.stats.clone();
        let diagnostics = Arc::new(Mutex::new(Vec::new()));
        let log = diagnostics.clone();
        let output = TranscodingOutput::new(
            inner.clone(),
            Arc::new(factory),
            Arc::new(move |line: &str| log.lock().unwrap().push(line.to_owned())),
        );
        output.open(upstream.clone()).unwrap();
        output.on_session(
            &SessionState {
                id: 1,
                phase: SessionPhase::Active,
                reason: None,
            },
            &SessionConfig::default(),
        );
        Self {
            output,
            inner,
            upstream,
            stats,
            diagnostics,
        }
    }
    fn lines(&self) -> Vec<String> {
        self.diagnostics.lock().unwrap().clone()
    }
    fn frame(&self, pts_us: i64, key_frame: bool) -> VideoFrame {
        let frame = VideoFrame {
            screen: 0,
            session: 1,
            data: vec![0, 0, 0, 3, if key_frame { 0x65 } else { 0x41 }, 1, 2].into(),
            pts_us,
            key_frame,
            received_at_nanos: pts_us * 1000,
        };
        self.output.on_video_frame(frame.clone());
        frame
    }
}
fn support() -> TranscodeSupport {
    TranscodeSupport {
        decode: vec![VideoCodec::H264, VideoCodec::H265],
        encode: vec![VideoCodec::H264],
        scaling: true,
    }
}
/// The iPhone's stream for car A: H.264 High 4.0, 1280x720.
fn high() -> Arc<VideoFormat> {
    let (sps, pps) = avc_parameter_sets(100, 0, 40, 1280, 720);
    Arc::new(
        VideoFormat::create(
            0,
            VideoCodec::H264,
            AvcConfig::from_parameter_sets(&sps, &pps).unwrap().into(),
            Some(60.0),
        )
        .unwrap(),
    )
}
fn wait(what: &str, condition: impl Fn() -> bool) {
    let start = Instant::now();
    while !condition() {
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "timed out: {what}"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
const CAR_A: &str =
    "video transcode H264 High 4.0 1280x720 -> H264 CB 3.1 1280x720@30 reason=sink-profile";

#[test]
fn constrained_baseline_receiver_gets_transcoded_frames() {
    let rig = Rig::new(FakeFactory::new(Some(support())));
    rig.inner.report(&["avc1.42E01F"]);
    assert_eq!(*rig.upstream.capabilities.lock().unwrap(), 1);
    rig.output.on_video_format(0, Some(high()));
    assert_eq!(rig.lines(), vec![CAR_A]);
    // The receiver keeps its placeholder until the encoder describes its own stream.
    assert_eq!(rig.inner.formats.lock().unwrap().clone().len(), 1);
    assert!(rig.inner.last_format().is_none());
    // The new decoder needs an IDR from the iPhone.
    wait("input keyframe request", || rig.upstream.keyframes() == 1);
    rig.frame(10_000, false);
    let sent: Vec<_> = [
        (20_000, true),
        (36_667, false),
        (53_334, false),
        (70_001, false),
    ]
    .into_iter()
    .map(|(pts, key)| rig.frame(pts, key))
    .collect();
    wait("transcoded frames", || rig.inner.frame_count() == 4);
    let format = rig.inner.last_format().unwrap();
    assert_eq!(format.codec_string, "avc1.42E01F");
    assert_eq!(
        (format.width, format.height, format.fps_hint),
        (1280, 720, Some(30.0))
    );
    let frames = rig.inner.frames.lock().unwrap().clone();
    for (out, input) in frames.iter().zip(&sent) {
        assert_eq!(out.pts_us, input.pts_us);
        assert_eq!(FakeTranscoder::decode_pts(&out.data), Some(input.pts_us));
        assert_eq!(
            (out.session, out.received_at_nanos),
            (1, input.received_at_nanos)
        );
        assert!(!Arc::ptr_eq(&out.data, &input.data));
    }
    assert!(frames[0].key_frame && !frames[1].key_frame);
    assert_eq!(FakeStats::get(&rig.stats.opened), 1);
    assert_eq!(FakeStats::get(&rig.stats.submitted), 4);
    assert_eq!(rig.inner.formats.lock().unwrap().len(), 2);
    rig.output.close();
    assert!(rig.inner.closed.load(Ordering::Acquire));
    wait("transcoder released", || {
        FakeStats::get(&rig.stats.closed) == 1
    });
}

#[test]
fn receiver_keyframe_requests_reach_the_encoder() {
    let rig = Rig::new(FakeFactory::new(Some(support())));
    rig.inner.report(&["avc1.42E01F"]);
    rig.output.on_video_format(0, Some(high()));
    wait("pipeline start", || rig.upstream.keyframes() == 1);
    rig.inner.request_keyframe();
    assert_eq!(rig.upstream.keyframes(), 2);
    rig.frame(0, true);
    wait("first frame", || rig.inner.frame_count() == 1);
    wait("early request applied", || {
        FakeStats::get(&rig.stats.forced) == 1
    });
    // The encoder makes the IDR, and the iPhone still hears the request so a static screen
    // yields a frame to encode.
    rig.inner.request_keyframe();
    wait("encoder keyframe", || {
        FakeStats::get(&rig.stats.forced) == 2
    });
    assert_eq!(rig.upstream.keyframes(), 3);
    rig.frame(16_667, false);
    wait("forced IDR", || rig.inner.frame_count() == 2);
    assert!(rig.inner.frames.lock().unwrap()[1].key_frame);
    assert_eq!(FakeStats::get(&rig.stats.opened), 1);
    // Other control events pass through untouched.
    rig.inner.host().control(ControlEvent::Key {
        code: KeyCode::Home,
        down: true,
        origin: Origin {
            output_id: "clink".into(),
            client_id: "head-unit".into(),
        },
    });
    assert_eq!(rig.upstream.controls.lock().unwrap().len(), 4);
}

#[test]
fn open_failure_falls_back_to_the_original_stream() {
    let mut factory = FakeFactory::new(Some(support()));
    factory.open_error = Some("no V4L2 encoder".into());
    let rig = Rig::new(factory);
    rig.inner.report(&["avc1.42E01F"]);
    let source = high();
    rig.output.on_video_format(0, Some(source.clone()));
    wait("fallback", || rig.inner.last_format().is_some());
    assert!(Arc::ptr_eq(&rig.inner.last_format().unwrap(), &source));
    assert_eq!(
        rig.lines(),
        vec![
            CAR_A.to_owned(),
            "transcoding unavailable: no V4L2 encoder".into()
        ]
    );
    // The receiver needs an IDR to start on the original stream.
    assert_eq!(rig.upstream.keyframes(), 1);
    let frame = rig.frame(0, true);
    assert!(Arc::ptr_eq(
        &rig.inner.frames.lock().unwrap()[0].data,
        &frame.data
    ));
    // An identical configuration does not retry or repeat the diagnostic.
    rig.output.on_video_format(0, Some(high()));
    assert_eq!(rig.lines().len(), 2);
    assert_eq!(rig.inner.formats.lock().unwrap().len(), 3);
    rig.inner.request_keyframe();
    assert_eq!(rig.upstream.keyframes(), 2);
}

#[test]
fn runtime_failure_falls_back_to_the_original_stream() {
    let mut factory = FakeFactory::new(Some(support()));
    factory.fail_after = Some(2);
    let rig = Rig::new(factory);
    rig.inner.report(&["avc1.42E01F"]);
    let source = high();
    rig.output.on_video_format(0, Some(source.clone()));
    wait("pipeline start", || rig.upstream.keyframes() == 1);
    for (i, key) in [true, false, false].into_iter().enumerate() {
        rig.frame(i as i64 * 33_333, key);
    }
    wait("fallback", || {
        rig.inner
            .last_format()
            .is_some_and(|f| Arc::ptr_eq(&f, &source))
    });
    assert_eq!(rig.inner.frame_count(), 2);
    assert_eq!(
        rig.lines(),
        vec![
            CAR_A.to_owned(),
            "transcoding unavailable: fake encoder failure".into()
        ]
    );
    assert_eq!(rig.upstream.keyframes(), 2);
    let frame = rig.frame(100_000, true);
    assert!(Arc::ptr_eq(
        &rig.inner.frames.lock().unwrap()[2].data,
        &frame.data
    ));
    wait("transcoder released", || {
        FakeStats::get(&rig.stats.closed) == 1
    });
}

#[test]
fn identical_formats_and_capabilities_do_not_rebuild() {
    let rig = Rig::new(FakeFactory::new(Some(support())));
    rig.inner.report(&["avc1.42E01F"]);
    rig.output.on_video_format(0, Some(high()));
    wait("pipeline start", || rig.upstream.keyframes() == 1);
    for i in 0..6 {
        // The input repeats its configuration as a new object with every keyframe.
        if i % 2 == 0 {
            rig.output.on_video_format(0, Some(high()));
            rig.inner.report(&["avc1.42E01F"]);
        }
        rig.frame(i * 33_333, i % 2 == 0);
    }
    wait("frames", || rig.inner.frame_count() == 6);
    assert_eq!(FakeStats::get(&rig.stats.opened), 1);
    assert_eq!(FakeStats::get(&rig.stats.closed), 0);
    assert_eq!(rig.lines(), vec![CAR_A]);
    let formats = rig.inner.formats.lock().unwrap().clone();
    assert_eq!(formats.len(), 2);
    assert!(formats[0].is_none() && formats[1].is_some());
    assert_eq!(rig.upstream.keyframes(), 1);
}

#[test]
fn accepted_streams_pass_through_untouched() {
    let rig = Rig::new(FakeFactory::new(Some(support())));
    rig.inner.report(&["avc1.64002A"]);
    let source = high();
    rig.output.on_video_format(0, Some(source.clone()));
    rig.output.on_video_format(1, Some(source.clone()));
    assert!(Arc::ptr_eq(&rig.inner.last_format().unwrap(), &source));
    let frame = rig.frame(0, true);
    assert!(Arc::ptr_eq(
        &rig.inner.frames.lock().unwrap()[0].data,
        &frame.data
    ));
    assert_eq!(
        rig.lines(),
        vec!["video passthrough H264 High 4.0 1280x720 reason=sink-accepts"]
    );
    assert_eq!(FakeStats::get(&rig.stats.opened), 0);
    rig.inner.request_keyframe();
    assert_eq!(rig.upstream.keyframes(), 1);
}

#[test]
fn missing_transcoder_is_reported_once_and_passes_through() {
    let rig = Rig::new(FakeFactory::new(None));
    rig.inner.report(&["avc1.42E01F"]);
    let source = high();
    rig.output.on_video_format(0, Some(source.clone()));
    rig.output.on_video_format(0, Some(high()));
    assert!(rig.inner.last_format().is_some());
    rig.frame(0, true);
    assert_eq!(rig.inner.frame_count(), 1);
    assert_eq!(
        rig.lines(),
        vec!["video unsupported H264 High 4.0 1280x720 reason=no-transcoder"]
    );
}

#[test]
fn capability_changes_switch_between_passthrough_and_transcoding() {
    let rig = Rig::new(FakeFactory::new(Some(support())));
    let source = high();
    // Before the receiver negotiates, the stream passes through silently.
    rig.output.on_video_format(0, Some(source.clone()));
    assert!(Arc::ptr_eq(&rig.inner.last_format().unwrap(), &source));
    assert!(rig.lines().is_empty());
    rig.inner.report(&["avc1.42E01F"]);
    assert_eq!(rig.lines(), vec![CAR_A]);
    assert!(rig.inner.last_format().is_none());
    wait("pipeline start", || FakeStats::get(&rig.stats.opened) == 1);
    rig.inner.report(&["avc1.42E01F", "avc1.64002A"]);
    assert!(Arc::ptr_eq(&rig.inner.last_format().unwrap(), &source));
    assert_eq!(
        rig.lines().last().unwrap(),
        "video passthrough H264 High 4.0 1280x720 reason=sink-accepts"
    );
    wait("transcoder released", || {
        FakeStats::get(&rig.stats.closed) == 1
    });
    rig.inner.report(&["avc1.42E01F"]);
    wait("second pipeline", || FakeStats::get(&rig.stats.opened) == 2);
    assert_eq!(rig.lines().len(), 3);
}

#[test]
fn session_end_stops_the_pipeline() {
    let rig = Rig::new(FakeFactory::new(Some(support())));
    rig.inner.report(&["avc1.42E01F"]);
    rig.output.on_video_format(0, Some(high()));
    wait("pipeline start", || FakeStats::get(&rig.stats.opened) == 1);
    rig.output.on_session(
        &SessionState {
            id: 1,
            phase: SessionPhase::Ended,
            reason: None,
        },
        &SessionConfig::default(),
    );
    wait("transcoder released", || {
        FakeStats::get(&rig.stats.closed) == 1
    });
    assert_eq!(
        *rig.inner.sessions.lock().unwrap(),
        vec![SessionPhase::Active, SessionPhase::Ended]
    );
    // The next session reports its decision again.
    rig.output.on_video_format(0, Some(high()));
    assert_eq!(rig.lines(), vec![CAR_A, CAR_A]);
}

#[test]
fn encoder_ignoring_the_target_profile_falls_back() {
    let mut factory = FakeFactory::new(Some(support()));
    factory.encoder_profile = Some((100, 0));
    let rig = Rig::new(factory);
    rig.inner.report(&["avc1.42E01F"]);
    let source = high();
    rig.output.on_video_format(0, Some(source.clone()));
    wait("pipeline start", || rig.upstream.keyframes() == 1);
    rig.frame(0, true);
    wait("fallback", || rig.lines().len() == 2);
    assert_eq!(
        rig.lines()[1],
        "transcoding unavailable: encoder produced H264 High 3.1, which the receiver rejects"
    );
    assert!(Arc::ptr_eq(&rig.inner.last_format().unwrap(), &source));
    assert_eq!(rig.inner.frame_count(), 0);
}
