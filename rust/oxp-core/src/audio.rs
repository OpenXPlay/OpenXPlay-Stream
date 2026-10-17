// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14.

use crate::{require, Result};
use std::{
    collections::{BTreeMap, VecDeque},
    f64::consts::PI,
};

/// Streaming windowed-sinc conversion, with rational phase and retained lookahead.
pub struct PcmResampler {
    input_rate: u32,
    input_channels: usize,
    output_rate: u32,
    output_channels: usize,
    cutoff: f64,
    radius: i64,
    weights: BTreeMap<u32, Vec<f64>>,
    samples: Vec<f64>,
    base: i64,
    total: i64,
    position: i64,
    phase: u32,
    partial: Vec<u8>,
    finished: bool,
}
fn sample(bytes: &[u8]) -> f64 {
    i16::from_le_bytes([bytes[0], bytes[1]]) as f64
}
fn put(bytes: &mut Vec<u8>, value: f64) {
    // Kotlin rounds halfway values toward positive infinity.
    bytes.extend_from_slice(&((value + 0.5).floor().clamp(-32768.0, 32767.0) as i16).to_le_bytes());
}
impl PcmResampler {
    pub fn new(
        input_rate: u32,
        input_channels: u32,
        output_rate: u32,
        output_channels: u32,
    ) -> Result<Self> {
        require(
            (1..=384_000).contains(&input_rate)
                && (1..=384_000).contains(&output_rate)
                && (1..=2).contains(&input_channels)
                && (1..=2).contains(&output_channels),
            "Invalid PCM format",
        )?;
        let cutoff = 0.94 * (output_rate as f64 / input_rate as f64).min(1.0);
        Ok(Self {
            input_rate,
            input_channels: input_channels as usize,
            output_rate,
            output_channels: output_channels as usize,
            cutoff,
            radius: (32.0 / cutoff).ceil() as i64,
            weights: BTreeMap::new(),
            samples: Vec::new(),
            base: 0,
            total: 0,
            position: 0,
            phase: 0,
            partial: Vec::new(),
            finished: false,
        })
    }
    pub fn reset(&mut self) {
        self.samples.clear();
        self.base = 0;
        self.total = 0;
        self.position = 0;
        self.phase = 0;
        self.partial.clear();
        self.finished = false;
    }
    pub fn process(&mut self, pcm: &[u8]) -> Result<Vec<u8>> {
        require(!self.finished, "Resampler has ended")?;
        let mut bytes = std::mem::take(&mut self.partial);
        bytes.extend_from_slice(pcm);
        let frame_bytes = self.input_channels * 2;
        let complete = bytes.len() / frame_bytes * frame_bytes;
        self.partial = bytes.split_off(complete);
        if self.input_rate == self.output_rate && self.input_channels == self.output_channels {
            return Ok(bytes);
        }
        let mut out = Vec::new();
        for frame in bytes.chunks_exact(frame_bytes) {
            for channel in 0..self.output_channels {
                let value = if self.input_channels == 2 && self.output_channels == 1 {
                    (sample(frame) + sample(&frame[2..])) / 2.0
                } else {
                    sample(
                        &frame[if self.input_channels == 1 {
                            0
                        } else {
                            channel * 2
                        }..],
                    )
                };
                if self.input_rate == self.output_rate {
                    put(&mut out, value);
                } else {
                    self.samples.push(value);
                }
            }
        }
        if self.input_rate == self.output_rate {
            return Ok(out);
        }
        self.total = self
            .total
            .checked_add((complete / frame_bytes) as i64)
            .ok_or("PCM duration overflow")?;
        Ok(self.emit(false))
    }
    pub fn finish(&mut self) -> Result<Vec<u8>> {
        require(self.partial.is_empty(), "Incomplete S16LE sample frame")?;
        if self.finished {
            return Ok(Vec::new());
        }
        self.finished = true;
        let out = if self.input_rate == self.output_rate {
            Vec::new()
        } else {
            self.emit(true)
        };
        self.samples.clear();
        Ok(out)
    }
    fn kernel(&self) -> Vec<f64> {
        let fraction = self.phase as f64 / self.output_rate as f64;
        let mut kernel: Vec<_> = (0..self.radius * 2)
            .map(|tap| {
                let d = (tap - self.radius + 1) as f64 - fraction;
                let x = PI * self.cutoff * d;
                let sinc = if x.abs() < 1e-12 {
                    self.cutoff
                } else {
                    self.cutoff * x.sin() / x
                };
                let window = if d.abs() >= self.radius as f64 {
                    0.0
                } else {
                    0.42 + 0.5 * (PI * d / self.radius as f64).cos()
                        + 0.08 * (2.0 * PI * d / self.radius as f64).cos()
                };
                sinc * window
            })
            .collect();
        let sum: f64 = kernel.iter().sum();
        for w in &mut kernel {
            *w /= sum;
        }
        kernel
    }
    fn emit(&mut self, end: bool) -> Vec<u8> {
        let mut out = Vec::new();
        while self.position < self.total && (end || self.total - self.position > self.radius) {
            if !self.weights.contains_key(&self.phase) && self.weights.len() < 1024 {
                let kernel = self.kernel();
                self.weights.insert(self.phase, kernel);
            }
            let fallback;
            let kernel = match self.weights.get(&self.phase) {
                Some(kernel) => kernel,
                None => {
                    fallback = self.kernel();
                    &fallback
                }
            };
            for channel in 0..self.output_channels {
                let mut value = 0.0;
                for (tap, w) in kernel.iter().enumerate() {
                    let index = self.position + tap as i64 - self.radius + 1;
                    if index >= self.base && index < self.total {
                        value += self.samples
                            [(index - self.base) as usize * self.output_channels + channel]
                            * w;
                    }
                }
                put(&mut out, value);
            }
            let next = self.phase as u64 + self.input_rate as u64;
            self.position += (next / self.output_rate as u64) as i64;
            self.phase = (next % self.output_rate as u64) as u32;
        }
        let discard = (self.position - self.radius + 1 - self.base)
            .max(0)
            .min(self.total - self.base);
        self.samples
            .drain(..discard as usize * self.output_channels);
        self.base += discard;
        out
    }
}
struct Source {
    resampler: PcmResampler,
    samples: VecDeque<i16>,
    ended: bool,
}
/// Caller-paced saturating mixer; live underruns produce silence, final frames are padded.
pub struct PcmMixer {
    rate: u32,
    channels: u32,
    frame_samples: usize,
    sources: BTreeMap<u32, Source>,
}
impl PcmMixer {
    pub fn new(rate: u32, channels: u32, frame_ms: u32) -> Result<Self> {
        let n = rate as u64 * frame_ms as u64;
        require(
            (1..=384_000).contains(&rate)
                && (1..=2).contains(&channels)
                && frame_ms > 0
                && n.is_multiple_of(1000)
                && n / 1000 <= i32::MAX as u64 / (channels as u64 * 2),
            "Invalid mixer frame",
        )?;
        Ok(Self {
            rate,
            channels,
            frame_samples: (n / 1000) as usize,
            sources: BTreeMap::new(),
        })
    }
    pub fn is_drained(&self) -> bool {
        self.sources
            .values()
            .all(|s| s.ended && s.samples.is_empty())
    }
    /// An ended source must drain before the same stream can start a new run.
    pub fn stream_drained(&self, stream: u32) -> bool {
        self.sources
            .get(&stream)
            .is_none_or(|s| s.ended && s.samples.is_empty())
    }
    pub fn add_stream(&mut self, stream: u32, rate: u32, channels: u32) -> Result<()> {
        require(
            self.sources
                .get(&stream)
                .is_none_or(|s| s.ended && s.samples.is_empty()),
            "Duplicate PCM stream",
        )?;
        self.sources.insert(
            stream,
            Source {
                resampler: PcmResampler::new(rate, channels, self.rate, self.channels)?,
                samples: VecDeque::new(),
                ended: false,
            },
        );
        Ok(())
    }
    pub fn feed(&mut self, stream: u32, bytes: &[u8]) -> Result<()> {
        let s = self.sources.get_mut(&stream).ok_or("Unknown PCM stream")?;
        require(!s.ended, "PCM stream has ended")?;
        let out = s.resampler.process(bytes)?;
        s.samples.extend(
            out.as_chunks::<2>()
                .0
                .iter()
                .map(|&b| i16::from_le_bytes(b)),
        );
        Ok(())
    }
    pub fn end_stream(&mut self, stream: u32) -> Result<()> {
        let s = self.sources.get_mut(&stream).ok_or("Unknown PCM stream")?;
        if !s.ended {
            let out = s.resampler.finish()?;
            s.samples.extend(
                out.as_chunks::<2>()
                    .0
                    .iter()
                    .map(|&b| i16::from_le_bytes(b)),
            );
            s.ended = true;
        }
        Ok(())
    }
    pub fn remove_stream(&mut self, stream: u32) {
        self.sources.remove(&stream);
    }
    pub fn read_frame(&mut self) -> Option<Vec<u8>> {
        if self.is_drained() {
            return None;
        }
        let mut out = Vec::with_capacity(self.frame_samples * self.channels as usize * 2);
        for _ in 0..self.frame_samples * self.channels as usize {
            let sum: i64 = self
                .sources
                .values_mut()
                .map(|s| s.samples.pop_front().unwrap_or(0) as i64)
                .sum();
            out.extend_from_slice(&(sum.clamp(-32768, 32767) as i16).to_le_bytes());
        }
        self.sources
            .retain(|_, s| !s.ended || !s.samples.is_empty());
        Some(out)
    }
}
impl Default for PcmMixer {
    fn default() -> Self {
        Self::new(16000, 1, 20).unwrap()
    }
}
/// Sample-accurate timestamps allowing 1.5 s buffered lead and 100 ms arrival lag.
pub struct AudioClock {
    rate: u32,
    next: Option<i64>,
    remainder: i64,
}
impl AudioClock {
    pub fn new(rate: u32) -> Result<Self> {
        require(rate > 0, "Invalid audio sample rate")?;
        Ok(Self {
            rate,
            next: None,
            remainder: 0,
        })
    }
    pub fn reset(&mut self) {
        self.next = None;
        self.remainder = 0;
    }
    pub fn timestamp(&mut self, samples: u32, arrival_us: i64) -> Result<i64> {
        require(samples > 0, "Audio packets require samples")?;
        let anchor = self.next.is_none_or(|n| {
            n.abs_diff(arrival_us) > if n > arrival_us { 1_500_000 } else { 100_000 }
        });
        let pts = if anchor {
            arrival_us
        } else {
            self.next.unwrap()
        };
        let fraction = samples as i64 * 1_000_000 + if anchor { 0 } else { self.remainder };
        self.next = Some(
            pts.checked_add(fraction / self.rate as i64)
                .ok_or("Audio timestamp overflow")?,
        );
        self.remainder = fraction % self.rate as i64;
        Ok(pts)
    }
}
