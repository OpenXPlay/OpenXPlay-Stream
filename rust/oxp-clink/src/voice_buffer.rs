// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

use crate::Result;
use oxp_core::audio::PcmResampler;
use std::collections::VecDeque;

/// Hold raw PCM before the mixer so an underrun does not consume the refill.
pub(crate) struct VoiceBuffer {
    rate: u32,
    channels: u32,
    data: VecDeque<u8>,
    pub target_us: u64,
    ended: bool,
    waiting: bool,
    arrival: Option<u64>,
    stable_since: Option<u64>,
    gaps: VecDeque<(u64, u64)>,
}
impl VoiceBuffer {
    pub fn new(rate: u32, channels: u32) -> Self {
        Self {
            rate,
            channels,
            data: VecDeque::new(),
            target_us: 80_000,
            ended: false,
            waiting: true,
            arrival: None,
            stable_since: None,
            gaps: VecDeque::new(),
        }
    }
    fn bytes(&self, us: u64) -> usize {
        (us * self.rate as u64 / 1_000_000) as usize * self.channels as usize * 2
    }
    pub fn ready(&self) -> bool {
        if self.ended {
            !self.data.is_empty()
        } else {
            !self.waiting || self.data.len() >= self.bytes(self.target_us)
        }
    }
    pub fn drained(&self) -> bool {
        self.ended && self.data.is_empty()
    }
    pub fn end(&mut self) {
        self.ended = true;
    }
    fn decay(&mut self, now: u64) {
        while self
            .gaps
            .front()
            .is_some_and(|g| now.saturating_sub(g.0) > 10_000_000)
        {
            self.gaps.pop_front();
        }
        let elapsed = now.saturating_sub(self.stable_since.unwrap_or(now));
        if elapsed >= 10_000_000 {
            self.target_us = self
                .target_us
                .saturating_sub(elapsed / 10_000_000 * 10_000)
                .max(80_000);
            self.stable_since = Some(now);
        }
    }
    pub fn push(&mut self, data: &[u8], now: u64) {
        if self.ended || data.is_empty() {
            return;
        }
        if let Some(last) = self.arrival {
            self.gaps.push_back((now, now.saturating_sub(last)));
        }
        self.arrival = Some(now);
        self.stable_since.get_or_insert(now);
        self.data.extend(data);
        self.decay(now);
        let excess = self
            .data
            .len()
            .saturating_sub(self.bytes(self.target_us + 200_000));
        let alignment = self.channels as usize * 2;
        self.data.drain(..excess / alignment * alignment);
    }
    pub fn read(&mut self, now: u64) -> Option<Vec<u8>> {
        self.decay(now);
        if self.ended {
            let n = self.data.len().min(self.bytes(20_000));
            return (n > 0).then(|| self.data.drain(..n).collect());
        }
        if self.waiting {
            if self.data.len() < self.bytes(self.target_us) {
                return None;
            }
            self.waiting = false;
        }
        if self.data.len() < self.bytes(20_000) {
            self.waiting = true;
            self.target_us =
                (self.target_us + self.gaps.iter().map(|g| g.1).max().unwrap_or(0)).min(300_000);
            self.stable_since = Some(now);
            return None;
        }
        let n = self.bytes(20_000);
        Some(self.data.drain(..n).collect())
    }
}
/// Resample before buffering so filter lookahead cannot pad paced mixer reads.
pub(crate) struct VoiceStream {
    resampler: PcmResampler,
    buffer: VoiceBuffer,
    ended: bool,
    arrival_us: u64,
}
impl VoiceStream {
    pub fn new(rate: u32, channels: u32) -> Result<Self> {
        Ok(Self {
            resampler: PcmResampler::new(rate, channels, 16000, 1)?,
            buffer: VoiceBuffer::new(16000, 1),
            ended: false,
            arrival_us: 0,
        })
    }
    pub fn ready(&self) -> bool {
        self.buffer.ready()
    }
    pub fn drained(&self) -> bool {
        self.buffer.drained()
    }
    pub fn push(&mut self, data: &[u8], now: u64) -> Result<()> {
        if !self.ended {
            self.arrival_us = now;
            self.buffer.push(&self.resampler.process(data)?, now);
        }
        Ok(())
    }
    pub fn end(&mut self) -> Result<()> {
        if !self.ended {
            self.buffer.push(&self.resampler.finish()?, self.arrival_us);
            self.buffer.end();
            self.ended = true;
        }
        Ok(())
    }
    pub fn read(&mut self, now: u64) -> Option<Vec<u8>> {
        self.buffer.read(now)
    }
}

