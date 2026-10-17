// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

#![forbid(unsafe_code)]
pub mod audio;
pub use audio::{Adts, AudioSpecificConfig};
pub mod avc;
mod bitstream;
pub mod hevc;
pub mod nal;
pub mod transcode_policy;
pub mod ts;
pub use avc::AvcConfig;
pub use hevc::HevcConfig;
pub use ts::TsMuxer;
pub type Result<T> = std::result::Result<T, &'static str>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    H265,
}
