// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

//! A deterministic transcoder for tests on hosts without codec hardware.

use crate::{
    EncodedFrame, Result, TranscodeConfig, TranscodeSupport, Transcoder, TranscoderFactory,
};
use oxp_media::{AvcConfig, VideoCodec};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

/// Builds a minimal SPS and PPS describing `width`x`height` 4:2:0 progressive video.
pub fn avc_parameter_sets(
    profile: u8,
    constraints: u8,
    level: u8,
    width: u32,
    height: u32,
) -> (Vec<u8>, Vec<u8>) {
    let mut b = BitWriter::default();
    b.put(profile as u32, 8);
    b.put(constraints as u32, 8);
    b.put(level as u32, 8);
    b.ue(0);
    if [
        100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135, 144,
    ]
    .contains(&profile)
    {
        b.ue(1);
        b.ue(0);
        b.ue(0);
        b.put(0, 1);
        b.put(0, 1);
    }
    b.ue(0);
    b.ue(2);
    b.ue(1);
    b.put(0, 1);
    let (mbs_w, mbs_h) = (width.div_ceil(16), height.div_ceil(16));
    b.ue(mbs_w - 1);
    b.ue(mbs_h - 1);
    b.put(1, 1);
    b.put(1, 1);
    let (crop_x, crop_y) = ((mbs_w * 16 - width) / 2, (mbs_h * 16 - height) / 2);
    if crop_x > 0 || crop_y > 0 {
        b.put(1, 1);
        b.ue(0);
        b.ue(crop_x);
        b.ue(0);
        b.ue(crop_y);
    } else {
        b.put(0, 1);
    }
    b.put(0, 1);
    let sps = b.finish(0x67);
    // pps_id 0, sps_id 0, CAVLC, one slice group, one reference, QP 26, deblocking control.
    (sps, vec![0x68, 0xCE, 0x3C, 0x80])
}

#[derive(Default)]
struct BitWriter {
    bits: Vec<bool>,
}
impl BitWriter {
    fn put(&mut self, value: u32, count: u32) {
        for i in (0..count).rev() {
            self.bits.push(value >> i & 1 == 1);
        }
    }
    fn ue(&mut self, value: u32) {
        let n = value + 1;
        let length = 32 - n.leading_zeros();
        self.put(0, length - 1);
        self.put(n, length);
    }
    /// Appends the RBSP stop bit and emulation prevention bytes behind a NAL header.
    fn finish(mut self, header: u8) -> Vec<u8> {
        self.bits.push(true);
        while !self.bits.len().is_multiple_of(8) {
            self.bits.push(false);
        }
        let mut out = vec![header];
        let mut zeros = 0;
        for chunk in self.bits.chunks(8) {
            let byte = chunk.iter().fold(0u8, |v, b| v << 1 | *b as u8);
            if zeros >= 2 && byte <= 3 {
                out.push(3);
                zeros = 0;
            }
            zeros = if byte == 0 { zeros + 1 } else { 0 };
            out.push(byte);
        }
        out
    }
}

/// Counters shared between a fake factory, its transcoders and the test.
#[derive(Default)]
pub struct FakeStats {
    pub geometry_updates: AtomicUsize,
    pub opened: AtomicUsize,
    pub submitted: AtomicUsize,
    pub encoded: AtomicUsize,
    pub forced: AtomicUsize,
    pub resets: AtomicUsize,
    pub closed: AtomicUsize,
}
impl FakeStats {
    pub fn get(counter: &AtomicUsize) -> usize {
        counter.load(Ordering::Acquire)
    }
}

pub struct FakeFactory {
    pub support: Option<TranscodeSupport>,
    pub encoders: Option<Vec<crate::EncoderCapability>>,
    /// Makes `open` fail with this message.
    pub open_error: Option<String>,
    /// Makes `poll` fail once this many frames have been encoded.
    pub fail_after: Option<usize>,
    /// Encoder profile written into the SPS; None uses the target profile.
    pub encoder_profile: Option<(u8, u8)>,
    pub stats: Arc<FakeStats>,
    /// The last opened configuration.
    pub opened: Mutex<Option<TranscodeConfig>>,
}
impl FakeFactory {
    pub fn new(support: Option<TranscodeSupport>) -> Self {
        Self {
            support,
            encoders: None,
            open_error: None,
            fail_after: None,
            encoder_profile: None,
            stats: Default::default(),
            opened: Mutex::new(None),
        }
    }
}
impl TranscoderFactory for FakeFactory {
    fn support(&self) -> Option<TranscodeSupport> {
        self.support.clone()
    }
    fn encoders(&self) -> Vec<crate::EncoderCapability> {
        self.encoders.clone().unwrap_or_else(|| {
            self.support.as_ref().map_or_else(Vec::new, |s| {
                s.encode
                    .iter()
                    .map(|&codec| crate::EncoderCapability {
                        codec,
                        max_width: None,
                        max_height: None,
                    })
                    .collect()
            })
        })
    }
    fn open(&self, config: &TranscodeConfig) -> Result<Box<dyn Transcoder>> {
        if let Some(e) = &self.open_error {
            return Err(e.clone().into());
        }
        self.stats.opened.fetch_add(1, Ordering::AcqRel);
        *self.opened.lock().unwrap() = Some(config.clone());
        let t = config.target;
        let (profile, constraints) = self
            .encoder_profile
            .unwrap_or((t.profile.profile, t.profile.constraints));
        let config: Arc<[u8]> = if t.profile.codec == VideoCodec::H265 {
            let (vps, sps, pps) = hevc_parameter_sets(t.profile.level, t.width, t.height);
            crate::annexb::hevc_configuration(&vps, &sps, &pps)?.into()
        } else {
            let (sps, pps) =
                avc_parameter_sets(profile, constraints, t.profile.level, t.width, t.height);
            AvcConfig::from_parameter_sets(&sps, &pps)?.into()
        };
        Ok(Box::new(FakeTranscoder {
            codec: t.profile.codec,
            config,
            pending: VecDeque::new(),
            awaiting_key: true,
            force: true,
            fail_after: self.fail_after,
            stats: self.stats.clone(),
        }))
    }
    fn describe(&self) -> String {
        "fake transcoder".into()
    }
}

