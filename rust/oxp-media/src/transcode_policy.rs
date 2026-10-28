// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

//! Chooses between forwarding a video stream unchanged, transcoding it for a sink, or reporting
//! it as unplayable. Inputs are codec strings and sizes, so every output and every transcoder
//! implementation shares one decision.

use crate::VideoCodec;
use std::fmt;

/// One codec configuration parsed from an RFC 6381 codec string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CodecProfile {
    pub codec: VideoCodec,
    /// AVC profile_idc or HEVC general_profile_idc.
    pub profile: u8,
    /// AVC constraint_set flags byte; zero for HEVC.
    pub constraints: u8,
    /// AVC level_idc (31 is 3.1) or HEVC general_level_idc (93 is 3.1).
    pub level: u8,
    /// HEVC high tier; false for AVC.
    pub high_tier: bool,
}
impl CodecProfile {
    pub fn avc(profile: u8, constraints: u8, level: u8) -> Self {
        Self {
            codec: VideoCodec::H264,
            profile,
            constraints,
            level,
            high_tier: false,
        }
    }
    /// Accepts `avc1.PPCCLL`, `avc3.PPCCLL` and `hvc1`/`hev1` strings such as `hvc1.1.6.L93.B0`.
    pub fn parse(value: &str) -> Option<Self> {
        let (kind, rest) = value.split_once('.')?;
        match kind {
            "avc1" | "avc3" => {
                if rest.len() != 6 || !rest.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return None;
                }
                let byte = |i: usize| u8::from_str_radix(&rest[i..i + 2], 16).ok();
                Some(Self::avc(byte(0)?, byte(2)?, byte(4)?))
            }
            "hvc1" | "hev1" => {
                let mut fields = rest.split('.');
                let profile = fields.next()?;
                let profile = profile
                    .strip_prefix(['A', 'B', 'C'])
                    .unwrap_or(profile)
                    .parse()
                    .ok()?;
                u32::from_str_radix(fields.next()?, 16).ok()?;
                let tier = fields.next()?;
                let high_tier = match tier.as_bytes().first()? {
                    b'L' => false,
                    b'H' => true,
                    _ => return None,
                };
                Some(Self {
                    codec: VideoCodec::H265,
                    profile,
                    constraints: 0,
                    level: tier[1..].parse().ok()?,
                    high_tier,
                })
            }
            _ => None,
        }
    }
    fn constrained_baseline(&self) -> bool {
        self.profile == 66 && self.constraints & 0x40 != 0
    }
    /// Profile compatibility only; `accepts` adds the level and tier limits.
    fn decodes_profile(&self, source: &Self) -> bool {
        if self.codec != source.codec {
            return false;
        }
        match self.codec {
            // High decoders also decode Main and Baseline streams; Main decodes Constrained Baseline.
            VideoCodec::H264 => match self.profile {
                100 => matches!(source.profile, 66 | 77 | 100),
                77 => source.profile == 77 || source.constrained_baseline(),
                p => p == source.profile,
            },
            // Main 10 decoders also decode Main.
            VideoCodec::H265 => {
                self.profile == source.profile || (self.profile == 2 && source.profile == 1)
            }
        }
    }
    /// Whether a decoder described by `self` plays a stream described by `source`.
    pub fn accepts(&self, source: &Self) -> bool {
        self.decodes_profile(source)
            && self.level >= source.level
            && (self.high_tier || !source.high_tier)
    }
    pub fn profile_name(&self) -> String {
        match (self.codec, self.profile) {
            (VideoCodec::H264, 66) if self.constrained_baseline() => "CB".into(),
            (VideoCodec::H264, 66) => "Baseline".into(),
            (VideoCodec::H264, 77) => "Main".into(),
            (VideoCodec::H264, 88) => "Extended".into(),
            (VideoCodec::H264, 100) => "High".into(),
            (VideoCodec::H264, 110) => "High10".into(),
            (VideoCodec::H264, 122) => "High422".into(),
            (VideoCodec::H264, 244) => "High444".into(),
            (VideoCodec::H265, 1) => "Main".into(),
            (VideoCodec::H265, 2) => "Main10".into(),
            (VideoCodec::H265, 3) => "MainStill".into(),
            (VideoCodec::H265, 4) => "RExt".into(),
            (_, p) => format!("P{p}"),
        }
    }
    pub fn level_name(&self) -> String {
        match self.codec {
            VideoCodec::H264 if self.level == 9 => "1b".into(),
            VideoCodec::H264 => format!("{}.{}", self.level / 10, self.level % 10),
            VideoCodec::H265 => format!("{}.{}", self.level / 30, self.level % 30 / 3),
        }
    }
    /// The RFC 6381 string for AVC targets, for example `avc1.42E01F`.
    pub fn codec_string(&self) -> String {
        match self.codec {
            VideoCodec::H264 => format!(
                "avc1.{:02X}{:02X}{:02X}",
                self.profile, self.constraints, self.level
            ),
            VideoCodec::H265 => format!(
                "hvc1.{}.0.{}{}",
                self.profile,
                if self.high_tier { 'H' } else { 'L' },
                self.level
            ),
        }
    }
}
impl fmt::Display for CodecProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let codec = match self.codec {
            VideoCodec::H264 => "H264",
            VideoCodec::H265 => "H265",
        };
        write!(f, "{codec} {} {}", self.profile_name(), self.level_name())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SourceVideo {
    pub profile: CodecProfile,
    pub width: u32,
    pub height: u32,
    pub fps: Option<f64>,
}
impl fmt::Display for SourceVideo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}x{}", self.profile, self.width, self.height)
    }
}

