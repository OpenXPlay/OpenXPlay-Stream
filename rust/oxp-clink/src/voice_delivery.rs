// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14.

use crate::voice_buffer::{VoicePacer, VoiceStream};
use crate::{model::*, session::ClinkSession, Result};
use oxp_core::audio::PcmMixer;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
pub(crate) fn voice_role(role: AudioRole) -> bool {
    matches!(role, AudioRole::Voice | AudioRole::Telephony)
}
enum Command {
    Format(u32, Option<Arc<AudioFormat>>),
    Packet(AudioPacket, Instant),
    Reset,
}
pub(crate) struct VoiceDelivery {
    commands: Mutex<(VecDeque<Command>, usize)>,
    origin: Instant,
}
impl Default for VoiceDelivery {
    fn default() -> Self {
        Self {
            commands: Mutex::default(),
            origin: Instant::now(),
        }
    }
}
impl VoiceDelivery {
    pub fn format(&self, stream: u32, format: Option<Arc<AudioFormat>>) {
        self.commands
            .lock()
            .unwrap()
            .0
            .push_back(Command::Format(stream, format));
    }
    pub fn packet(&self, packet: AudioPacket) {
        if packet.data.len() > 192_000 {
            return;
        }
        let mut q = self.commands.lock().unwrap();
        while q.1 + packet.data.len() > 192_000 {
            let Some(i) = q.0.iter().position(|c| matches!(c, Command::Packet(..))) else {
                break;
            };
            if let Some(Command::Packet(p, _)) = q.0.remove(i) {
                q.1 -= p.data.len();
            }
        }
        q.1 += packet.data.len();
        q.0.push_back(Command::Packet(packet, Instant::now()));
    }
    pub fn reset(&self) {
        let mut q = self.commands.lock().unwrap();
        q.0.clear();
        q.1 = 0;
        q.0.push_back(Command::Reset);
    }
    pub fn run(
        &self,
        active: impl Fn() -> bool,
        session: impl Fn() -> Option<Arc<ClinkSession>>,
        valid: impl Fn(&AudioPacket) -> bool,
    ) -> Result<()> {
        let mut mixer = PcmMixer::default();
        let mut formats = BTreeMap::<u32, Arc<AudioFormat>>::new();
        let mut buffers = BTreeMap::<u32, VoiceStream>::new();
        let mut ended_mixer = BTreeSet::new();
        let origin = self.origin;
        let mut pacer = VoicePacer::default();
        let mut tail = VecDeque::new();
        let mut started: Option<Arc<ClinkSession>> = None;
        let mut has_data = false;
        let mut next = Instant::now();
        let mut epoch = next;
        let mut frames = 0u64;
        while active() {
            let owner = session();
            if started.as_ref().is_some_and(|s| {
                !s.is_playing() || owner.as_ref().is_none_or(|o| !Arc::ptr_eq(o, s))
            }) {
                tail.clear();
                started = None;
                mixer = PcmMixer::default();
                formats.clear();
                buffers.clear();
                ended_mixer.clear();
                has_data = false;
            }
            loop {
                let command = {
                    let mut q = self.commands.lock().unwrap();
                    if let Some(Command::Format(stream, format)) = q.0.front() {
                        let stream = *stream;
                        let replacing = format.is_some();
                        if let Some(buffer) = buffers.get_mut(&stream) {
                            buffer.end()?;
                            if buffer.drained() && ended_mixer.insert(stream) {
                                mixer.end_stream(stream)?;
                            }
                        }
                        // Keep the replacement marker and subsequent packets queued until the old PCM drains.
                        if replacing
                            && (buffers.get(&stream).is_some_and(|b| !b.drained())
                                || !mixer.stream_drained(stream))
                        {
                            break;
                        }
                    }
                    let Some(command) = q.0.pop_front() else {
                        break;
                    };
                    if let Command::Packet(p, _) = &command {
                        q.1 -= p.data.len();
                    }
                    command
                };
                match command {
                    Command::Reset => {
                        finish(&active, &mut started, &mut tail, epoch, frames)?;
                        mixer = PcmMixer::default();
                        formats.clear();
                        buffers.clear();
                        ended_mixer.clear();
                        has_data = false;
                        frames = 0;
                    }
                    Command::Format(stream, f) => {
                        formats.remove(&stream);
                        if let Some(f) = f.filter(|f| {
                            voice_role(f.role)
                                && f.codec == AudioCodec::PcmS16le
                                && (8000..=48000).contains(&f.sample_rate)
                                && (1..=2).contains(&f.channels)
                        }) {
                            mixer.remove_stream(stream);
                            mixer.add_stream(stream, 16000, 1)?;
                            buffers.insert(stream, VoiceStream::new(f.sample_rate, f.channels)?);
                            ended_mixer.remove(&stream);
                            formats.insert(stream, f);
                        }
                    }
                    Command::Packet(p, arrival) => {
                        if valid(&p) && formats.contains_key(&p.stream) {
                            if let Some(buffer) = buffers.get_mut(&p.stream) {
                                buffer.push(
                                    &p.data,
                                    arrival.saturating_duration_since(origin).as_micros() as u64,
                                )?;
                                has_data = true;
                            }
                        }
                    }
                }
            }
            let Some(owner) = owner.filter(|s| s.audio_ready()) else {
                thread::sleep(Duration::from_millis(5));
                continue;
            };
            if Instant::now() < next || (!has_data && started.is_none()) {
                thread::sleep(Duration::from_millis(2));
                continue;
            }
            if started.is_none() && !buffers.values().any(|b| b.ready()) {
                thread::sleep(Duration::from_millis(2));
                continue;
            }
            let Some(frame) = read_frame(&mut mixer, &mut buffers, &mut ended_mixer, origin)?
            else {
                finish(&active, &mut started, &mut tail, epoch, frames)?;
                has_data = false;
                thread::sleep(Duration::from_millis(2));
                continue;
            };
            if started.is_none() {
                owner.voice_state(true)?;
                started = Some(owner.clone());
                epoch = Instant::now();
                frames = 0;
                pacer.reset(epoch.duration_since(origin).as_micros() as u64);
                tail.push_back(frame);
                frames += 1;
                // Prime 60 ms immediately, retaining one frame for the acknowledged tail.
                for _ in 0..3 {
                    if let Some(frame) =
                        read_frame(&mut mixer, &mut buffers, &mut ended_mixer, origin)?
                    {
                        tail.push_back(frame);
                        frames += 1;
                    }
                }
                if tail.len() > 1 {
                    let data: Vec<u8> = tail.drain(..tail.len() - 1).flatten().collect();
                    owner.send_voice(&data, false)?;
                }
            } else {
                tail.push_back(frame);
                frames += 1;
                if tail.len() > 1 {
                    owner.send_voice(&tail.pop_front().unwrap(), false)?;
                }
            }
            next = origin + Duration::from_micros(pacer.next(origin.elapsed().as_micros() as u64));
        }
        Ok(())
    }
}
fn read_frame(
    mixer: &mut PcmMixer,
    buffers: &mut BTreeMap<u32, VoiceStream>,
    ended: &mut BTreeSet<u32>,
    origin: Instant,
) -> Result<Option<Vec<u8>>> {
    for (&stream, buffer) in buffers {
        if let Some(data) = buffer.read(origin.elapsed().as_micros() as u64) {
            mixer.feed(stream, &data)?;
        }
        if buffer.drained() && ended.insert(stream) {
            mixer.end_stream(stream)?;
        }
    }
    Ok(mixer.read_frame())
}

