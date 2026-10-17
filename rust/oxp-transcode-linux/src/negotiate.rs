// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

//! V4L2 device selection, format choice and encoder settings, kept apart from the ioctls so the
//! decisions are testable on any host.

use crate::{TranscodeSupport, TranscodeTarget};
use oxp_core::model::VideoCodec;
use std::path::PathBuf;

pub const fn fourcc(code: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*code)
}
pub const PIX_FMT_H264: u32 = fourcc(b"H264");
pub const PIX_FMT_HEVC: u32 = fourcc(b"HEVC");
pub const PIX_FMT_NV12: u32 = fourcc(b"NV12");
pub const PIX_FMT_NV12M: u32 = fourcc(b"NM12");
/// Raw formats the NV12 copy understands, in order of preference.
pub const RAW_FORMATS: [u32; 2] = [PIX_FMT_NV12, PIX_FMT_NV12M];

pub fn fourcc_name(code: u32) -> String {
    code.to_le_bytes()
        .iter()
        .map(|&b| if b.is_ascii_graphic() { b as char } else { '?' })
        .collect()
}
pub fn codec_fourcc(codec: VideoCodec) -> u32 {
    match codec {
        VideoCodec::H264 => PIX_FMT_H264,
        VideoCodec::H265 => PIX_FMT_HEVC,
    }
}

/// The multi-planar format of one queue, as the driver reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MplaneFormat {
    pub width: u32,
    pub height: u32,
    pub pixelformat: u32,
    /// (bytesperline, sizeimage) per plane.
    pub planes: Vec<(u32, u32)>,
}

/// One video node: its driver and the formats of its two memory-to-memory queues.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    pub path: PathBuf,
    pub driver: String,
    pub card: String,
    /// Streaming multi-planar memory-to-memory device.
    pub m2m: bool,
    /// Formats the device consumes (the OUTPUT queue).
    pub consumes: Vec<u32>,
    /// Formats the device produces (the CAPTURE queue).
    pub produces: Vec<u32>,
}
impl DeviceInfo {
    fn decodes(&self, codec: VideoCodec) -> bool {
        self.m2m
            && self.consumes.contains(&codec_fourcc(codec))
            && RAW_FORMATS.iter().any(|f| self.produces.contains(f))
    }
    fn encodes_avc(&self) -> bool {
        self.m2m
            && self.produces.contains(&PIX_FMT_H264)
            && RAW_FORMATS.iter().any(|f| self.consumes.contains(f))
    }
}

/// A stateful decoder and an H.264 encoder that agree on a raw frame format.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DevicePair {
    pub decoder: DeviceInfo,
    pub encoder: DeviceInfo,
    pub raw_format: u32,
}

/// Every video node found on the host. Stateless (slice-based) decoders and cameras are ignored
/// because they neither consume H264/HEVC bitstreams nor produce them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inventory {
    pub devices: Vec<DeviceInfo>,
}
impl Inventory {
    pub fn select(&self, codec: VideoCodec) -> Option<DevicePair> {
        for decoder in self.devices.iter().filter(|d| d.decodes(codec)) {
            for encoder in self.devices.iter().filter(|d| d.encodes_avc()) {
                let shared = RAW_FORMATS
                    .into_iter()
                    .find(|f| decoder.produces.contains(f) && encoder.consumes.contains(f));
                if let Some(raw_format) = shared {
                    return Some(DevicePair {
                        decoder: decoder.clone(),
                        encoder: encoder.clone(),
                        raw_format,
                    });
                }
            }
        }
        None
    }
    pub fn support(&self) -> Option<TranscodeSupport> {
        let decode: Vec<_> = [VideoCodec::H264, VideoCodec::H265]
            .into_iter()
            .filter(|c| self.select(*c).is_some())
            .collect();
        (!decode.is_empty()).then_some(TranscodeSupport {
            decode,
            encode: vec![VideoCodec::H264],
            // Frames pass through the CPU between the codecs, which can resize them.
            scaling: true,
        })
    }
    pub fn describe(&self) -> String {
        let Some(pair) = self
            .select(VideoCodec::H264)
            .or_else(|| self.select(VideoCodec::H265))
        else {
            let nodes: Vec<_> = self
                .devices
                .iter()
                .map(|d| format!("{} ({})", d.path.display(), d.driver))
                .collect();
            return format!(
                "video transcoder unavailable: no V4L2 M2M decoder and H.264 encoder pair among [{}]",
                nodes.join(", ")
            );
        };
        let codecs: Vec<_> = [(VideoCodec::H264, "H264"), (VideoCodec::H265, "HEVC")]
            .into_iter()
            .filter(|(c, _)| pair.decoder.decodes(*c))
            .map(|(_, n)| n)
            .collect();
        format!(
            "video transcoder decoder={} ({}: {}) encoder={} ({}) raw={}",
            pair.decoder.path.display(),
            pair.decoder.driver,
            codecs.join(","),
            pair.encoder.path.display(),
            pair.encoder.driver,
            fourcc_name(pair.raw_format)
        )
    }
}

