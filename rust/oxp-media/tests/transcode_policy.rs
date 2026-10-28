// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

use oxp_media::{transcode_policy::*, VideoCodec};

fn source(codec: &str, width: u32, height: u32, fps: Option<f64>) -> SourceVideo {
    SourceVideo {
        profile: CodecProfile::parse(codec).unwrap(),
        width,
        height,
        fps,
    }
}
fn sink(decoders: &[&str]) -> SinkVideo {
    SinkVideo::from_codec_strings(decoders.iter().copied())
}
fn support(decode: &[VideoCodec], scaling: bool) -> TranscodeSupport {
    TranscodeSupport {
        decode: decode.to_vec(),
        encode: vec![VideoCodec::H264],
        scaling,
    }
}
const BOTH: &[VideoCodec] = &[VideoCodec::H264, VideoCodec::H265];
/// Wi-Fi Display bitmaps "02 1F": Constrained Baseline and High at levels 3.1 to 4.2.
const WFD_ALL: &[&str] = &[
    "avc1.42E01F",
    "avc1.42E020",
    "avc1.42E028",
    "avc1.42E029",
    "avc1.42E02A",
    "avc1.64001F",
    "avc1.640020",
    "avc1.640028",
    "avc1.640029",
    "avc1.64002A",
];

#[test]
fn codec_strings_parse_and_print() {
    let high = CodecProfile::parse("avc1.640028").unwrap();
    assert_eq!(
        (high.codec, high.profile, high.level),
        (VideoCodec::H264, 100, 40)
    );
    assert_eq!(high.to_string(), "H264 High 4.0");
    assert_eq!(
        CodecProfile::parse("avc1.42E01F").unwrap().to_string(),
        "H264 CB 3.1"
    );
    assert_eq!(
        CodecProfile::parse("avc3.42001E").unwrap().to_string(),
        "H264 Baseline 3.0"
    );
    assert_eq!(
        CodecProfile::parse("avc1.4D4029").unwrap().to_string(),
        "H264 Main 4.1"
    );
    let hevc = CodecProfile::parse("hvc1.1.6.L120.90").unwrap();
    assert_eq!(
        (hevc.codec, hevc.profile, hevc.level),
        (VideoCodec::H265, 1, 120)
    );
    assert!(!hevc.high_tier);
    assert_eq!(hevc.to_string(), "H265 Main 4.0");
    assert_eq!(
        CodecProfile::parse("hev1.A2.4.H153").unwrap().to_string(),
        "H265 Main10 5.1"
    );
    assert_eq!(
        CodecProfile::avc(66, 0xE0, 31).codec_string(),
        "avc1.42E01F"
    );
    for bad in [
        "",
        "avc1",
        "avc1.64001",
        "avc1.64001G",
        "vp09.00.10.08",
        "hvc1.1.6",
        "hvc1.x.6.L93",
        "hvc1.1.6.X93",
        "hvc1.1.Z.L93",
    ] {
        assert!(CodecProfile::parse(bad).is_none(), "{bad}");
    }
}

#[test]
fn acceptance_follows_profile_families_levels_and_tiers() {
    let p = |s| CodecProfile::parse(s).unwrap();
    for (decoder, stream, expected) in [
        ("avc1.42E01F", "avc1.42E01F", true),
        ("avc1.42E01F", "avc1.42E01E", true),
        ("avc1.42E01F", "avc1.42E028", false),
        ("avc1.42E01F", "avc1.64001F", false),
        ("avc1.64002A", "avc1.42E01F", true),
        ("avc1.64002A", "avc1.4D4028", true),
        ("avc1.64001F", "avc1.640028", false),
        ("avc1.4D4028", "avc1.42E01F", true),
        ("avc1.4D4028", "avc1.42001F", false),
        ("hvc1.2.4.L153", "hvc1.1.6.L120.90", true),
        ("hvc1.1.6.L120", "hvc1.1.6.H120", false),
        ("hvc1.1.6.H153", "hvc1.1.6.H120", true),
        ("hvc1.1.6.L153", "avc1.42E01F", false),
    ] {
        assert_eq!(
            p(decoder).accepts(&p(stream)),
            expected,
            "{decoder} {stream}"
        );
    }
}

