// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use crate::{
    bitstream::{dimensions, Bits, Cursor},
    Result,
};
#[derive(Clone, Debug)]
pub struct AvcConfig {
    pub profile: u8,
    pub compatibility: u8,
    pub level: u8,
    pub nal_length_size: u8,
    pub sps: Vec<Vec<u8>>,
    pub pps: Vec<Vec<u8>>,
    pub sps_extensions: Vec<Vec<u8>>,
    pub width: u32,
    pub height: u32,
    pub codec_string: String,
}
const HIGH: &[u8] = &[
    44, 83, 86, 100, 110, 118, 122, 128, 134, 135, 138, 139, 144, 244,
];
impl AvcConfig {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut c = Cursor::new(data);
        if c.u8()? != 1 {
            return Err("Unsupported avcC version");
        }
        let profile = c.u8()?;
        let compatibility = c.u8()?;
        let level = c.u8()?;
        // Reserved bits are ignored like FFmpeg and Chromium do; some encoders (iOS) leave them zero.
        let lengths = c.u8()?;
        if lengths & 3 == 2 {
            return Err("Invalid avcC NAL length size");
        }
        let count = c.u8()?;
        fn sets(c: &mut Cursor<'_>, count: u8, kind: u8) -> Result<Vec<Vec<u8>>> {
            let mut out = Vec::new();
            for _ in 0..count {
                let n = c.nal()?;
                if n.is_empty() || n[0] & 0x9f != kind {
                    return Err("Invalid AVC parameter-set NAL type");
                }
                out.push(n);
            }
            Ok(out)
        }
        let sps = sets(&mut c, count & 31, 7)?;
        let n = c.u8()?;
        let pps = sets(&mut c, n, 8)?;
        if sps.is_empty() || pps.is_empty() {
            return Err("avcC requires SPS and PPS");
        }
        let mut sps_extensions = Vec::new();
        // The High-profile extension is optional. Like FFmpeg and Chromium, ignore its reserved
        // bits and any incomplete or trailing bytes.
        if c.data.len() - c.pos >= 4 && HIGH.contains(&profile) {
            c.pos += 3;
            let n = c.u8()?;
            sps_extensions = sets(&mut c, n, 13)?;
        }
        c.pos = c.data.len();
        c.end()?;
        let (width, height) = Self::parse_sps(&sps[0])?;
        Ok(Self {
            profile,
            compatibility,
            level,
            nal_length_size: (lengths & 3) + 1,
            sps,
            pps,
            sps_extensions,
            width,
            height,
            codec_string: format!("avc1.{profile:02X}{compatibility:02X}{level:02X}"),
        })
    }
    pub fn parse_sps(sps: &[u8]) -> Result<(u32, u32)> {
        if sps.is_empty() || sps[0] & 0x9f != 7 {
            return Err("Expected AVC SPS");
        }
        let mut b = Bits::new(sps, 1)?;
        let profile = b.bits(8)? as u8;
        b.skip(16)?;
        if b.ue()? > 31 {
            return Err("Invalid seq_parameter_set_id");
        }
        let mut chroma = 1;
        let mut separate = false;
        if HIGH.contains(&profile) {
            chroma = b.ue()?;
            if chroma > 3 {
                return Err("Invalid chroma_format_idc");
            }
            if chroma == 3 {
                separate = b.flag()?;
            }
            if b.ue()? > 6 || b.ue()? > 6 {
                return Err("Unsupported AVC bit depth");
            }
            b.skip(1)?;
            if b.flag()? {
                for i in 0..if chroma == 3 { 12 } else { 8 } {
                    if b.flag()? {
                        let mut last = 8;
                        let mut next = 8;
                        for _ in 0..if i < 6 { 16 } else { 64 } {
                            if next != 0 {
                                next = (last + b.se()?).rem_euclid(256);
                            }
                            if next != 0 {
                                last = next;
                            }
                        }
                    }
                }
            }
        }
        if b.ue()? > 12 {
            return Err("Invalid log2_max_frame_num_minus4");
        }
        match b.ue()? {
            0 => {
                if b.ue()? > 12 {
                    return Err("Invalid log2_max_pic_order_cnt_lsb_minus4");
                }
            }
            1 => {
                b.skip(1)?;
                b.se()?;
                b.se()?;
                let n = b.ue()?;
                if n > 255 {
                    return Err("Invalid picture-order cycle");
                }
                for _ in 0..n {
                    b.se()?;
                }
            }
            2 => {}
            _ => return Err("Invalid pic_order_cnt_type"),
        }
        b.ue()?;
        b.skip(1)?;
        let wm = b.ue()? as i64 + 1;
        let hm = b.ue()? as i64 + 1;
        let frame = b.flag()?;
        if !frame {
            b.skip(1)?;
        }
        b.skip(1)?;
        let mut crop = [0i64; 4];
        if b.flag()? {
            for v in &mut crop {
                *v = b.ue()? as i64;
            }
        }
        let chroma = if separate { 0 } else { chroma };
        let x = if chroma == 1 || chroma == 2 { 2 } else { 1 };
        let factor = if frame { 1 } else { 2 };
        let y = if chroma == 1 { 2 * factor } else { factor };
        dimensions(
            wm * 16 - (crop[0] + crop[1]) * x,
            hm * 16 * factor - (crop[2] + crop[3]) * y,
        )
    }
    pub fn from_parameter_sets(sps: &[u8], pps: &[u8]) -> Result<Vec<u8>> {
        if sps.len() < 4 || sps.len() > 65535 || pps.len() > 65535 {
            return Err("Invalid parameter sets");
        }
        let mut out = vec![1, sps[1], sps[2], sps[3], 255, 225];
        out.extend_from_slice(&(sps.len() as u16).to_be_bytes());
        out.extend_from_slice(sps);
        out.push(1);
        out.extend_from_slice(&(pps.len() as u16).to_be_bytes());
        out.extend_from_slice(pps);
        Ok(out)
    }
}