/// Encodes each submitted frame into a one-slice access unit that carries the input timestamp.
pub struct FakeTranscoder {
    codec: VideoCodec,
    config: Arc<[u8]>,
    pending: VecDeque<i64>,
    awaiting_key: bool,
    force: bool,
    fail_after: Option<usize>,
    stats: Arc<FakeStats>,
}
impl FakeTranscoder {
    /// The slice payload: the timestamp the frame was submitted with.
    pub fn decode_pts(data: &[u8]) -> Option<i64> {
        Some(i64::from_be_bytes(data.get(5..13)?.try_into().ok()?))
    }
}
impl Transcoder for FakeTranscoder {
    fn update_crop(&mut self, _: Option<oxp_core::model::ViewArea>) -> Result<()> {
        self.stats.geometry_updates.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    fn submit(&mut self, data: &[u8], pts_us: i64, key_frame: bool) -> Result<bool> {
        if data.is_empty() {
            return Err("empty access unit".into());
        }
        if self.awaiting_key && !key_frame {
            return Ok(true);
        }
        if self.pending.len() >= 4 {
            return Ok(false);
        }
        self.awaiting_key = false;
        self.pending.push_back(pts_us);
        self.stats.submitted.fetch_add(1, Ordering::AcqRel);
        Ok(true)
    }
    fn poll(&mut self, out: &mut EncodedFrame, timeout: Duration) -> Result<bool> {
        let Some(pts) = self.pending.pop_front() else {
            std::thread::sleep(timeout.min(Duration::from_millis(1)));
            return Ok(false);
        };
        let encoded = self.stats.encoded.load(Ordering::Acquire);
        if self.fail_after.is_some_and(|n| encoded >= n) {
            return Err("fake encoder failure".into());
        }
        let key = std::mem::take(&mut self.force);
        out.data.clear();
        if self.codec == VideoCodec::H265 {
            out.data.extend_from_slice(&10u32.to_be_bytes());
            out.data
                .extend_from_slice(&[if key { 0x26 } else { 0x02 }, 1]);
        } else {
            out.data.extend_from_slice(&9u32.to_be_bytes());
            out.data.push(if key { 0x65 } else { 0x41 });
        }
        out.data.extend_from_slice(&pts.to_be_bytes());
        out.pts_us = pts;
        out.key_frame = key;
        out.config = Some(self.config.clone());
        self.stats.encoded.fetch_add(1, Ordering::AcqRel);
        Ok(true)
    }
    fn force_key_frame(&mut self) -> Result<()> {
        self.force = true;
        self.stats.forced.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
    fn reset(&mut self) -> Result<()> {
        self.pending.clear();
        self.awaiting_key = true;
        self.stats.resets.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
}
impl Drop for FakeTranscoder {
    fn drop(&mut self) {
        self.stats.closed.fetch_add(1, Ordering::AcqRel);
    }
}

/// Minimal Main profile parameter sets for deterministic HEVC tests.
pub fn hevc_parameter_sets(level: u8, width: u32, height: u32) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut b = BitWriter::default();
    b.put(0, 4);
    b.put(0, 3);
    b.put(1, 1);
    b.put(1, 8);
    b.put(0x6000_0000, 32);
    b.put(0xB0, 8);
    b.put(0, 32);
    b.put(0, 8);
    b.put(level as u32, 8);
    b.ue(0);
    b.ue(1);
    b.ue(width);
    b.ue(height);
    b.put(0, 1);
    let mut sps = b.finish(0x42);
    sps.insert(1, 1);
    (vec![0x40, 1, 0x80], sps, vec![0x44, 1, 0xC0])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generated_parameter_sets_parse_with_cropping() {
        for (profile, constraints, level, w, h, codec) in [
            (66, 0xE0, 31, 1280, 720, "avc1.42E01F"),
            (100, 0, 40, 1920, 1080, "avc1.640028"),
            (66, 0xE0, 30, 848, 476, "avc1.42E01E"),
        ] {
            let (sps, pps) = avc_parameter_sets(profile, constraints, level, w, h);
            let config =
                AvcConfig::parse(&AvcConfig::from_parameter_sets(&sps, &pps).unwrap()).unwrap();
            assert_eq!(config.codec_string, codec);
            assert_eq!((config.width, config.height), (w, h));
        }
    }
}