/// What a receiver decodes. Bounds limit transcode targets only, never passthrough.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SinkVideo {
    pub decoders: Vec<CodecProfile>,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub max_fps: Option<f64>,
}
impl SinkVideo {
    /// Unparseable codec strings are ignored.
    pub fn from_codec_strings<'a>(values: impl IntoIterator<Item = &'a str>) -> Self {
        Self {
            decoders: values.into_iter().filter_map(CodecProfile::parse).collect(),
            ..Default::default()
        }
    }
    pub fn accepts(&self, source: &CodecProfile) -> bool {
        self.decoders.iter().any(|d| d.accepts(source))
    }
}

/// What the local transcoder can do; absent when the host has none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscodeSupport {
    pub decode: Vec<VideoCodec>,
    pub encode: Vec<VideoCodec>,
    /// Whether the target may be smaller than the source.
    pub scaling: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TranscodeTarget {
    pub profile: CodecProfile,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate: u32,
}
impl fmt::Display for TranscodeTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {}x{}@{}",
            self.profile, self.width, self.height, self.fps
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum TranscodeDecision {
    Passthrough {
        reason: &'static str,
    },
    Transcode {
        target: TranscodeTarget,
        reason: &'static str,
    },
    Unsupported {
        reason: &'static str,
    },
}
impl TranscodeDecision {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Passthrough { reason }
            | Self::Transcode { reason, .. }
            | Self::Unsupported { reason } => reason,
        }
    }
}

/// One diagnostic line, for example
/// `video transcode H264 High 4.0 1280x720 -> H264 CB 3.1 1280x720@30 reason=sink-profile`.
pub fn describe(source: &SourceVideo, decision: &TranscodeDecision) -> String {
    match decision {
        TranscodeDecision::Passthrough { reason } => {
            format!("video passthrough {source} reason={reason}")
        }
        TranscodeDecision::Transcode { target, reason } => {
            format!("video transcode {source} -> {target} reason={reason}")
        }
        TranscodeDecision::Unsupported { reason } => {
            format!("video unsupported {source} reason={reason}")
        }
    }
}

/// Reasons: `sink-unknown` and `sink-accepts` pass through; `sink-codec`, `sink-profile` and
/// `sink-level` explain a transcode; `no-transcoder`, `no-decoder`, `no-encoder` and `no-target`
/// explain why the stream cannot be made playable.
pub fn decide(
    source: &SourceVideo,
    sink: &SinkVideo,
    support: Option<&TranscodeSupport>,
) -> TranscodeDecision {
    use TranscodeDecision::*;
    if sink.decoders.is_empty() {
        return Passthrough {
            reason: "sink-unknown",
        };
    }
    if sink.accepts(&source.profile) {
        return Passthrough {
            reason: "sink-accepts",
        };
    }
    let same_codec: Vec<_> = sink
        .decoders
        .iter()
        .filter(|d| d.codec == source.profile.codec)
        .collect();
    let reason = if same_codec.is_empty() {
        "sink-codec"
    } else if same_codec
        .iter()
        .any(|d| d.decodes_profile(&source.profile))
    {
        "sink-level"
    } else {
        "sink-profile"
    };
    let Some(support) = support else {
        return Unsupported {
            reason: "no-transcoder",
        };
    };
    if !support.decode.contains(&source.profile.codec) {
        return Unsupported {
            reason: "no-decoder",
        };
    }
    if support.encode.is_empty() {
        return Unsupported {
            reason: "no-encoder",
        };
    }
    match target(source, sink, support) {
        Some(target) => Transcode { target, reason },
        None => Unsupported {
            reason: "no-target",
        },
    }
}

