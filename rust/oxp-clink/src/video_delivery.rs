// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use crate::{model::VideoFrame, require, Result};
use std::collections::VecDeque;
#[derive(Default)]
pub struct VideoSwitch {
    pub playing: bool,
    pub live: bool,
}
impl VideoSwitch {
    pub fn play(&mut self, value: bool) {
        self.playing = value;
        self.reset();
    }
    pub fn reset(&mut self) {
        self.live = false;
    }
    pub fn accept(&mut self, key: bool) -> bool {
        if !self.playing || (!self.live && !key) {
            return false;
        }
        self.live = true;
        true
    }
    pub fn placeholder(&self) -> bool {
        self.playing && !self.live
    }
}
/// Caller serializes access; no I/O occurs in the queue critical section.
pub struct VideoQueue {
    frames: VecDeque<VideoFrame>,
    bytes: usize,
    waiting: bool,
    stalled: bool,
    gap: bool,
    pub dropped: u64,
    max_bytes: usize,
    max_duration_us: i64,
    max_frames: usize,
}
impl Default for VideoQueue {
    fn default() -> Self {
        Self::new(4 * 1024 * 1024, 600_000, 120).unwrap()
    }
}
impl VideoQueue {
    pub fn new(max_bytes: usize, max_duration_us: i64, max_frames: usize) -> Result<Self> {
        require(
            max_bytes > 0 && max_duration_us > 0 && max_frames > 0,
            "invalid_queue_limits",
        )?;
        Ok(Self {
            frames: VecDeque::new(),
            bytes: 0,
            waiting: true,
            stalled: false,
            gap: false,
            dropped: 0,
            max_bytes,
            max_duration_us,
            max_frames,
        })
    }
    pub fn reset(&mut self) {
        self.dropped += self.frames.len() as u64;
        self.frames.clear();
        self.bytes = 0;
        self.waiting = true;
        self.gap = true;
    }
    pub fn set_stalled(&mut self, stalled: bool) {
        self.stalled = stalled;
        self.reset();
    }
    pub fn offer(&mut self, f: VideoFrame) -> bool {
        self.offer_observed(f, || {})
    }
    /// Observes overflow only; waiting for an IDR also requests a keyframe.
    pub(crate) fn offer_observed(&mut self, f: VideoFrame, overflowed: impl FnOnce()) -> bool {
        if self.stalled {
            self.dropped += 1;
            return false;
        }
        if self.waiting && !f.key_frame {
            self.dropped += 1;
            return true;
        }
        let overflow = self.bytes + f.data.len() > self.max_bytes
            || self.frames.len() >= self.max_frames
            || self
                .frames
                .front()
                .is_some_and(|first| f.pts_us.wrapping_sub(first.pts_us) > self.max_duration_us);
        if overflow {
            overflowed();
            self.reset();
        }
        if f.data.len() > self.max_bytes || (self.waiting && !f.key_frame) {
            self.dropped += 1;
            return true;
        }
        if f.key_frame {
            self.waiting = false;
        }
        self.bytes += f.data.len();
        self.frames.push_back(f);
        overflow
    }
    pub fn take_gap(&mut self) -> bool {
        std::mem::take(&mut self.gap)
    }
    pub fn poll(&mut self) -> Option<VideoFrame> {
        let f = self.frames.pop_front()?;
        self.bytes -= f.data.len();
        Some(f)
    }
}
