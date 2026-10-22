// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use crate::{nal, AvcConfig, HevcConfig, Result, VideoCodec};
const MAX_PCR_FILL_US: i64 = 2_000_000;
#[derive(Clone, Copy, Debug)]
pub enum VideoConfig<'a> {
    Avc(&'a AvcConfig),
    Hevc(&'a HevcConfig),
}
impl<'a> From<&'a AvcConfig> for VideoConfig<'a> {
    fn from(config: &'a AvcConfig) -> Self {
        Self::Avc(config)
    }
}
impl<'a> From<&'a HevcConfig> for VideoConfig<'a> {
    fn from(config: &'a HevcConfig) -> Self {
        Self::Hevc(config)
    }
}
#[derive(Clone, Debug)]
pub struct TsOptions {
    pub pmt_pid: u16,
    pub video_pid: u16,
    pub audio_pid: u16,
    pub program_number: u16,
    pub audio_enabled: bool,
    pub insert_aud: bool,
    pub psi_interval_frames: u32,
    pub video_codec: VideoCodec,
    pub pmt_version: u8,
}
impl Default for TsOptions {
    fn default() -> Self {
        Self {
            pmt_pid: 0x100,
            video_pid: 0x101,
            audio_pid: 0x102,
            program_number: 1,
            audio_enabled: false,
            insert_aud: true,
            psi_interval_frames: 15,
            video_codec: VideoCodec::H264,
            pmt_version: 0,
        }
    }
}
/// Serialized muxer. The callback borrows the same packet buffer on every call.
pub struct TsMuxer<F: FnMut(&[u8; 188])> {
    sink: F,
    options: TsOptions,
    packet: [u8; 188],
    counters: [u8; 8192],
    video_payload_written: bool,
    video_frames: u32,
    tables_written: bool,
    last_pcr_us: Option<i64>,
    pat: Vec<u8>,
    pmt: Vec<u8>,
}
impl<F: FnMut(&[u8; 188])> TsMuxer<F> {
    pub fn new(sink: F) -> Self {
        Self::with_options(sink, TsOptions::default()).unwrap()
    }
    pub fn with_options(sink: F, o: TsOptions) -> Result<Self> {
        let pids = [o.pmt_pid, o.video_pid, o.audio_pid];
        if pids.iter().any(|p| !(0x20..=0x1ffe).contains(p))
            || pids[0] == pids[1]
            || pids[0] == pids[2]
            || pids[1] == pids[2]
            || o.program_number == 0
            || o.psi_interval_frames == 0
            || o.pmt_version > 31
        {
            return Err("Invalid muxer options");
        }
        let pat = section(vec![
            0,
            0xb0,
            13,
            0,
            1,
            0xc1,
            0,
            0,
            (o.program_number >> 8) as u8,
            o.program_number as u8,
            0xe0 | (o.pmt_pid >> 8) as u8,
            o.pmt_pid as u8,
        ]);
        let mut pmt = vec![
            2,
            0xb0,
            if o.audio_enabled { 23 } else { 18 },
            (o.program_number >> 8) as u8,
            o.program_number as u8,
            0xc1 | (o.pmt_version << 1),
            0,
            0,
            0xe0 | (o.video_pid >> 8) as u8,
            o.video_pid as u8,
            0xf0,
            0,
            match o.video_codec {
                VideoCodec::H264 => 0x1b,
                VideoCodec::H265 => 0x24,
            },
            0xe0 | (o.video_pid >> 8) as u8,
            o.video_pid as u8,
            0xf0,
            0,
        ];
        if o.audio_enabled {
            pmt.extend_from_slice(&[
                0x0f,
                0xe0 | (o.audio_pid >> 8) as u8,
                o.audio_pid as u8,
                0xf0,
                0,
            ]);
        }
        Ok(Self {
            sink,
            options: o,
            packet: [0; 188],
            counters: [0; 8192],
            video_payload_written: false,
            video_frames: 0,
            tables_written: false,
            last_pcr_us: None,
            pat,
            pmt: section(pmt),
        })
    }
    pub fn write_video<'a>(
        &mut self,
        frame: &[u8],
        config: impl Into<VideoConfig<'a>>,
        pts_us: i64,
        key: bool,
    ) -> Result<()> {
        if pts_us < 0 || frame.is_empty() {
            return Err("Invalid video timestamp or empty access unit");
        }
        let mut payload = Vec::new();
        let key = match config.into() {
            VideoConfig::Avc(config) => {
                if self.options.video_codec != VideoCodec::H264 {
                    return Err("Muxer codec mismatch");
                }
                let annex = nal::avcc_to_annex_b(frame)?;
                if self.options.insert_aud {
                    payload.extend_from_slice(&[0, 0, 0, 1, 9, 0xf0]);
                }
                if key {
                    for n in config.sps.iter().chain(&config.pps) {
                        payload.extend_from_slice(&[0, 0, 0, 1]);
                        payload.extend_from_slice(n);
                    }
                }
                payload.extend_from_slice(&annex);
                key
            }
            VideoConfig::Hevc(config) => {
                if self.options.video_codec != VideoCodec::H265 {
                    return Err("Muxer codec mismatch");
                }
                let units = nal::units(frame, nal::Format::Avcc)?;
                if units.iter().any(|n| n.len() < 2) {
                    return Err("Truncated HEVC NAL header");
                }
                let kind = |n: &[u8]| (n[0] >> 1) & 63;
                let irap = units.iter().any(|n| (16..=21).contains(&kind(n)));
                let mut append = |n: &[u8]| {
                    payload.extend_from_slice(&[0, 0, 0, 1]);
                    payload.extend_from_slice(n);
                };
                if irap {
                    for n in config.vps.iter().chain(&config.sps).chain(&config.pps) {
                        append(n);
                    }
                }
                // Replace in-band parameter sets and omit prefix SEI on IRAPs so the slice follows
                // VPS/SPS/PPS directly. Prefix SEI must not be moved behind the slice.
                for n in units {
                    let t = kind(n);
                    if t != 35 && (!irap || (!(32..=34).contains(&t) && t != 39)) {
                        append(n);
                    }
                }
                irap || key
            }
        };
        if !self.tables_written || key || self.video_frames == 0 {
            self.tables();
        }
        self.video_frames = (self.video_frames + 1) % self.options.psi_interval_frames;
        self.advance_pcr(pts_us, false);
        let pcr =
            if self.last_pcr_us.is_none() || key || pts_us - self.last_pcr_us.unwrap() >= 40_000 {
                Some(pts_us)
            } else {
                None
            };
        self.pes(self.options.video_pid, 0xe0, &payload, pts_us, key, pcr)?;
        if pcr.is_some() {
            self.last_pcr_us = pcr;
        }
        Ok(())
    }
    pub fn write_audio(&mut self, adts: &[u8], pts_us: i64) -> Result<()> {
        if !self.options.audio_enabled || pts_us < 0 {
            return Err("Disabled AAC or negative timestamp");
        }
        crate::Adts::validate_frame(adts)?;
        if !self.tables_written {
            self.tables();
        }
        self.advance_pcr(pts_us, true);
        if self.last_pcr_us.is_none() {
            self.pcr_only(pts_us);
            self.last_pcr_us = Some(pts_us);
        }
        self.pes(self.options.audio_pid, 0xc0, adts, pts_us, false, None)
    }
    fn tables(&mut self) {
        let mut pat = vec![0];
        pat.extend_from_slice(&self.pat);
        let mut pmt = vec![0];
        pmt.extend_from_slice(&self.pmt);
        self.payload(0, &pat, false, None);
        self.payload(self.options.pmt_pid, &pmt, false, None);
        self.tables_written = true;
    }
    fn advance_pcr(&mut self, pts: i64, endpoint: bool) {
        if let Some(mut previous) = self.last_pcr_us {
            // A jump beyond MAX_PCR_FILL_US is a timestamp discontinuity, not a gap to fill: filling a
            // jump of hours generates millions of PCR packets and exhausts memory.
            if pts - previous > MAX_PCR_FILL_US {
                self.last_pcr_us = Some(pts - 40_000);
                return;
            }
            while pts > previous
                && (pts - previous > 40_000 || (endpoint && pts - previous == 40_000))
            {
                previous += 40_000;
                self.pcr_only(previous);
                self.last_pcr_us = Some(previous);
            }
        }
    }
    fn pes(
        &mut self,
        pid: u16,
        id: u8,
        bytes: &[u8],
        pts: i64,
        random: bool,
        pcr: Option<i64>,
    ) -> Result<()> {
        let size = if id == 0xe0 { 0 } else { bytes.len() + 8 };
        if size > 65535 {
            return Err("Audio PES is too large");
        }
        let t = to_90khz(pts)?;
        let mut pes = vec![
            0,
            0,
            1,
            id,
            (size >> 8) as u8,
            size as u8,
            0x80,
            0x80,
            5,
            0x21 | ((((t >> 30) & 7) as u8) << 1),
            (t >> 22) as u8,
            ((((t >> 15) & 127) as u8) << 1) | 1,
            (t >> 7) as u8,
            (((t & 127) as u8) << 1) | 1,
        ];
        pes.extend_from_slice(bytes);
        self.payload(pid, &pes, random, pcr);
        Ok(())
    }
    fn payload(&mut self, pid: u16, bytes: &[u8], random: bool, pcr: Option<i64>) {
        let mut offset = 0;
        while offset < bytes.len() {
            let first = offset == 0;
            let pcr = if first { pcr } else { None };
            let random = first && random;
            let minimum = if pcr.is_some() {
                8
            } else if random {
                2
            } else {
                0
            };
            let count = (bytes.len() - offset).min(184 - minimum);
            let adaptation = 184 - count;
            self.header(
                pid,
                first,
                if adaptation == 0 { 1 } else { 3 },
                self.counters[pid as usize],
            );
            self.counters[pid as usize] = (self.counters[pid as usize] + 1) & 15;
            if pid == self.options.video_pid {
                self.video_payload_written = true;
            }
            if adaptation > 0 {
                self.packet[4] = (adaptation - 1) as u8;
                if adaptation > 1 {
                    self.packet[5] =
                        (if random { 0x40 } else { 0 }) | (if pcr.is_some() { 0x10 } else { 0 });
                    if let Some(t) = pcr {
                        self.encode_pcr(t);
                    }
                }
            }
            self.packet[4 + adaptation..4 + adaptation + count]
                .copy_from_slice(&bytes[offset..offset + count]);
            (self.sink)(&self.packet);
            offset += count;
        }
    }
    fn pcr_only(&mut self, t: i64) {
        let pid = self.options.video_pid;
        self.header(
            pid,
            false,
            2,
            if self.video_payload_written {
                (self.counters[pid as usize] + 15) & 15
            } else {
                0
            },
        );
        self.packet[4] = 183;
        self.packet[5] = 0x10;
        self.encode_pcr(t);
        (self.sink)(&self.packet);
    }
    fn header(&mut self, pid: u16, start: bool, control: u8, counter: u8) {
        self.packet.fill(255);
        self.packet[0] = 0x47;
        self.packet[1] = (pid >> 8) as u8 | if start { 0x40 } else { 0 };
        self.packet[2] = pid as u8;
        self.packet[3] = (control << 4) | counter;
    }
    fn encode_pcr(&mut self, t: i64) {
        let b = to_90khz(t).unwrap();
        let ext = ((t % 100) * 27 % 300) as u16;
        self.packet[6] = (b >> 25) as u8;
        self.packet[7] = (b >> 17) as u8;
        self.packet[8] = (b >> 9) as u8;
        self.packet[9] = (b >> 1) as u8;
        self.packet[10] = (((b & 1) as u8) << 7) | 0x7e | (ext >> 8) as u8;
        self.packet[11] = ext as u8;
    }
}
pub fn to_90khz(t: i64) -> Result<u64> {
    if t < 0 {
        return Err("Negative timestamp");
    }
    let t = t as u64;
    Ok((((t / 100) & 0x1ffffffff) * 9 + (t % 100) * 9 / 100) & 0x1ffffffff)
}
pub fn crc32_mpeg2(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &b in bytes {
        crc ^= (b as u32) << 24;
        for _ in 0..8 {
            crc = (crc << 1) ^ if crc & 0x80000000 != 0 { 0x04c11db7 } else { 0 };
        }
    }
    crc
}
fn section(mut body: Vec<u8>) -> Vec<u8> {
    let c = crc32_mpeg2(&body);
    body.extend_from_slice(&c.to_be_bytes());
    body
}