/// H.264 Table A-1: (level_idc, MaxMBPS, MaxFS, MaxBR in kbit/s for Baseline and Main).
const AVC_LEVELS: [(u8, u32, u32, u32); 19] = [
    (10, 1_485, 99, 64),
    (11, 3_000, 396, 192),
    (12, 6_000, 396, 384),
    (13, 11_880, 396, 768),
    (20, 11_880, 396, 2_000),
    (21, 19_800, 792, 4_000),
    (22, 20_250, 1_620, 4_000),
    (30, 40_500, 1_620, 10_000),
    (31, 108_000, 3_600, 14_000),
    (32, 216_000, 5_120, 20_000),
    (40, 245_760, 8_192, 20_000),
    (41, 245_760, 8_192, 50_000),
    (42, 522_240, 8_704, 50_000),
    (50, 589_824, 22_080, 135_000),
    (51, 983_040, 36_864, 240_000),
    (52, 2_073_600, 36_864, 240_000),
    (60, 4_177_920, 139_264, 240_000),
    (61, 8_355_840, 139_264, 480_000),
    (62, 16_711_680, 139_264, 800_000),
];
fn avc_limits(level: u8) -> Option<(u32, u32, u32)> {
    AVC_LEVELS
        .iter()
        .find(|(l, ..)| *l == level)
        .map(|&(_, mbps, fs, br)| (mbps, fs, br))
}
fn macroblocks(width: u32, height: u32) -> u32 {
    width.div_ceil(16) * height.div_ceil(16)
}
/// Profiles a hardware encoder produces, in order of receiver compatibility.
const AVC_TARGET_PROFILES: [u8; 3] = [66, 77, 100];
/// Frame rate assumed when the source does not announce one: the Wi-Fi Display native rate.
const DEFAULT_FPS: f64 = 30.0;

struct Plan {
    width: u32,
    height: u32,
    fps: u32,
}
fn plan(source: &SourceVideo, sink: &SinkVideo, level: u8, scaling: bool) -> Option<Plan> {
    let (max_mbps, max_fs, _) = avc_limits(level)?;
    // A.3.1: neither dimension may exceed sqrt(8 * MaxFS) macroblocks.
    let dimension = ((8.0 * max_fs as f64).sqrt() as u32) * 16;
    let bound_w = sink.max_width.unwrap_or(u32::MAX).min(dimension);
    let bound_h = sink.max_height.unwrap_or(u32::MAX).min(dimension);
    let fits = |w: u32, h: u32| w <= bound_w && h <= bound_h && macroblocks(w, h) <= max_fs;
    let (mut width, mut height) = (source.width, source.height);
    if width == 0 || height == 0 {
        return None;
    }
    if !fits(width, height) {
        if !scaling {
            return None;
        }
        // Keep the aspect ratio with a macroblock-aligned width and an even height.
        let aspect = width as f64 / height as f64;
        let mut w = width.min(bound_w) & !15;
        loop {
            let h = (w as f64 / aspect / 2.0) as u32 * 2;
            if w < 16 {
                return None;
            }
            if h >= 2 && fits(w, h) {
                (width, height) = (w, h);
                break;
            }
            w -= 16;
        }
    }
    let mut fps = source.fps.unwrap_or(DEFAULT_FPS);
    if let Some(max) = sink.max_fps.filter(|v| *v > 0.0) {
        fps = fps.min(max);
    }
    let fps = (fps.min(max_mbps as f64 / macroblocks(width, height) as f64) as u32).min(240);
    (fps > 0).then_some(Plan { width, height, fps })
}
fn avc_target(source: &SourceVideo, sink: &SinkVideo, scaling: bool) -> Option<TranscodeTarget> {
    let mut best: Option<(u64, u8, Plan)> = None;
    for profile in AVC_TARGET_PROFILES {
        let Some(top) = sink
            .decoders
            .iter()
            .filter(|d| d.codec == VideoCodec::H264 && d.profile == profile)
            .filter(|d| avc_limits(d.level).is_some())
            .max_by_key(|d| d.level)
        else {
            continue;
        };
        if let Some(p) = plan(source, sink, top.level, scaling) {
            let rate = p.width as u64 * p.height as u64 * p.fps as u64;
            // Earlier profiles win ties: Constrained Baseline is the most widely decoded.
            if best.as_ref().is_none_or(|(r, ..)| rate > *r) {
                best = Some((rate, profile, p));
            }
        }
    }
    let (_, profile, p) = best?;
    // The lowest level that carries the plan keeps the stream playable on the most decoders.
    let mut levels: Vec<_> = sink
        .decoders
        .iter()
        .filter(|d| d.codec == VideoCodec::H264 && d.profile == profile)
        .filter_map(|d| avc_limits(d.level).map(|limits| (d, limits)))
        .collect();
    levels.sort_by_key(|(d, _)| d.level);
    let (decoder, (_, _, max_br)) = levels.into_iter().find(|(_, (mbps, fs, _))| {
        let mbs = macroblocks(p.width, p.height);
        mbs <= *fs && mbs as u64 * p.fps as u64 <= *mbps as u64
    })?;
    // About 4 Mbit/s for 720p30, scaled with the pixel rate and capped by the level.
    let pixel_rate = p.width as u64 * p.height as u64 * p.fps as u64;
    let level_cap = max_br as u64 * if profile == 100 { 1250 } else { 1000 };
    let bitrate = (pixel_rate * 4_000_000 / (1280 * 720 * 30))
        .clamp(1_000_000, 8_000_000)
        .min(level_cap) as u32;
    let constraints = match profile {
        // Constrained Baseline: constraint_set0, 1 and 2, as Wi-Fi Display announces it.
        66 => 0xE0,
        77 => 0x40,
        _ => 0,
    };
    Some(TranscodeTarget {
        profile: CodecProfile::avc(profile, constraints, decoder.level),
        width: p.width,
        height: p.height,
        fps: p.fps,
        bitrate,
    })
}