const CID_CODEC_BASE: u32 = 0x0099_0900;
pub const CID_MIN_BUFFERS_FOR_CAPTURE: u32 = 0x0098_0900 + 39;
pub const CID_B_FRAMES: u32 = CID_CODEC_BASE + 202;
pub const CID_GOP_SIZE: u32 = CID_CODEC_BASE + 203;
pub const CID_BITRATE: u32 = CID_CODEC_BASE + 207;
pub const CID_HEADER_MODE: u32 = CID_CODEC_BASE + 216;
pub const CID_REPEAT_SEQ_HEADER: u32 = CID_CODEC_BASE + 226;
pub const CID_FORCE_KEY_FRAME: u32 = CID_CODEC_BASE + 229;
pub const CID_H264_8X8_TRANSFORM: u32 = CID_CODEC_BASE + 355;
pub const CID_H264_ENTROPY_MODE: u32 = CID_CODEC_BASE + 357;
pub const CID_H264_I_PERIOD: u32 = CID_CODEC_BASE + 358;
pub const CID_H264_LEVEL: u32 = CID_CODEC_BASE + 359;
pub const CID_H264_PROFILE: u32 = CID_CODEC_BASE + 363;
pub const CID_PREPEND_SPSPPS_TO_IDR: u32 = CID_CODEC_BASE + 644;
pub const CID_DEC_DISPLAY_DELAY: u32 = CID_CODEC_BASE + 653;
pub const CID_DEC_DISPLAY_DELAY_ENABLE: u32 = CID_CODEC_BASE + 654;

/// A control to set before streaming. Values are tried in order until the driver accepts one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlStep {
    pub name: &'static str,
    pub id: u32,
    pub values: Vec<i32>,
    /// A rejected required control fails the session; others are best effort.
    pub required: bool,
}
fn step(name: &'static str, id: u32, values: &[i32], required: bool) -> ControlStep {
    ControlStep {
        name,
        id,
        values: values.to_vec(),
        required,
    }
}

/// The V4L2 menu index of an H.264 level_idc.
pub fn h264_level(level_idc: u8) -> Option<i32> {
    const LEVELS: [u8; 20] = [
        10, 9, 11, 12, 13, 20, 21, 22, 30, 31, 32, 40, 41, 42, 50, 51, 52, 60, 61, 62,
    ];
    LEVELS
        .iter()
        .position(|l| *l == level_idc)
        .map(|i| i as i32)
}
/// V4L2 menu indices for an H.264 profile, preferred first. Some encoders implement
/// Constrained Baseline only under the Baseline entry.
pub fn h264_profiles(profile: u8, constraints: u8) -> Option<&'static [i32]> {
    match profile {
        66 if constraints & 0x40 != 0 => Some(&[1, 0]),
        66 => Some(&[0]),
        77 => Some(&[2]),
        100 => Some(&[4]),
        _ => None,
    }
}

/// Encoder settings for a low-latency H.264 stream whose IDR frames repeat SPS and PPS.
pub fn encoder_controls(target: &TranscodeTarget) -> Result<Vec<ControlStep>, String> {
    let p = target.profile;
    let profiles = h264_profiles(p.profile, p.constraints)
        .ok_or_else(|| format!("no V4L2 profile for H.264 profile_idc {}", p.profile))?;
    let level =
        h264_level(p.level).ok_or_else(|| format!("no V4L2 level for level_idc {}", p.level))?;
    let gop = target.fps.clamp(1, 120) as i32;
    let mut steps = vec![
        step("H264 profile", CID_H264_PROFILE, profiles, true),
        step("H264 level", CID_H264_LEVEL, &[level], true),
        step("bitrate", CID_BITRATE, &[target.bitrate as i32], true),
        step("B frames", CID_B_FRAMES, &[0], false),
        step("GOP size", CID_GOP_SIZE, &[gop], false),
        step("H264 I period", CID_H264_I_PERIOD, &[gop], false),
        // JOINED_WITH_1ST_FRAME: SPS and PPS share the IDR buffer.
        step("header mode", CID_HEADER_MODE, &[1], false),
        step("repeat sequence header", CID_REPEAT_SEQ_HEADER, &[1], false),
        step(
            "prepend SPS/PPS to IDR",
            CID_PREPEND_SPSPPS_TO_IDR,
            &[1],
            false,
        ),
    ];
    if p.profile != 100 {
        steps.push(step("H264 CAVLC", CID_H264_ENTROPY_MODE, &[0], false));
        steps.push(step(
            "H264 8x8 transform",
            CID_H264_8X8_TRANSFORM,
            &[0],
            false,
        ));
    }
    Ok(steps)
}
/// Decoder settings: emit pictures in decode order as soon as they are decoded.
pub fn decoder_controls() -> Vec<ControlStep> {
    vec![
        step(
            "display delay enable",
            CID_DEC_DISPLAY_DELAY_ENABLE,
            &[1],
            false,
        ),
        step("display delay", CID_DEC_DISPLAY_DELAY, &[0], false),
    ]
}

