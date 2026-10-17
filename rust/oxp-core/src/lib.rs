// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

#![forbid(unsafe_code)]
pub mod audio;
pub mod context;
pub use context::*;
pub mod frame_queue;
pub mod hub;
pub mod interfaces;
pub mod keyframe_limiter;
pub mod model;
pub mod video_codec_policy;
pub use frame_queue::{FrameQueue, QueuedFrame};
pub use hub::Hub;
pub use interfaces::*;
pub use keyframe_limiter::KeyframeLimiter;
pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;
pub(crate) fn require(ok: bool, message: &'static str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(message.into())
    }
}

/// Process-wide monotonic clock shared by media timestamps and browser clock synchronization.
pub fn monotonic_nanos() -> i64 {
    static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    EPOCH
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_nanos()
        .min(i64::MAX as u128) as i64
}