/// Chooses an encodable receiver target even when the source profile is already accepted
/// (for example, when an output must crop or stretch the picture).
pub fn target(
    source: &SourceVideo,
    sink: &SinkVideo,
    support: &TranscodeSupport,
) -> Option<TranscodeTarget> {
    if !support.decode.contains(&source.profile.codec) {
        return None;
    }
    if support.encode.contains(&VideoCodec::H264) {
        if let Some(target) = avc_target(source, sink, support.scaling) {
            return Some(target);
        }
    }
    if !support.encode.contains(&VideoCodec::H265) {
        return None;
    }
    // Main tier HEVC limits, matching the codec-specific WFD limits used by Carlink.
    let decoder = sink
        .decoders
        .iter()
        .filter(|d| {
            d.codec == VideoCodec::H265
                && d.profile == 1
                && !d.high_tier
                && [30, 60, 63, 90, 93, 120, 123, 150, 153, 156, 180, 183, 186].contains(&d.level)
        })
        .max_by_key(|d| d.level)?;
    let (max_pixels, max_rate) = match decoder.level {
        30 => (36_864, 552_960),
        60 => (122_880, 3_686_400),
        63 => (245_760, 7_372_800),
        90 => (552_960, 16_588_800),
        93 => (983_040, 33_177_600),
        120 => (2_228_224, 66_846_720),
        123 => (2_228_224, 133_693_440),
        150 => (8_912_896, 267_386_880),
        153 => (8_912_896, 534_773_760),
        156 => (8_912_896, 1_069_547_520),
        180 => (35_651_584, 1_069_547_520),
        183 => (35_651_584, 2_139_095_040),
        186 => (35_651_584, 4_278_190_080u64),
        _ => return None,
    };
    let (mut width, mut height) = (source.width, source.height);
    if width < 2 || height < 2 {
        return None;
    }
    let scale = (max_pixels as f64 / (width as f64 * height as f64))
        .sqrt()
        .min(1.0)
        .min(sink.max_width.unwrap_or(width) as f64 / width as f64)
        .min(sink.max_height.unwrap_or(height) as f64 / height as f64);
    if scale < 1.0 && !support.scaling {
        return None;
    }
    width = (width as f64 * scale) as u32 & !1;
    height = (height as f64 * scale) as u32 & !1;
    if width < 2 || height < 2 {
        return None;
    }
    let pixels = width as u64 * height as u64;
    let fps = source
        .fps
        .unwrap_or(DEFAULT_FPS)
        .min(sink.max_fps.unwrap_or(240.0))
        .min(max_rate as f64 / pixels as f64)
        .min(30.0) as u32;
    (fps > 0).then_some(TranscodeTarget {
        profile: *decoder,
        width,
        height,
        fps,
        // Android uses 4 Mbit/s and a 30 fps encoder for HEVC output.
        bitrate: 4_000_000,
    })
}
