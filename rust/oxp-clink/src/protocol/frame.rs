// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use crate::{require, Result};
pub const HEADER_BYTES: usize = 20;
pub const MAX_BYTES: usize = 4 * 1024 * 1024;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub sequence: u32,
    pub unix_seconds: u32,
    pub source: u8,
    pub format: u8,
    pub kind: u8,
    pub category: u8,
    pub method: u16,
    pub body: Vec<u8>,
}
impl Frame {
    pub fn encode(&self) -> Result<Vec<u8>> {
        require(
            self.source <= 1
                && self.format <= 3
                && self.kind <= 3
                && self.body.len() <= MAX_BYTES - 20,
            "invalid_packet_header",
        )?;
        let mut out = Vec::new();
        out.extend_from_slice(&((20 + self.body.len()) as u32).to_be_bytes());
        out.extend_from_slice(&self.sequence.to_be_bytes());
        out.extend_from_slice(&self.unix_seconds.to_be_bytes());
        out.extend_from_slice(&[
            self.source | (self.format << 1) | (self.kind << 3),
            self.category,
        ]);
        out.extend_from_slice(&self.method.to_be_bytes());
        out.extend_from_slice(&[0, 0]);
        let crc = crc16(&out);
        out.extend_from_slice(&crc.to_be_bytes());
        out.extend_from_slice(&self.body);
        Ok(out)
    }
    pub fn decode(b: &[u8]) -> Result<Self> {
        require((20..=MAX_BYTES).contains(&b.len()), "invalid_packet_length")?;
        require(
            u32::from_be_bytes(b[..4].try_into()?) as usize == b.len(),
            "invalid_packet_length",
        )?;
        let f = b[12];
        require(
            f & 0xe0 == 0 && b[16..18] == [0, 0],
            "invalid_packet_header",
        )?;
        require(
            u16::from_be_bytes(b[18..20].try_into()?) == crc16(&b[..18]),
            "invalid_packet_crc",
        )?;
        Ok(Self {
            sequence: u32::from_be_bytes(b[4..8].try_into()?),
            unix_seconds: u32::from_be_bytes(b[8..12].try_into()?),
            source: f & 1,
            format: (f >> 1) & 3,
            kind: (f >> 3) & 3,
            category: b[13],
            method: u16::from_be_bytes(b[14..16].try_into()?),
            body: b[20..].to_vec(),
        })
    }
}
pub fn crc16(b: &[u8]) -> u16 {
    let mut c = 0;
    for &v in b {
        c ^= v as u16;
        for _ in 0..8 {
            c = (c >> 1) ^ if c & 1 != 0 { 0xa001 } else { 0 };
        }
    }
    c
}
#[derive(Default)]
pub struct FrameReader {
    buffer: Vec<u8>,
}
impl FrameReader {
    pub fn pending_bytes(&self) -> usize {
        self.buffer.len()
    }
    pub fn reset(&mut self) {
        self.buffer.clear();
    }
    pub fn feed(&mut self, b: &[u8]) -> Result<Vec<Frame>> {
        require(
            b.len() <= MAX_BYTES && self.buffer.len() + b.len() <= MAX_BYTES + 65536,
            "invalid_packet_length",
        )?;
        self.buffer.extend_from_slice(b);
        let mut out = Vec::new();
        let mut pos = 0;
        while self.buffer.len() - pos >= 4 {
            let n = u32::from_be_bytes(self.buffer[pos..pos + 4].try_into()?) as usize;
            require((20..=MAX_BYTES).contains(&n), "invalid_packet_length")?;
            if self.buffer.len() - pos < n {
                break;
            }
            out.push(Frame::decode(&self.buffer[pos..pos + n])?);
            pos += n;
        }
        self.buffer.drain(..pos);
        Ok(out)
    }
}
