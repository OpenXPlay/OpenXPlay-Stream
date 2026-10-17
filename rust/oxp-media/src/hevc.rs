// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use crate::{
    bitstream::{dimensions, Bits, Cursor},
    Result,
};
#[derive(Clone, Debug)]
pub struct HevcConfig {
    pub profile_space: u8,
    pub profile: u8,
    pub compatibility: u32,
    pub high_tier: bool,
    pub level: u8,
    pub constraints: Vec<u8>,
    pub nal_length_size: u8,
    pub vps: Vec<Vec<u8>>,
    pub sps: Vec<Vec<u8>>,
    pub pps: Vec<Vec<u8>>,
    pub width: u32,
    pub height: u32,
    pub codec_string: String,
}
impl HevcConfig {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut c = Cursor::new(data);
        if c.u8()? != 1 {
            return Err("Unsupported hvcC version");
        }
        let p = c.u8()?;
        let compatibility = c.u32()?;
        let constraints = c.bytes(6)?.to_vec();
        let level = c.u8()?;
        if c.u16()? & 0xf000 != 0xf000
            || c.u8()? & 0xfc != 0xfc
            || c.u8()? & 0xfc != 0xfc
            || c.u8()? & 0xf8 != 0xf8
            || c.u8()? & 0xf8 != 0xf8
        {
            return Err("Invalid hvcC reserved bits");
        }
        c.u16()?;
        let temporal = c.u8()?;
        if temporal & 3 == 2 {
            return Err("Reserved hvcC NAL length size");
        }
        let mut vps = Vec::new();
        let mut sps = Vec::new();
        let mut pps = Vec::new();
        for _ in 0..c.u8()? {
            let h = c.u8()?;
            if h & 0x40 != 0 {
                return Err("Invalid hvcC array reserved bit");
            }
            let t = h & 63;
            for _ in 0..c.u16()? {
                let n = c.nal()?;
                if n.len() < 2 || n[0] & 128 != 0 || (n[0] >> 1) & 63 != t || n[1] & 7 == 0 {
                    return Err("Invalid HEVC array NAL header");
                }
                match t {
                    32 => vps.push(n),
                    33 => sps.push(n),
                    34 => pps.push(n),
                    _ => {}
                }
            }
        }
        c.end()?;
        if vps.is_empty() || sps.is_empty() || pps.is_empty() {
            return Err("hvcC requires VPS, SPS and PPS");
        }
        let (width, height) = Self::parse_sps(&sps[0])?;
        let profile_space = p >> 6;
        let profile = p & 31;
        let high_tier = p & 32 != 0;
        let mut codec_string = format!(
            "hvc1.{}{profile}.{:X}.{}{level}",
            ["", "A", "B", "C"][profile_space as usize],
            compatibility.reverse_bits(),
            if high_tier { "H" } else { "L" }
        );
        if let Some(last) = constraints.iter().rposition(|&b| b != 0) {
            for v in &constraints[..=last] {
                codec_string.push_str(&format!(".{v:02X}"));
            }
        }
        Ok(Self {
            profile_space,
            profile,
            compatibility,
            high_tier,
            level,
            constraints,
            nal_length_size: (temporal & 3) + 1,
            vps,
            sps,
            pps,
            width,
            height,
            codec_string,
        })
    }
    pub fn parse_sps(sps: &[u8]) -> Result<(u32, u32)> {
        if sps.len() < 2 || sps[0] & 128 != 0 || (sps[0] >> 1) & 63 != 33 || sps[1] & 7 == 0 {
            return Err("Expected HEVC SPS");
        }
        let mut b = Bits::new(sps, 2)?;
        b.skip(4)?;
        let layers = b.bits(3)? as usize;
        if layers > 6 {
            return Err("Invalid sps_max_sub_layers_minus1");
        }
        b.skip(97)?;
        let mut flags = Vec::new();
        for _ in 0..layers {
            flags.push((b.flag()?, b.flag()?));
        }
        if layers > 0 {
            for _ in layers..8 {
                if b.bits(2)? != 0 {
                    return Err("Invalid profile-tier reserved bits");
                }
            }
        }
        for (p, l) in flags {
            if p {
                b.skip(88)?;
            }
            if l {
                b.skip(8)?;
            }
        }
        if b.ue()? > 15 {
            return Err("Invalid sps_seq_parameter_set_id");
        }
        let chroma = b.ue()?;
        if chroma > 3 {
            return Err("Invalid chroma_format_idc");
        }
        let separate = chroma == 3 && b.flag()?;
        let w = b.ue()? as i64;
        let h = b.ue()? as i64;
        let mut crop = [0i64; 4];
        if b.flag()? {
            for v in &mut crop {
                *v = b.ue()? as i64;
            }
        }
        let chroma = if separate { 0 } else { chroma };
        let x = if chroma == 1 || chroma == 2 { 2 } else { 1 };
        let y = if chroma == 1 { 2 } else { 1 };
        dimensions(w - (crop[0] + crop[1]) * x, h - (crop[2] + crop[3]) * y)
    }
}
