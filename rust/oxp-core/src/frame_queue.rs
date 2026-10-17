// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use crate::model::VideoFrame;
use std::collections::VecDeque;

#[derive(Clone, Debug)]
pub struct QueuedFrame {
    pub frame: VideoFrame,
    pub gap: bool,
}
/// One screen per queue. Callers serialize access; requests are returned for dispatch after unlocking.
pub struct FrameQueue {
    frames: VecDeque<QueuedFrame>,
    bytes: usize,
    max_duration_us: i64,
    max_bytes: usize,
    waiting: bool,
    gap: bool,
    requested: bool,
    overflowed: bool,
    dropped: u64,
    session: Option<i64>,
    screen: Option<u32>,
}
impl Default for FrameQueue {
    fn default() -> Self {
        Self::new(100_000, 8 * 1024 * 1024)
    }
}
impl FrameQueue {
    pub fn new(max_duration_us: i64, max_bytes: usize) -> Self {
        assert!(max_duration_us >= 0 && max_bytes > 0);
        Self {
            frames: VecDeque::new(),
            bytes: 0,
            max_duration_us,
            max_bytes,
            waiting: true,
            gap: true,
            requested: false,
            overflowed: false,
            dropped: 0,
            session: None,
            screen: None,
        }
    }
    pub fn max_duration_us(&self) -> i64 {
        self.max_duration_us
    }
    /// Takes effect from the next offer, so a playback change keeps the decoding history.
    pub fn set_max_duration_us(&mut self, max_duration_us: i64) {
        assert!(max_duration_us >= 0);
        self.max_duration_us = max_duration_us;
    }
    /// Frames discarded for back-pressure: those evicted when the queue outgrew its bounds, plus
    /// later frames refused until the key frame that recovers. Waiting for a first key frame is not counted.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
    pub fn len(&self) -> usize {
        self.frames.len()
    }
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
    pub fn queued_bytes(&self) -> usize {
        self.bytes
    }
    pub fn duration_us(&self) -> i64 {
        self.frames
            .back()
            .zip(self.frames.front())
            .map_or(0, |(b, f)| b.frame.pts_us.saturating_sub(f.frame.pts_us))
    }
    fn reset(&mut self) {
        self.frames.clear();
        self.bytes = 0;
        self.waiting = true;
        self.gap = true;
        self.requested = false;
        self.overflowed = false;
    }
    pub fn clear(&mut self) {
        self.reset();
        self.session = None;
    }
    pub fn poll(&mut self) -> Option<QueuedFrame> {
        let f = self.frames.pop_front()?;
        self.bytes -= f.frame.data.len();
        Some(f)
    }
    /// Returns (retained, request_keyframe). A single request covers each outstanding gap.
    pub fn offer(&mut self, frame: VideoFrame) -> (bool, bool) {
        assert!(!frame.data.is_empty());
        assert!(self.screen.is_none_or(|s| s == frame.screen));
        self.screen = Some(frame.screen);
        if self.session.is_some_and(|s| frame.session < s) {
            return (false, false);
        }
        if self.session != Some(frame.session) {
            self.reset();
            self.session = Some(frame.session);
        }
        if self
            .frames
            .back()
            .is_some_and(|f| frame.pts_us < f.frame.pts_us)
        {
            self.reset();
        }
        if (self.waiting && !frame.key_frame) || frame.data.len() > self.max_bytes {
            if frame.data.len() > self.max_bytes {
                let r = self.requested;
                self.dropped += self.frames.len() as u64;
                self.reset();
                self.requested = r;
                self.overflowed = true;
            }
            if self.overflowed {
                self.dropped += 1;
            }
            let request = !self.requested;
            self.requested = true;
            return (false, request);
        }
        self.waiting = false;
        self.bytes += frame.data.len();
        let key = frame.key_frame;
        self.frames.push_back(QueuedFrame {
            frame,
            gap: self.gap,
        });
        self.gap = false;
        let mut request = false;
        while self.bytes > self.max_bytes || self.duration_us() > self.max_duration_us {
            self.overflowed = true;
            self.poll();
            self.dropped += 1;
            while self.frames.front().is_some_and(|f| !f.frame.key_frame) {
                self.poll();
                self.dropped += 1;
            }
            if let Some(f) = self.frames.front_mut() {
                f.gap = true;
            } else {
                self.waiting = true;
                self.gap = true;
            }
            if !self.requested {
                self.requested = true;
                request = true;
            }
        }
        if key && !self.frames.is_empty() {
            self.requested = false;
            self.overflowed = false;
        }
        (!self.frames.is_empty(), request)
    }
}
