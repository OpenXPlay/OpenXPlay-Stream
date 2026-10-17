// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14.

use oxp_core::model::*;
use std::collections::{BTreeMap, VecDeque};
const MAX_QUEUED_US: i64 = 1_500_000;

#[derive(Default)]
pub(crate) struct AudioQueue {
    packets: VecDeque<(AudioPacket, i64, u64)>,
    // Format markers follow the last packet accepted before their callback.
    markers: VecDeque<(u64, serde_json::Value)>,
    offered: u64,
    generations: BTreeMap<u32, u64>,
    duration_us: i64,
    pub dropped: u64,
    pub sent: u64,
}
impl AudioQueue {
    pub fn format(&mut self, message: serde_json::Value) -> bool {
        if self.markers.len() >= 256 {
            return false;
        }
        self.markers.push_back((self.offered, message));
        true
    }
    fn marker_ready(&self) -> bool {
        self.markers.front().is_some_and(|(n, _)| {
            self.packets
                .front()
                .is_none_or(|(_, _, ordinal)| ordinal > n)
        })
    }
    pub fn poll_format(&mut self) -> Option<serde_json::Value> {
        if !self.marker_ready() {
            return None;
        }
        self.markers.pop_front().map(|(_, v)| v)
    }
    pub fn generation(&self, stream: u32) -> u64 {
        self.generations.get(&stream).copied().unwrap_or(0)
    }
    pub fn remove(&mut self, stream: u32) {
        let next = self.generation(stream).wrapping_add(1);
        self.generations.insert(stream, next);
        self.packets.retain(|(p, _, _)| p.stream != stream);
        self.duration_us = self.packets.iter().map(|(_, d, _)| d).sum();
    }
    pub fn clear(&mut self) {
        self.markers.clear();
        for stream in self.generations.keys().copied().collect::<Vec<_>>() {
            self.remove(stream);
        }
    }
    pub fn offer(&mut self, p: AudioPacket, f: &AudioFormat) {
        if f.sample_rate == 0 || f.channels == 0 {
            return;
        }
        let samples = if f.codec == AudioCodec::PcmS16le {
            p.data.len() as i64 / (f.channels as i64 * 2)
        } else {
            1024
        };
        if samples <= 0 {
            return;
        }
        let duration = samples * 1_000_000 / f.sample_rate as i64;
        let pts = p.pts_us;
        self.generations.entry(p.stream).or_default();
        self.offered += 1;
        self.packets.push_back((p, duration, self.offered));
        self.duration_us += duration;
        // CarPlay buffered media allows 1 s of latency and arrives in Wi-Fi batches; 0.5 s dropped a quarter of it.
        while self.packets.front().is_some_and(|(p, _, _)| {
            self.duration_us > MAX_QUEUED_US
                || pts.saturating_sub(p.pts_us).saturating_add(duration) > MAX_QUEUED_US
        }) {
            self.duration_us -= self.packets.pop_front().unwrap().1;
            self.dropped += 1;
        }
    }
    pub fn poll(&mut self) -> Option<(AudioPacket, u64)> {
        if self.marker_ready() {
            return None;
        }
        let (p, d, _) = self.packets.pop_front()?;
        self.duration_us -= d;
        self.sent += 1;
        let generation = self.generation(p.stream);
        Some((p, generation))
    }
}
pub(crate) fn valid_mic_packet(r: &MicRequest, size: usize) -> bool {
    r.codec == AudioCodec::PcmS16le
        && r.frame_ms == 20
        && (8000..=48000).contains(&r.sample_rate)
        && (1..=2).contains(&r.channels)
        && r.sample_rate.is_multiple_of(50)
        && size > 0
        && size.is_multiple_of(r.sample_rate as usize / 50 * r.channels as usize * 2)
}