#[test]
fn decision_table() {
    use TranscodeDecision::*;
    let cb31 = sink(&["avc1.42E01F"]);
    let car_a = source("avc1.640028", 1280, 720, Some(60.0));
    let transcoder = support(BOTH, true);
    let cases: Vec<(
        &str,
        SourceVideo,
        SinkVideo,
        Option<TranscodeSupport>,
        TranscodeDecision,
    )> = vec![
        (
            "unknown sink keeps the stream",
            car_a,
            sink(&[]),
            Some(transcoder.clone()),
            Passthrough {
                reason: "sink-unknown",
            },
        ),
        (
            "matching Constrained Baseline",
            source("avc1.42E01F", 1280, 720, Some(30.0)),
            cb31.clone(),
            None,
            Passthrough {
                reason: "sink-accepts",
            },
        ),
        (
            "High receivers accept High at or below their level",
            source("avc1.64001F", 1280, 720, None),
            sink(&["avc1.64002A"]),
            None,
            Passthrough {
                reason: "sink-accepts",
            },
        ),
        (
            "car A: High 4.0 to a Constrained Baseline 3.1 receiver",
            car_a,
            cb31.clone(),
            Some(transcoder.clone()),
            Transcode {
                target: TranscodeTarget {
                    profile: CodecProfile::avc(66, 0xE0, 31),
                    width: 1280,
                    height: 720,
                    fps: 30,
                    bitrate: 4_000_000,
                },
                reason: "sink-profile",
            },
        ),
        (
            "Constrained Baseline above the receiver level",
            source("avc1.42E028", 1280, 720, Some(30.0)),
            cb31.clone(),
            Some(transcoder.clone()),
            Transcode {
                target: TranscodeTarget {
                    profile: CodecProfile::avc(66, 0xE0, 31),
                    width: 1280,
                    height: 720,
                    fps: 30,
                    bitrate: 4_000_000,
                },
                reason: "sink-level",
            },
        ),
        (
            "no transcoder",
            car_a,
            cb31.clone(),
            None,
            Unsupported {
                reason: "no-transcoder",
            },
        ),
        (
            "H.265 to an H.264-only receiver is transcoded and scaled to the level",
            source("hvc1.1.6.L123.90", 1920, 1080, Some(60.0)),
            cb31.clone(),
            Some(transcoder.clone()),
            Transcode {
                target: TranscodeTarget {
                    profile: CodecProfile::avc(66, 0xE0, 31),
                    width: 1280,
                    height: 720,
                    fps: 30,
                    bitrate: 4_000_000,
                },
                reason: "sink-codec",
            },
        ),
        (
            "H.265 without a hardware H.265 decoder",
            source("hvc1.1.6.L123.90", 1920, 1080, None),
            cb31.clone(),
            Some(support(&[VideoCodec::H264], true)),
            Unsupported {
                reason: "no-decoder",
            },
        ),
        (
            "too large for the level without scaling",
            source("avc1.640028", 1920, 1080, None),
            cb31.clone(),
            Some(support(BOTH, false)),
            Unsupported {
                reason: "no-target",
            },
        ),
        (
            "receiver without an encodable H.264 profile",
            car_a,
            sink(&["avc1.58A01E"]),
            Some(transcoder.clone()),
            Unsupported {
                reason: "no-target",
            },
        ),
        (
            "no encoder",
            car_a,
            cb31.clone(),
            Some(TranscodeSupport {
                decode: BOTH.to_vec(),
                encode: vec![],
                scaling: true,
            }),
            Unsupported {
                reason: "no-encoder",
            },
        ),
        (
            "full-size target on the lowest sufficient Constrained Baseline level",
            source("hvc1.1.6.L123.90", 1920, 1080, Some(30.0)),
            sink(WFD_ALL),
            Some(transcoder.clone()),
            Transcode {
                target: TranscodeTarget {
                    profile: CodecProfile::avc(66, 0xE0, 40),
                    width: 1920,
                    height: 1080,
                    fps: 30,
                    bitrate: 8_000_000,
                },
                reason: "sink-codec",
            },
        ),
    ];
    for (name, source, sink, support, expected) in cases {
        assert_eq!(decide(&source, &sink, support.as_ref()), expected, "{name}");
    }
}

