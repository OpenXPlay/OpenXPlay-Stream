// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

//! V4L2 memory-to-memory transcoding: devices are found by capability, not by path.

mod device;
mod sys;
mod transcoder;
pub use device::discover;
pub use transcoder::V4l2Transcoder;

use crate::{
    negotiate::{codec_fourcc, fourcc_name, Inventory},
    Result, TranscodeConfig, TranscodeSupport, Transcoder, TranscoderFactory,
};
use std::sync::OnceLock;

/// Scans `/dev/video*` once, on first use.
#[derive(Default)]
pub struct V4l2Factory {
    inventory: OnceLock<Inventory>,
}
impl V4l2Factory {
    pub fn inventory(&self) -> &Inventory {
        self.inventory.get_or_init(discover)
    }
}
impl TranscoderFactory for V4l2Factory {
    fn support(&self) -> Option<TranscodeSupport> {
        self.inventory().support()
    }
    fn open(&self, config: &TranscodeConfig) -> Result<Box<dyn Transcoder>> {
        let pair = self
            .inventory()
            .select(config.source.codec)
            .ok_or_else(|| {
                format!(
                    "no V4L2 M2M {} decoder paired with an H.264 encoder",
                    fourcc_name(codec_fourcc(config.source.codec))
                )
            })?;
        Ok(Box::new(V4l2Transcoder::open(&pair, config)?))
    }
    fn describe(&self) -> String {
        self.inventory().describe()
    }
}