#[derive(Default)]
pub(crate) struct VoicePacer {
    start_us: u64,
    frames: u64,
}
impl VoicePacer {
    pub fn reset(&mut self, now: u64) {
        self.start_us = now;
        self.frames = 0;
    }
    pub fn next(&mut self, now: u64) -> u64 {
        if now.saturating_sub(self.start_us + self.frames * 20_000) > 200_000 {
            self.reset(now);
        }
        self.frames += 1;
        self.start_us + self.frames * 20_000
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn starts_at_eighty_ms_and_preserves_pcm() {
        let mut b = VoiceBuffer::new(16000, 1);
        b.push(&vec![1; 1920], 0);
        assert!(b.read(0).is_none());
        b.push(&vec![2; 640], 20_000);
        for _ in 0..3 {
            assert_eq!(b.read(20_000).unwrap(), vec![1; 640]);
        }
        assert_eq!(b.read(20_000).unwrap(), vec![2; 640]);
        assert!(b.read(40_000).is_none());
        assert_eq!(b.target_us, 100_000);
        b.push(&vec![0; 2560], 60_000);
        assert!(b.read(60_000).is_none());
        b.push(&vec![0; 640], 80_000);
        assert!(b.read(80_000).is_some());
    }
    #[test]
    fn largest_recent_gap_caps_target_and_quiet_time_decays_it() {
        let mut b = VoiceBuffer::new(16000, 1);
        b.push(&vec![0; 1280], 0);
        b.push(&vec![0; 1280], 250_000);
        for _ in 0..4 {
            assert!(b.read(250_000).is_some());
        }
        assert!(b.read(270_000).is_none());
        assert_eq!(b.target_us, 300_000);
        b.read(10_270_000);
        assert_eq!(b.target_us, 290_000);
        b.read(220_270_000);
        assert_eq!(b.target_us, 80_000);
    }
    #[test]
    fn trims_oldest_aligned_samples_and_drains_short_ended_streams() {
        let mut b = VoiceBuffer::new(16000, 1);
        let data: Vec<u8> = (0..9600).map(|i| if i < 640 { 1 } else { 2 }).collect();
        b.push(&data, 0);
        assert_eq!(b.data.len(), 8960);
        assert_eq!(b.read(0).unwrap(), vec![2; 640]);
        let mut short = VoiceBuffer::new(8000, 2);
        short.push(&[7; 12], 0);
        assert!(short.read(0).is_none());
        short.end();
        assert_eq!(short.read(0).unwrap(), [7; 12]);
        assert!(short.drained());
        assert!(short.read(0).is_none());
    }
    #[test]
    fn resampled_streams_drain_without_padding_between_paced_frames() {
        for rate in [8000, 16000, 48000] {
            for channels in 1..=2 {
                let mut stream = VoiceStream::new(rate, channels).unwrap();
                let input: Vec<u8> = (0..rate * channels * 80 / 1000)
                    .flat_map(|_| 1000i16.to_le_bytes())
                    .collect();
                for data in input.chunks(37) {
                    stream.push(data, 0).unwrap();
                }
                stream.end().unwrap();
                stream.end().unwrap();
                let mut out = Vec::new();
                while !stream.drained() {
                    out.extend(stream.read(0).unwrap());
                }
                assert_eq!(out.len(), 2560, "rate={rate} channels={channels}");
                assert!(stream.read(0).is_none());
            }
        }
    }

    #[test]
    fn streams_refill_independently() {
        let mut a = VoiceBuffer::new(16000, 1);
        let mut b = VoiceBuffer::new(16000, 1);
        a.push(&vec![0; 2560], 0);
        b.push(&vec![0; 640], 0);
        assert!(a.read(0).is_some());
        assert!(b.read(0).is_none());
        assert_eq!(b.target_us, 80_000);
    }
    #[test]
    fn deadlines_do_not_accumulate_polling_delay_and_stalls_reanchor() {
        let mut p = VoicePacer::default();
        p.reset(1_000_000);
        for n in 0..500 {
            assert_eq!(
                p.next(1_000_000 + n * 20_000 + 2000),
                1_000_000 + (n + 1) * 20_000
            );
        }
        assert_eq!(p.next(11_200_000), 11_020_000);
        assert_eq!(p.next(11_220_001), 11_240_001);
        p.reset(30_000_000);
        assert_eq!(p.next(30_000_000), 30_020_000);
    }
}