#[test]
fn targets_respect_display_bounds_and_level_frame_rates() {
    let mut bounded = sink(&["avc1.42E01F"]);
    bounded.max_width = Some(800);
    bounded.max_height = Some(480);
    bounded.max_fps = Some(25.0);
    let TranscodeDecision::Transcode { target, .. } = decide(
        &source("avc1.640028", 1280, 720, Some(60.0)),
        &bounded,
        Some(&support(BOTH, true)),
    ) else {
        panic!("expected transcode")
    };
    assert_eq!((target.width, target.height, target.fps), (800, 450, 25));
    assert_eq!(target.bitrate, 1_302_083);

    // A level 3.0 receiver cannot take 720p; the target shrinks to fit 1620 macroblocks.
    let TranscodeDecision::Transcode { target, .. } = decide(
        &source("avc1.640028", 1280, 720, None),
        &sink(&["avc1.42E01E"]),
        Some(&support(BOTH, true)),
    ) else {
        panic!("expected transcode")
    };
    assert!(target.width.div_ceil(16) * target.height.div_ceil(16) <= 1620);
    assert_eq!(target.width % 16, 0);
    assert_eq!(target.height % 2, 0);
    assert_eq!(target.profile.level, 30);
    assert!((target.width as f64 / target.height as f64 - 16.0 / 9.0).abs() < 0.05);
    assert_eq!(target.fps, 25);
}

#[test]
fn descriptions_name_source_target_and_reason() {
    let car_a = source("avc1.640028", 1280, 720, Some(60.0));
    let decision = decide(&car_a, &sink(&["avc1.42E01F"]), Some(&support(BOTH, true)));
    assert_eq!(
        describe(&car_a, &decision),
        "video transcode H264 High 4.0 1280x720 -> H264 CB 3.1 1280x720@30 reason=sink-profile"
    );
    assert_eq!(
        describe(
            &car_a,
            &TranscodeDecision::Passthrough {
                reason: "sink-accepts"
            }
        ),
        "video passthrough H264 High 4.0 1280x720 reason=sink-accepts"
    );
    assert_eq!(
        describe(
            &car_a,
            &TranscodeDecision::Unsupported {
                reason: "no-transcoder"
            }
        ),
        "video unsupported H264 High 4.0 1280x720 reason=no-transcoder"
    );
}

#[test]
fn hevc_encoding_is_capability_gated_and_uses_its_own_level_limits() {
    let source = source("avc1.64002A", 1920, 1080, Some(60.0));
    let receiver = sink(&["hvc1.1.6.L123.B0"]);
    let hardware = TranscodeSupport {
        decode: BOTH.to_vec(),
        encode: vec![VideoCodec::H265],
        scaling: true,
    };
    let TranscodeDecision::Transcode { target, reason } =
        decide(&source, &receiver, Some(&hardware))
    else {
        panic!("HEVC conversion expected")
    };
    assert_eq!(reason, "sink-codec");
    assert_eq!(target.profile.codec, VideoCodec::H265);
    assert_eq!((target.width, target.height, target.fps), (1920, 1080, 30));
    assert_eq!(target.bitrate, 4_000_000);
    assert!(matches!(
        decide(&source, &receiver, Some(&support(BOTH, true))),
        TranscodeDecision::Unsupported { .. }
    ));
    let small = sink(&["hvc1.1.6.L93.B0"]);
    let TranscodeDecision::Transcode { target, .. } = decide(&source, &small, Some(&hardware))
    else {
        panic!("scaled HEVC expected")
    };
    assert!(target.width as u64 * target.height as u64 <= 983040);
    assert_eq!(target.width % 2, 0);
    assert_eq!(target.height % 2, 0);
    assert!(target.width as u64 * target.height as u64 * target.fps as u64 <= 33_177_600);
}
