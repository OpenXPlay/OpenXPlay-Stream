// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

//! Bitstream conversion at the transcoder edges: the Stream model carries four-byte
//! length-prefixed NAL units, hardware codecs consume and produce Annex B.

use crate::Result;
use oxp_core::model::VideoCodec;
use oxp_media::{
    nal::{self, Format},
    AvcConfig, HevcConfig,
};
use std::sync::Arc;

const START: [u8; 4] = [0, 0, 0, 1];

/// Annex B parameter sets from an avcC or hvcC record, prefixed to every key frame so a
/// decoder can start or restart at any IDR.
pub fn parameter_sets(codec: VideoCodec, config: &[u8]) -> Result<Vec<u8>> {
    let sets = match codec {
        VideoCodec::H264 => {
            let c = AvcConfig::parse(config)?;
            c.sps.into_iter().chain(c.pps).collect::<Vec<_>>()
        }
        VideoCodec::H265 => {
            let c = HevcConfig::parse(config)?;
            c.vps.into_iter().chain(c.sps).chain(c.pps).collect()
        }
    };
    let mut out = Vec::new();
    for set in sets {
        out.extend_from_slice(&START);
        out.extend_from_slice(&set);
    }
    Ok(out)
}

/// Writes `prefix` (already Annex B) and the length-prefixed `access_unit` into `dst` as Annex B.
/// Each four-byte length becomes a four-byte start code, so no scratch buffer is needed.
pub fn write_access_unit(prefix: &[u8], access_unit: &[u8], dst: &mut [u8]) -> Result<usize> {
    let total = prefix.len() + access_unit.len();
    if dst.len() < total {
        return Err(format!(
            "access unit of {total} bytes exceeds the {} byte decoder buffer",
            dst.len()
        )
        .into());
    }
    dst[..prefix.len()].copy_from_slice(prefix);
    let body = &mut dst[prefix.len()..total];
    body.copy_from_slice(access_unit);
    let mut position = 0;
    while position < body.len() {
        let length = body
            .get(position..position + 4)
            .ok_or("Truncated NAL length")?;
        let n = u32::from_be_bytes(length.try_into().unwrap()) as usize;
        if n == 0 || n > body.len() - position - 4 {
            return Err("Invalid NAL length".into());
        }
        body[position..position + 4].copy_from_slice(&START);
        position += 4 + n;
    }
    Ok(total)
}

/// Tracks the encoder's SPS and PPS across access units and splits them from the picture data.
#[derive(Default)]
pub struct ParameterSets {
    sps: Vec<u8>,
    pps: Vec<u8>,
    config: Option<Arc<[u8]>>,
}
impl ParameterSets {
    /// Writes the non-parameter-set NAL units of an Annex B H.264 access unit into `out` as
    /// four-byte length-prefixed units and returns whether it holds an IDR slice. Access unit
    /// delimiters are dropped because the transport stream writer adds its own.
    pub fn split_access_unit(&mut self, annex_b: &[u8], out: &mut Vec<u8>) -> Result<bool> {
        out.clear();
        let mut key = false;
        let mut changed = false;
        for unit in nal::iter(annex_b, Format::AnnexB) {
            let unit = unit?;
            let slot = match unit[0] & 31 {
                7 => &mut self.sps,
                8 => &mut self.pps,
                9 => continue,
                kind => {
                    key |= kind == 5;
                    out.extend_from_slice(&(unit.len() as u32).to_be_bytes());
                    out.extend_from_slice(unit);
                    continue;
                }
            };
            if slot.as_slice() != unit {
                slot.clear();
                slot.extend_from_slice(unit);
                changed = true;
            }
        }
        if changed && !self.sps.is_empty() && !self.pps.is_empty() {
            self.config = Some(AvcConfig::from_parameter_sets(&self.sps, &self.pps)?.into());
        }
        Ok(key)
    }
    pub fn config(&self) -> Option<Arc<[u8]>> {
        self.config.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::avc_parameter_sets;

    fn avcc(units: &[&[u8]]) -> Vec<u8> {
        let mut out = Vec::new();
        for u in units {
            out.extend_from_slice(&(u.len() as u32).to_be_bytes());
            out.extend_from_slice(u);
        }
        out
    }

    #[test]
    fn length_prefixes_become_start_codes_in_place() {
        let mut dst = [0xAAu8; 32];
        let n = write_access_unit(
            &[0, 0, 0, 1, 0x67, 1],
            &avcc(&[&[0x65, 9, 9], &[0x06, 1]]),
            &mut dst,
        )
        .unwrap();
        assert_eq!(
            &dst[..n],
            &[0, 0, 0, 1, 0x67, 1, 0, 0, 0, 1, 0x65, 9, 9, 0, 0, 0, 1, 0x06, 1]
        );
        assert_eq!(dst[n], 0xAA);
        assert!(write_access_unit(&[], &avcc(&[&[0x65; 40]]), &mut dst).is_err());
        assert!(write_access_unit(&[], &[0, 0, 0, 9, 0x65], &mut dst).is_err());
        assert!(write_access_unit(&[], &[0, 0, 0, 0], &mut dst).is_err());
        assert!(write_access_unit(&[], &[0, 0, 1], &mut dst).is_err());
        assert_eq!(write_access_unit(&[], &[], &mut dst).unwrap(), 0);
    }

    #[test]
    fn source_parameter_sets_cover_avc_and_hevc() {
        let (sps, pps) = avc_parameter_sets(100, 0, 40, 1280, 720);
        let config = AvcConfig::from_parameter_sets(&sps, &pps).unwrap();
        let header = parameter_sets(VideoCodec::H264, &config).unwrap();
        let units = nal::units(&header, Format::AnnexB).unwrap();
        assert_eq!(units, vec![&sps[..], &pps[..]]);
        assert!(parameter_sets(VideoCodec::H265, &config).is_err());
    }

    #[test]
    fn encoder_output_splits_parameter_sets_and_tracks_changes() {
        let (sps, pps) = avc_parameter_sets(66, 0xE0, 31, 1280, 720);
        let mut stream = Vec::new();
        for unit in [&[0x09, 0xF0][..], &sps, &pps, &[0x65, 1, 2, 3]] {
            stream.extend_from_slice(&START);
            stream.extend_from_slice(unit);
        }
        stream.extend_from_slice(&[0, 0]);
        let mut sets = ParameterSets::default();
        let mut out = Vec::new();
        assert!(sets.split_access_unit(&stream, &mut out).unwrap());
        assert_eq!(out, avcc(&[&[0x65, 1, 2, 3]]));
        let config = sets.config().unwrap();
        let parsed = AvcConfig::parse(&config).unwrap();
        assert_eq!(parsed.codec_string, "avc1.42E01F");
        assert_eq!((parsed.width, parsed.height), (1280, 720));

        // A repeated identical header keeps the shared configuration.
        assert!(sets.split_access_unit(&stream, &mut out).unwrap());
        assert!(Arc::ptr_eq(&config, &sets.config().unwrap()));

        // Header-only buffers yield no picture data.
        let mut header = Vec::new();
        for unit in [&sps, &pps] {
            header.extend_from_slice(&START);
            header.extend_from_slice(unit);
        }
        assert!(!ParameterSets::default()
            .split_access_unit(&header, &mut out)
            .unwrap());
        assert!(out.is_empty());

        assert!(!sets
            .split_access_unit(&[0, 0, 0, 1, 0x41, 7], &mut out)
            .unwrap());
        assert_eq!(out, avcc(&[&[0x41, 7]]));
        assert!(sets
            .split_access_unit(&[7, 0, 0, 1, 0x41], &mut out)
            .is_err());
    }
}