fn finish(
    active: &impl Fn() -> bool,
    started: &mut Option<Arc<ClinkSession>>,
    tail: &mut VecDeque<Vec<u8>>,
    epoch: Instant,
    frames: u64,
) -> Result<()> {
    if let Some(owner) = started.take().filter(|s| s.is_playing()) {
        if !tail.is_empty() {
            let data = tail.iter().flatten().copied().collect::<Vec<_>>();
            owner.send_voice(&data, true)?;
            let end = (epoch + Duration::from_millis(frames * 20))
                .max(Instant::now() + Duration::from_millis(tail.len() as u64 * 20));
            while active() && owner.is_playing() && Instant::now() < end {
                thread::sleep(Duration::from_millis(2));
            }
        }
        if owner.is_playing() {
            owner.voice_state(false)?;
        }
    }
    tail.clear();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[test]
    fn empty_format_replacement_does_not_wait_for_playback() {
        let voice = VoiceDelivery::default();
        let format = Arc::new(AudioFormat {
            stream: 1,
            role: AudioRole::Voice,
            codec: AudioCodec::PcmS16le,
            sample_rate: 16000,
            channels: 1,
            config: None,
        });
        voice.format(1, Some(format.clone()));
        voice.format(1, Some(format));
        voice.format(1, None);
        let turns = AtomicUsize::new(0);
        voice
            .run(
                || turns.fetch_add(1, Ordering::Relaxed) < 2,
                || None,
                |_| true,
            )
            .unwrap();
        assert!(voice.commands.lock().unwrap().0.is_empty());
    }
}