/// Compressed buffer size for a picture; an IDR rarely exceeds 1.5 bytes per pixel.
pub fn bitstream_buffer_size(width: u32, height: u32) -> u32 {
    (width.saturating_mul(height).saturating_mul(3) / 2)
        .max(1 << 20)
        .next_multiple_of(4096)
}
/// Encoder output size; the target bitrate keeps frames far below this.
pub fn encoded_buffer_size(width: u32, height: u32) -> u32 {
    (width.saturating_mul(height) / 2)
        .max(512 << 10)
        .next_multiple_of(4096)
}
pub const DECODER_INPUT_BUFFERS: u32 = 6;
pub const ENCODER_INPUT_BUFFERS: u32 = 4;
pub const ENCODER_OUTPUT_BUFFERS: u32 = 4;
/// Decoded picture buffers: the driver's minimum (its DPB) plus two in flight to the encoder.
pub fn capture_buffer_count(driver_minimum: Option<i32>) -> u32 {
    driver_minimum.unwrap_or(4).clamp(1, 32) as u32 + 2
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxp_media::transcode_policy::CodecProfile;

    fn device(
        path: &str,
        driver: &str,
        m2m: bool,
        consumes: &[u32],
        produces: &[u32],
    ) -> DeviceInfo {
        DeviceInfo {
            path: path.into(),
            driver: driver.into(),
            card: driver.into(),
            m2m,
            consumes: consumes.to_vec(),
            produces: produces.to_vec(),
        }
    }
    fn venus() -> Vec<DeviceInfo> {
        vec![
            device(
                "/dev/video0",
                "qcom-venus",
                true,
                &[PIX_FMT_H264, PIX_FMT_HEVC, fourcc(b"VP80")],
                &[PIX_FMT_NV12],
            ),
            device(
                "/dev/video1",
                "qcom-venus",
                true,
                &[PIX_FMT_NV12],
                &[PIX_FMT_H264, PIX_FMT_HEVC],
            ),
        ]
    }

    #[test]
    fn venus_pair_is_found_by_capability_in_any_order() {
        let mut devices = venus();
        devices.reverse();
        devices.insert(
            0,
            device("/dev/video2", "uvcvideo", false, &[], &[fourcc(b"YUYV")]),
        );
        let inventory = Inventory { devices };
        let pair = inventory.select(VideoCodec::H264).unwrap();
        assert_eq!(pair.decoder.path, PathBuf::from("/dev/video0"));
        assert_eq!(pair.encoder.path, PathBuf::from("/dev/video1"));
        assert_eq!(pair.raw_format, PIX_FMT_NV12);
        assert_eq!(
            inventory.select(VideoCodec::H265).unwrap().decoder,
            pair.decoder
        );
        assert_eq!(
            inventory.support().unwrap(),
            TranscodeSupport {
                decode: vec![VideoCodec::H264, VideoCodec::H265],
                encode: vec![VideoCodec::H264],
                scaling: true,
            }
        );
        assert_eq!(
            inventory.describe(),
            "video transcoder decoder=/dev/video0 (qcom-venus: H264,HEVC) encoder=/dev/video1 (qcom-venus) raw=NV12"
        );
    }

    #[test]
    fn raw_formats_must_match_and_stateless_or_capture_nodes_are_ignored() {
        let stateless = device(
            "/dev/video3",
            "hantro-vpu",
            true,
            &[fourcc(b"S264")],
            &[PIX_FMT_NV12],
        );
        let nv12m_encoder = device(
            "/dev/video4",
            "enc",
            true,
            &[PIX_FMT_NV12M],
            &[PIX_FMT_H264],
        );
        let decoder = device(
            "/dev/video5",
            "dec",
            true,
            &[PIX_FMT_H264],
            &[PIX_FMT_NV12, PIX_FMT_NV12M],
        );
        let nv12_only = device(
            "/dev/video6",
            "dec2",
            true,
            &[PIX_FMT_H264],
            &[PIX_FMT_NV12],
        );
        let not_m2m = device(
            "/dev/video7",
            "enc2",
            false,
            &[PIX_FMT_NV12],
            &[PIX_FMT_H264],
        );
        let inventory = Inventory {
            devices: vec![
                stateless.clone(),
                nv12_only.clone(),
                not_m2m.clone(),
                nv12m_encoder.clone(),
            ],
        };
        assert!(inventory.select(VideoCodec::H264).is_none());
        assert!(inventory.support().is_none());
        assert!(inventory.describe().contains("unavailable"));
        let inventory = Inventory {
            devices: vec![
                stateless,
                nv12_only,
                decoder.clone(),
                not_m2m,
                nv12m_encoder.clone(),
            ],
        };
        let pair = inventory.select(VideoCodec::H264).unwrap();
        assert_eq!(
            (pair.decoder, pair.encoder, pair.raw_format),
            (decoder, nv12m_encoder, PIX_FMT_NV12M)
        );
        assert_eq!(inventory.support().unwrap().decode, vec![VideoCodec::H264]);
        assert!(Inventory::default().support().is_none());
    }

    #[test]
    fn encoder_controls_for_constrained_baseline() {
        let target = TranscodeTarget {
            profile: CodecProfile::avc(66, 0xE0, 31),
            width: 1280,
            height: 720,
            fps: 30,
            bitrate: 4_000_000,
        };
        let steps = encoder_controls(&target).unwrap();
        let get = |id| steps.iter().find(|s| s.id == id).unwrap();
        assert_eq!(get(CID_H264_PROFILE).values, vec![1, 0]);
        assert!(get(CID_H264_PROFILE).required);
        assert_eq!(get(CID_H264_LEVEL).values, vec![9]);
        assert_eq!(get(CID_BITRATE).values, vec![4_000_000]);
        assert_eq!(get(CID_GOP_SIZE).values, vec![30]);
        assert_eq!(get(CID_B_FRAMES).values, vec![0]);
        assert_eq!(get(CID_HEADER_MODE).values, vec![1]);
        assert_eq!(get(CID_H264_ENTROPY_MODE).values, vec![0]);
        assert!(!get(CID_PREPEND_SPSPPS_TO_IDR).required);
        assert_eq!(steps.iter().filter(|s| s.required).count(), 3);
        let high = TranscodeTarget {
            profile: CodecProfile::avc(100, 0, 40),
            ..target
        };
        let steps = encoder_controls(&high).unwrap();
        assert_eq!(steps[0].values, vec![4]);
        assert!(steps.iter().all(|s| s.id != CID_H264_ENTROPY_MODE));
        let extended = TranscodeTarget {
            profile: CodecProfile::avc(88, 0, 30),
            ..target
        };
        assert!(encoder_controls(&extended).is_err());
        let level = TranscodeTarget {
            profile: CodecProfile::avc(66, 0xE0, 33),
            ..target
        };
        assert!(encoder_controls(&level).is_err());
    }

    #[test]
    fn level_menu_and_buffer_sizing() {
        for (idc, menu) in [
            (10, 0),
            (9, 1),
            (11, 2),
            (30, 8),
            (31, 9),
            (40, 11),
            (42, 13),
            (62, 19),
        ] {
            assert_eq!(h264_level(idc), Some(menu));
        }
        assert_eq!(h264_level(33), None);
        assert_eq!(h264_profiles(77, 0x40), Some(&[2][..]));
        assert_eq!(h264_profiles(66, 0), Some(&[0][..]));
        assert_eq!(bitstream_buffer_size(1280, 720), 1_384_448);
        assert_eq!(bitstream_buffer_size(320, 240), 1 << 20);
        assert_eq!(encoded_buffer_size(1280, 720), 524_288);
        assert_eq!(encoded_buffer_size(1920, 1080), 1_040_384);
        assert_eq!(capture_buffer_count(None), 6);
        assert_eq!(capture_buffer_count(Some(17)), 19);
        assert_eq!(capture_buffer_count(Some(-3)), 3);
        assert_eq!(fourcc_name(PIX_FMT_NV12M), "NM12");
        assert_eq!(decoder_controls().len(), 2);
    }
}
