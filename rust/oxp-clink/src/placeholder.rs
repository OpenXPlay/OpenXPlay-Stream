// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use crate::{model::*, require, Result};
use oxp_media::{
    nal::{self, Format},
    AvcConfig,
};
use std::sync::Arc;
pub trait PlaceholderVideo: Send + Sync {
    fn format(&self) -> &VideoFormat;
    fn frames(&self) -> &[Arc<[u8]>];
    fn fps(&self) -> u32;
}
pub struct BundledPlaceholderVideo {
    format: VideoFormat,
    frames: Vec<Arc<[u8]>>,
}
impl BundledPlaceholderVideo {
    pub fn new() -> Result<Self> {
        let clip = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../kotlin/clink/src/main/resources/placeholder-1280x720.h264"
        ));
        let units = nal::units(clip, Format::AnnexB)?;
        let sps = units.iter().find(|n| n[0] & 31 == 7).ok_or("missing_sps")?;
        let pps = units.iter().find(|n| n[0] & 31 == 8).ok_or("missing_pps")?;
        let config = AvcConfig::from_parameter_sets(sps, pps)?;
        let format = VideoFormat::create(0, VideoCodec::H264, config.into(), Some(30.0))?;
        let mut frames = Vec::new();
        let mut current = Vec::new();
        for n in units {
            let t = n[0] & 31;
            if t == 9 && !current.is_empty() {
                frames.push(Arc::from(std::mem::take(&mut current)));
            }
            if [1, 5, 6].contains(&t) {
                current.extend_from_slice(&(n.len() as u32).to_be_bytes());
                current.extend_from_slice(n);
            }
        }
        if !current.is_empty() {
            frames.push(Arc::from(current));
        }
        require(
            frames.len() == 30 && nal::is_key_frame(&frames[0], VideoCodec::H264, Format::Avcc)?,
            "invalid_placeholder",
        )?;
        Ok(Self { format, frames })
    }
}
impl PlaceholderVideo for BundledPlaceholderVideo {
    fn format(&self) -> &VideoFormat {
        &self.format
    }
    fn frames(&self) -> &[Arc<[u8]>] {
        &self.frames
    }
    fn fps(&self) -> u32 {
        30
    }
}

pub struct BundledHevcPlaceholderVideo {
    format: VideoFormat,
    frames: Vec<Arc<[u8]>>,
}
impl BundledHevcPlaceholderVideo {
    pub fn new() -> Result<Self> {
        let clip = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../kotlin/clink/src/main/resources/placeholder-1280x720.h265"
        ));
        let units = nal::units(clip, Format::AnnexB)?;
        // Main level 3.1, compatibility 0x60000000, four-byte NAL lengths.
        let mut config = vec![
            1, 1, 0x60, 0, 0, 0, 0x90, 0, 0, 0, 0, 0, 93, 0xf0, 0, 0xfc, 0xfd, 0xf8, 0xf8, 0, 0,
            0x0f, 3,
        ];
        for kind in 32..=34 {
            let n = units
                .iter()
                .find(|n| (n[0] >> 1) & 63 == kind)
                .ok_or("missing_hevc_parameter_set")?;
            config.push(0x80 | kind);
            config.extend_from_slice(&1u16.to_be_bytes());
            config.extend_from_slice(&(n.len() as u16).to_be_bytes());
            config.extend_from_slice(n);
        }
        let format = VideoFormat::create(0, VideoCodec::H265, config.into(), Some(30.0))?;
        let mut frames = Vec::new();
        let mut current = Vec::new();
        for n in units {
            if (n[0] >> 1) & 63 <= 31 {
                require(n.len() >= 3, "invalid_hevc_placeholder_slice")?;
                // first_slice_segment_in_pic_flag starts a picture without an AUD.
                if n[2] & 0x80 != 0 && !current.is_empty() {
                    frames.push(Arc::from(std::mem::take(&mut current)));
                }
                current.extend_from_slice(&(n.len() as u32).to_be_bytes());
                current.extend_from_slice(n);
            }
        }
        if !current.is_empty() {
            frames.push(Arc::from(current));
        }
        require(
            frames.len() == 30 && nal::is_key_frame(&frames[0], VideoCodec::H265, Format::Avcc)?,
            "invalid_hevc_placeholder",
        )?;
        Ok(Self { format, frames })
    }
}
impl PlaceholderVideo for BundledHevcPlaceholderVideo {
    fn format(&self) -> &VideoFormat {
        &self.format
    }
    fn frames(&self) -> &[Arc<[u8]>] {
        &self.frames
    }
    fn fps(&self) -> u32 {
        30
    }
}
