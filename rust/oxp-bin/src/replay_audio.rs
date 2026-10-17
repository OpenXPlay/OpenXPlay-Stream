// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14.

use crate::{event, Args};
use oxp_core::audio::AudioClock;
use oxp_wire::{AudioCodec, AudioFormat, AudioRole, Header, Kind, Message, MicRequest, Result};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Seek, SeekFrom, Write},
};

pub struct WavClip {
    pub rate: u32,
    pub channels: u32,
    pub pcm: Vec<u8>,
}
impl WavClip {
    pub fn parse(b: &[u8]) -> Result<Self> {
        if b.len() < 12
            || &b[..4] != b"RIFF"
            || &b[8..12] != b"WAVE"
            || u32::from_le_bytes(b[4..8].try_into()?) as u64 + 8 != b.len() as u64
        {
            return Err("Invalid WAV header or RIFF length".into());
        }
        let mut format = None;
        let mut pcm = None;
        let mut pos = 12;
        while pos < b.len() {
            if b.len() - pos < 8 {
                return Err("Truncated WAV chunk".into());
            }
            let n = u32::from_le_bytes(b[pos + 4..pos + 8].try_into()?) as usize;
            let start = pos + 8;
            let end = start.checked_add(n).ok_or("WAV size overflow")?;
            if end.checked_add(n & 1).is_none_or(|v| v > b.len()) {
                return Err("Truncated WAV payload".into());
            }
            match &b[pos..pos + 4] {
                b"fmt " => {
                    if format.is_some() || n < 16 {
                        return Err("Duplicate or short WAV format".into());
                    }
                    let u16_at = |p| u16::from_le_bytes([b[p], b[p + 1]]);
                    let u32_at = |p| u32::from_le_bytes([b[p], b[p + 1], b[p + 2], b[p + 3]]);
                    let channels = u16_at(start + 2) as u32;
                    let rate = u32_at(start + 4);
                    if u16_at(start) != 1
                        || u16_at(start + 14) != 16
                        || !(1..=2).contains(&channels)
                        || !(8000..=48000).contains(&rate)
                        || u16_at(start + 12) as u32 != channels * 2
                        || u32_at(start + 8) != rate * channels * 2
                    {
                        return Err("WAV requires PCM S16LE mono/stereo at 8–48 kHz".into());
                    }
                    format = Some((rate, channels));
                }
                b"data" => {
                    if pcm.is_some() {
                        return Err("Multiple WAV data chunks".into());
                    }
                    pcm = Some(b[start..end].to_vec());
                }
                _ => {}
            }
            pos = end + (n & 1);
        }
        let (rate, channels) = format.ok_or("Missing WAV format")?;
        let pcm = pcm.ok_or("Missing WAV data")?;
        if pcm.is_empty() || pcm.len() % (channels as usize * 2) != 0 {
            return Err("Incomplete PCM frames".into());
        }
        Ok(Self {
            rate,
            channels,
            pcm,
        })
    }
    fn frames(&self) -> usize {
        self.pcm.len() / (self.channels as usize * 2)
    }
}
pub struct WavWriter {
    file: File,
    rate: u32,
    size: u32,
}
impl WavWriter {
    pub fn new(path: &str, rate: u32) -> Result<Self> {
        let mut s = Self {
            file: File::create(path)?,
            rate,
            size: 0,
        };
        s.header()?;
        Ok(s)
    }
    fn header(&mut self) -> Result<()> {
        let mut b = b"RIFF".to_vec();
        b.extend_from_slice(&(self.size + 36).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&self.rate.to_le_bytes());
        b.extend_from_slice(&(self.rate * 2).to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&self.size.to_le_bytes());
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&b)?;
        Ok(())
    }
    pub fn write(&mut self, b: &[u8]) -> Result<()> {
        if !b.len().is_multiple_of(2) || b.len() as u64 + self.size as u64 > u32::MAX as u64 - 36 {
            return Err("WAV size overflow or partial sample".into());
        }
        self.file.seek(SeekFrom::End(0))?;
        self.file.write_all(b)?;
        self.size += b.len() as u32;
        // Keep an interrupted replay's captured WAV readable too.
        self.header()
    }
}
impl Drop for WavWriter {
    fn drop(&mut self) {
        let _ = self.header();
    }
}
#[derive(Default)]
struct Totals {
    packets: u64,
    samples: u64,
    squares: f64,
}
pub struct ReplayAudio {
    media: Vec<Vec<u8>>,
    voice: Option<WavClip>,
    every: Option<f64>,
    pub mic_rate: Option<u32>,
    writer: Option<WavWriter>,
    media_index: usize,
    media_samples: u64,
    media_base: u64,
    media_due: u64,
    voice_frame: usize,
    voice_packet: u64,
    voice_start: u64,
    voice_due: u64,
    voice_active: bool,
    mic_active: bool,
    media_clock: AudioClock,
    voice_clock: Option<AudioClock>,
    sequence: [u32; 2],
    summary_due: u64,
    totals: BTreeMap<(String, String), Totals>,
    stdio: bool,
}
impl ReplayAudio {
    pub fn new(args: &Args) -> Result<Self> {
        let media = if let Some(path) = args.get("--audio-media") {
            let units = oxp_media::Adts::parse(&std::fs::read(path)?)?;
            if units.is_empty()
                || units.iter().any(|u| {
                    u.config.object_type != 2
                        || u.config.sample_rate != 48000
                        || u.config.channel_configuration != 2
                })
            {
                return Err("MEDIA requires AAC-LC 48 kHz stereo".into());
            }
            units.into_iter().map(|u| u.data).collect()
        } else {
            Vec::new()
        };
        let voice = args
            .get("--audio-voice")
            .map(|p| {
                std::fs::read(p)
                    .map_err(Into::into)
                    .and_then(|b| WavClip::parse(&b))
            })
            .transpose()?;
        let every = args
            .get("--audio-voice-every")
            .map(str::parse::<f64>)
            .transpose()?;
        if every.is_some_and(|v| {
            voice.is_none() || !v.is_finite() || v < 1e-9 || v >= i64::MAX as f64 / 1e9
        }) {
            return Err("--audio-voice-every requires voice and a finite positive interval".into());
        }
        let mic_rate = args.get("--mic-rate").map(str::parse::<u32>).transpose()?;
        if mic_rate.is_some_and(|r| !(8000..=48000).contains(&r)) {
            return Err("Microphone rate must be 8–48 kHz".into());
        }
        let writer = if let Some(path) = args.get("--mic-out") {
            Some(WavWriter::new(
                path,
                mic_rate.ok_or("--mic-out requires --mic-rate")?,
            )?)
        } else {
            None
        };
        let voice_clock = voice.as_ref().map(|v| AudioClock::new(v.rate).unwrap());
        Ok(Self {
            media_due: if media.is_empty() { u64::MAX } else { 0 },
            voice_due: if voice.is_none() { u64::MAX } else { 0 },
            media,
            voice,
            every,
            mic_rate,
            writer,
            media_index: 0,
            media_samples: 0,
            media_base: 0,
            voice_frame: 0,
            voice_packet: 0,
            voice_start: 0,
            voice_active: false,
            mic_active: false,
            media_clock: AudioClock::new(48000)?,
            voice_clock,
            sequence: [0; 2],
            summary_due: 1_000_000_000,
            totals: BTreeMap::new(),
            stdio: args.get("--stdio").is_some(),
        })
    }
    pub fn start(&mut self, w: &mut impl Write) -> Result<()> {
        if !self.media.is_empty() {
            oxp_wire::write(
                w,
                &Message::audio_format(
                    0,
                    Some(&AudioFormat {
                        role: AudioRole::Media,
                        codec: AudioCodec::AacLc,
                        sample_rate: 48000,
                        channels: 2,
                        config: Some(vec![0x11, 0x90]),
                    }),
                )?,
            )?;
        }
        if self.every.is_none() {
            self.microphone(w, true)?;
        }
        Ok(())
    }
    fn microphone(&mut self, w: &mut impl Write, active: bool) -> Result<()> {
        let Some(rate) = self.mic_rate else {
            return Ok(());
        };
        if active == self.mic_active {
            return Ok(());
        }
        self.mic_active = active;
        let request = MicRequest {
            codec: AudioCodec::PcmS16Le,
            sample_rate: rate,
            channels: 1,
            frame_ms: 20,
        };
        oxp_wire::write(
            w,
            &Message::microphone(1, if active { Some(&request) } else { None })?,
        )?;
        Ok(())
    }
    pub fn next_deadline(&self) -> u64 {
        self.media_due
            .min(self.voice_due)
            .min(if self.mic_rate.is_some() {
                self.summary_due
            } else {
                u64::MAX
            })
    }
    fn packet(
        &mut self,
        w: &mut impl Write,
        stream: u8,
        pts_us: i64,
        data: Vec<u8>,
        arrival: i64,
    ) -> Result<()> {
        let seq = &mut self.sequence[stream as usize];
        let header = Header {
            kind: Kind::Audio,
            stream,
            flags: 0,
            sequence: *seq,
            pts_us,
            sender_clock_us: arrival,
        };
        *seq = seq.wrapping_add(1);
        oxp_wire::write(
            w,
            &Message::Binary {
                header,
                payload: data,
            },
        )?;
        Ok(())
    }
    pub fn tick(&mut self, w: &mut impl Write, now: u64, arrival: i64) -> Result<()> {
        if now >= self.media_due {
            let pts = self.media_clock.timestamp(1024, arrival)?;
            let data = self.media[self.media_index].clone();
            self.packet(w, 0, pts, data, arrival)?;
            self.media_index = (self.media_index + 1) % self.media.len();
            self.media_samples += 1024;
            self.media_due = self.media_base
                + self.media_samples / 48000 * 1_000_000_000
                + self.media_samples % 48000 * 1_000_000_000 / 48000;
            if now.saturating_sub(self.media_due) > 100_000_000 {
                self.media_base = now + 21_333_333;
                self.media_samples = 0;
                self.media_due = self.media_base;
            }
        }
        if now >= self.voice_due {
            if !self.voice_active {
                self.voice_active = true;
                self.voice_frame = 0;
                self.voice_packet = 0;
                self.voice_start = now;
                self.voice_clock.as_mut().unwrap().reset();
                let v = self.voice.as_ref().unwrap();
                oxp_wire::write(
                    w,
                    &Message::audio_format(
                        1,
                        Some(&AudioFormat {
                            role: AudioRole::Voice,
                            codec: AudioCodec::PcmS16Le,
                            sample_rate: v.rate,
                            channels: v.channels,
                            config: None,
                        }),
                    )?,
                )?;
                if self.every.is_some() {
                    self.microphone(w, true)?;
                }
            }
            let v = self.voice.as_ref().unwrap();
            if self.voice_frame == v.frames() && self.every.is_some() {
                self.voice_active = false;
                oxp_wire::write(w, &Message::audio_format(1, None)?)?;
                self.microphone(w, false)?;
                self.voice_due =
                    (self.voice_start + (self.every.unwrap() * 1e9) as u64).max(now + 20_000_000);
            } else {
                let n = ((self.voice_packet + 1) * v.rate as u64 / 50
                    - self.voice_packet * v.rate as u64 / 50) as usize;
                let frame_bytes = v.channels as usize * 2;
                let mut data = vec![0; n * frame_bytes];
                let mut copied = 0;
                while copied < n {
                    let count = (n - copied).min(v.frames() - self.voice_frame);
                    data[copied * frame_bytes..(copied + count) * frame_bytes].copy_from_slice(
                        &v.pcm[self.voice_frame * frame_bytes
                            ..(self.voice_frame + count) * frame_bytes],
                    );
                    copied += count;
                    self.voice_frame += count;
                    if self.voice_frame == v.frames() {
                        if self.every.is_some() {
                            break;
                        }
                        self.voice_frame = 0;
                    }
                }
                let pts = self
                    .voice_clock
                    .as_mut()
                    .unwrap()
                    .timestamp(n as u32, arrival)?;
                self.packet(w, 1, pts, data, arrival)?;
                self.voice_packet += 1;
                self.voice_due = self.voice_start + self.voice_packet * 20_000_000;
                if now.saturating_sub(self.voice_due) > 100_000_000 {
                    self.voice_start = now + 20_000_000 - self.voice_packet * 20_000_000;
                    self.voice_due = now + 20_000_000;
                }
            }
        }
        if now >= self.summary_due {
            self.summaries();
            self.summary_due = now + 1_000_000_000;
        }
        Ok(())
    }
    pub fn accepts(&self, stream: u8, data: &[u8]) -> bool {
        self.mic_rate.is_some_and(|rate| {
            self.mic_active
                && stream == 1
                && data.len().is_multiple_of(2)
                && (rate / 50..=rate.div_ceil(50)).contains(&(data.len() as u32 / 2))
        })
    }
    pub fn receive(&mut self, stream: u8, data: &[u8], output: &str, client: &str) -> Result<()> {
        if !self.accepts(stream, data) {
            return Err("Invalid microphone packet".into());
        }
        if let Some(w) = &mut self.writer {
            w.write(data)?;
        }
        let t = self
            .totals
            .entry((output.into(), client.into()))
            .or_default();
        t.packets += 1;
        for b in data.chunks_exact(2) {
            let v = i16::from_le_bytes([b[0], b[1]]) as f64 / 32768.0;
            t.squares += v * v;
            t.samples += 1;
        }
        Ok(())
    }
    fn drain_summaries(&mut self) -> Vec<serde_json::Value> {
        std::mem::take(&mut self.totals).into_iter().map(|((output,client),t)| {
            let db=if t.squares==0.0 {None} else {Some(10.0*(t.squares/t.samples as f64).log10())};
            json!({"type":"mic-stats","stream":1,"packets":t.packets,"origin":{"outputId":output,"clientId":client},"rmsDbfs":db})
        }).collect()
    }
    fn summaries(&mut self) {
        for v in self.drain_summaries() {
            if self.stdio {
                event(v);
            } else {
                println!("{v}");
            }
        }
    }
}
impl Drop for ReplayAudio {
    fn drop(&mut self) {
        self.summaries();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn options(values: &[(&str, &str)]) -> Args {
        Args(
            values
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }
    fn messages(b: &[u8]) -> Vec<Message> {
        let mut input = b;
        let mut out = Vec::new();
        while let Some(m) = oxp_wire::read(&mut input).unwrap() {
            out.push(m);
        }
        out
    }
    #[test]
    fn wav_validation_voice_turns_and_microphone_recording() {
        let path =
            std::env::temp_dir().join(format!("oxp-replay-voice-{}.wav", std::process::id()));
        let out = path.with_file_name(format!("oxp-replay-mic-{}.wav", std::process::id()));
        {
            let mut w = WavWriter::new(path.to_str().unwrap(), 16000).unwrap();
            w.write(&vec![4; 960]).unwrap();
        }
        let b = std::fs::read(&path).unwrap();
        let clip = WavClip::parse(&b).unwrap();
        assert_eq!(clip.frames(), 480);
        let mut bad = b.clone();
        bad[34] = 8;
        assert!(WavClip::parse(&bad).is_err());
        assert!(WavClip::parse(&b[..b.len() - 1]).is_err());
        let args = options(&[
            ("--audio-voice", path.to_str().unwrap()),
            ("--audio-voice-every", "1"),
            ("--mic-rate", "16000"),
            ("--mic-out", out.to_str().unwrap()),
        ]);
        let mut a = ReplayAudio::new(&args).unwrap();
        let mut wire = Vec::new();
        a.start(&mut wire).unwrap();
        assert!(wire.is_empty());
        a.tick(&mut wire, 0, 5_000_000).unwrap();
        assert!(a.mic_active);
        a.receive(1, &vec![0; 640], "web", "one").unwrap();
        assert!(a.receive(2, &vec![0; 640], "web", "one").is_err());
        a.tick(&mut wire, 20_000_000, 5_020_000).unwrap();
        a.tick(&mut wire, 40_000_000, 5_040_000).unwrap();
        assert!(!a.mic_active);
        let m = messages(&wire);
        assert_eq!(m.len(), 6);
        let audio = m
            .iter()
            .filter_map(|m| {
                if let Message::Binary { header, payload } = m {
                    Some((header, payload))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(audio.len(), 2);
        assert_eq!(audio[1].0.pts_us - audio[0].0.pts_us, 20000);
        assert_eq!(audio[1].1.len(), 640);
        assert_eq!(&audio[1].1[320..], vec![0; 320]);
        assert_eq!(a.next_deadline(), 1_000_000_000);
        drop(a);
        let clip = WavClip::parse(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!(clip.pcm, vec![0; 640]);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(out).unwrap();
        assert!(ReplayAudio::new(&options(&[("--audio-voice-every", "NaN")])).is_err());
        assert!(ReplayAudio::new(&options(&[("--mic-out", "unused")])).is_err());
    }
    #[test]
    fn microphone_summaries_are_per_origin_with_silence_null() {
        let mut a = ReplayAudio::new(&options(&[("--mic-rate", "16000")])).unwrap();
        a.start(&mut Vec::new()).unwrap();
        let half = (0..320)
            .flat_map(|_| 16384i16.to_le_bytes())
            .collect::<Vec<_>>();
        a.receive(1, &half, "web", "one").unwrap();
        a.receive(1, &half, "web", "one").unwrap();
        a.receive(1, &vec![0; 640], "clink", "two").unwrap();
        let summaries = a.drain_summaries();
        assert_eq!(summaries.len(), 2);
        let silent = summaries
            .iter()
            .find(|v| v["origin"]["clientId"] == "two")
            .unwrap();
        assert!(silent["rmsDbfs"].is_null());
        let half = summaries
            .iter()
            .find(|v| v["origin"]["clientId"] == "one")
            .unwrap();
        assert_eq!(half["packets"], 2);
        assert!((half["rmsDbfs"].as_f64().unwrap() + 6.020599913).abs() < 1e-6);
        assert!(a.drain_summaries().is_empty());
    }
    #[test]
    fn normalized_media_loops_and_stalls_do_not_burst() {
        let path =
            std::env::temp_dir().join(format!("oxp-replay-media-{}.aac", std::process::id()));
        let c = oxp_media::AudioSpecificConfig::new(2, 48000, 2).unwrap();
        let mut b = oxp_media::Adts::build_header(&c, 3).unwrap().to_vec();
        b.extend([1, 2, 3]);
        std::fs::write(&path, b).unwrap();
        let mut a =
            ReplayAudio::new(&options(&[("--audio-media", path.to_str().unwrap())])).unwrap();
        let mut wire = Vec::new();
        a.start(&mut wire).unwrap();
        a.tick(&mut wire, 0, 0).unwrap();
        a.tick(&mut wire, 1_000_000_000, 1_000_000).unwrap();
        assert_eq!(a.next_deadline(), 1_021_333_333);
        let m = messages(&wire);
        assert_eq!(m.len(), 3);
        assert!(
            matches!(&m[2],Message::Binary {header,payload} if header.pts_us==1_000_000 && payload==&[1,2,3])
        );
        std::fs::remove_file(path).unwrap();
    }
}
