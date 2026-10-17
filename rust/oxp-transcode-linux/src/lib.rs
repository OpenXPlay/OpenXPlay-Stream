// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

//! Hardware video transcoding for receivers that cannot decode the input profile. The trait and
//! the output adapter are platform-neutral; the V4L2 memory-to-memory implementation is Linux-only.

#![cfg_attr(not(target_os = "linux"), forbid(unsafe_code))]
#![deny(unsafe_op_in_unsafe_fn)]
pub mod annexb;
pub mod fake;
pub mod negotiate;
pub mod nv12;
pub mod output;
pub mod pacing;
#[cfg(target_os = "linux")]
pub mod v4l2;
pub use output::TranscodingOutput;
use oxp_core::model::VideoFormat;
pub use oxp_media::transcode_policy::{TranscodeSupport, TranscodeTarget};
use std::{sync::Arc, time::Duration};
pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;

/// What a transcoder session converts: the input stream and the policy's target.
#[derive(Clone, Debug)]
pub struct TranscodeConfig {
    pub source: Arc<VideoFormat>,
    pub target: TranscodeTarget,
}

/// One encoded access unit. Callers reuse it so `data` keeps its allocation between frames.
#[derive(Clone, Debug, Default)]
pub struct EncodedFrame {
    /// Four-byte length-prefixed NAL units without parameter sets or access unit delimiters.
    pub data: Vec<u8>,
    pub pts_us: i64,
    pub key_frame: bool,
    /// avcC record describing `data`; rebuilt only when the encoder changes its parameter sets.
    pub config: Option<Arc<[u8]>>,
}

/// A running decoder-to-encoder session. Calls come from one worker thread and may block for
/// at most the timeout they are given.
pub trait Transcoder: Send {
    /// Queues one access unit of four-byte length-prefixed NAL units. Returns false without
    /// consuming it when no input buffer is free; poll and retry. Until the first key frame,
    /// and after `reset`, other frames are consumed and dropped.
    fn submit(&mut self, data: &[u8], pts_us: i64, key_frame: bool) -> Result<bool>;
    /// Moves the next encoded access unit into `out`, waiting up to `timeout`. Returns false
    /// when none is ready.
    fn poll(&mut self, out: &mut EncodedFrame, timeout: Duration) -> Result<bool>;
    /// Makes the next encoded frame an IDR.
    fn force_key_frame(&mut self) -> Result<()>;
    /// Discards queued input after a discontinuity; the next submitted frame must be a key frame.
    fn reset(&mut self) -> Result<()>;
}

pub trait TranscoderFactory: Send + Sync {
    /// None when this host has no usable transcoder.
    fn support(&self) -> Option<TranscodeSupport>;
    fn open(&self, config: &TranscodeConfig) -> Result<Box<dyn Transcoder>>;
    /// One line for the relay log naming the devices in use.
    fn describe(&self) -> String;
}

/// A factory for hosts without hardware transcoding.
pub struct Unavailable(pub String);
impl TranscoderFactory for Unavailable {
    fn support(&self) -> Option<TranscodeSupport> {
        None
    }
    fn open(&self, _: &TranscodeConfig) -> Result<Box<dyn Transcoder>> {
        Err(self.0.clone().into())
    }
    fn describe(&self) -> String {
        format!("video transcoder unavailable: {}", self.0)
    }
}

/// The platform's hardware transcoder: V4L2 memory-to-memory codecs found by capability on Linux.
pub fn default_factory() -> Arc<dyn TranscoderFactory> {
    #[cfg(target_os = "linux")]
    {
        Arc::new(v4l2::V4l2Factory::default())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Arc::new(Unavailable(
            "hardware transcoding requires Linux V4L2".into(),
        ))
    }
}
