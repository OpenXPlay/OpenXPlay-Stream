// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use crate::{Result, VideoCodec};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    AnnexB,
    Avcc,
}
/// Lazy borrowed NAL iteration; errors cannot be concealed by an earlier IDR.
pub struct NalIterator<'a> {
    data: &'a [u8],
    format: Format,
    position: usize,
    payload_start: Option<usize>,
    zeros: usize,
    finished: bool,
}
pub fn iter(data: &[u8], format: Format) -> NalIterator<'_> {
    NalIterator {
        data,
        format,
        position: 0,
        payload_start: None,
        zeros: 0,
        finished: false,
    }
}
impl<'a> Iterator for NalIterator<'a> {
    type Item = Result<&'a [u8]>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        let result = self.next_unit();
        if result.as_ref().is_none_or(|v| v.is_err()) {
            self.finished = true;
        }
        result
    }
}
impl<'a> NalIterator<'a> {
    fn next_unit(&mut self) -> Option<Result<&'a [u8]>> {
        if self.format == Format::Avcc {
            if self.position == self.data.len() {
                return None;
            }
            if self.data.len() - self.position < 4 {
                return Some(Err("Truncated NAL length"));
            }
            let n = u32::from_be_bytes(
                self.data[self.position..self.position + 4]
                    .try_into()
                    .unwrap(),
            ) as usize;
            self.position += 4;
            if n == 0 || n > self.data.len() - self.position {
                return Some(Err("Invalid NAL length"));
            }
            let s = self.position;
            self.position += n;
            return Some(Ok(&self.data[s..self.position]));
        }
        if self.data.is_empty() {
            return None;
        }
        while self.position < self.data.len() {
            let i = self.position;
            let b = self.data[i];
            self.position += 1;
            if b == 1 && self.zeros >= 2 {
                let prefix = i - self.zeros;
                let previous = self.payload_start;
                self.payload_start = Some(i + 1);
                self.zeros = 0;
                if let Some(s) = previous {
                    if prefix <= s {
                        return Some(Err("Empty NAL unit"));
                    }
                    return Some(Ok(&self.data[s..prefix]));
                } else if prefix != 0 {
                    return Some(Err("Unexpected bytes before start code"));
                }
            } else {
                if self.payload_start.is_none() && b != 0 {
                    return Some(Err("Missing Annex B start code"));
                }
                self.zeros = if b == 0 { self.zeros + 1 } else { 0 };
            }
        }
        self.finished = true;
        let s = match self.payload_start {
            Some(s) => s,
            None => return Some(Err("Missing Annex B start code")),
        };
        let end = self.data.len() - self.zeros;
        Some(if end <= s {
            Err("Empty NAL unit")
        } else {
            Ok(&self.data[s..end])
        })
    }
}
pub fn units(data: &[u8], format: Format) -> Result<Vec<&[u8]>> {
    iter(data, format).collect()
}
pub fn annex_b_to_avcc(data: &[u8]) -> Result<Vec<u8>> {
    convert(data, Format::AnnexB)
}
pub fn avcc_to_annex_b(data: &[u8]) -> Result<Vec<u8>> {
    convert(data, Format::Avcc)
}
fn convert(data: &[u8], source: Format) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for n in units(data, source)? {
        out.extend_from_slice(
            &(if source == Format::AnnexB {
                u32::try_from(n.len()).map_err(|_| "NAL output is too large")?
            } else {
                1
            })
            .to_be_bytes(),
        );
        out.extend_from_slice(n);
    }
    Ok(out)
}
pub fn is_key_frame(data: &[u8], codec: VideoCodec, format: Format) -> Result<bool> {
    let mut key = false;
    for n in units(data, format)? {
        match codec {
            VideoCodec::H264 => key |= n[0] & 31 == 5,
            VideoCodec::H265 => {
                if n.len() < 2 {
                    return Err("Truncated HEVC NAL header");
                }
                key |= (16..=23).contains(&((n[0] >> 1) & 63));
            }
        }
    }
    Ok(key)
}
