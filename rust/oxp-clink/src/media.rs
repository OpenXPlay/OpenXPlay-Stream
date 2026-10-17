// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use crate::{require, Error, Result};
use std::{
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};
const TS_BYTES: usize = 188;

/// The output of one muxer call as RFC 4571 framed RTP packets (two-byte length, RTP header, up to
/// seven TS packets) in one reusable buffer, so packetizing allocates nothing per packet.
///
/// Bounded: a call producing more than `max_bytes` marks the unit overflowed and appends nothing
/// more. On 2026-10-15 one muxer call produced hundreds of thousands of TS packets (a timestamp jump
/// of hours filled by PCR packets); retaining all of them, then a second copy as RTP packets,
/// exhausted the heap.
pub struct RtpUnit {
    bytes: Vec<u8>,
    max_bytes: usize,
    packets: usize,
    overflowed: bool,
    first_sequence: u16,
    timestamp: u32,
    ssrc: u32,
    /// Offset of the open packet's length prefix.
    open: usize,
    /// TS packets in the open packet; zero before the first.
    grouped: usize,
}
impl Default for RtpUnit {
    fn default() -> Self {
        Self::new(Self::MAX_BYTES).unwrap()
    }
}
impl RtpUnit {
    /// Two-byte RFC 4571 length plus the 12-byte RTP header.
    pub const HEADER_BYTES: usize = 14;
    pub const FRAMED_PACKET_BYTES: usize = Self::HEADER_BYTES + 7 * TS_BYTES;
    /// Twice the largest legitimate unit: a 4 MiB video frame (VideoQueue's limit) is about 4.4 MB framed.
    pub const MAX_BYTES: usize = 8 * 1024 * 1024;
    const INITIAL_BYTES: usize = 64 * 1024;
    const RETAINED_BYTES: usize = 1024 * 1024;
    pub fn new(max_bytes: usize) -> Result<Self> {
        require(
            max_bytes >= Self::FRAMED_PACKET_BYTES,
            "invalid_rtp_unit_limit",
        )?;
        Ok(Self {
            bytes: Vec::with_capacity(Self::INITIAL_BYTES.min(max_bytes)),
            max_bytes,
            packets: 0,
            overflowed: false,
            first_sequence: 0,
            timestamp: 0,
            ssrc: 0,
            open: 0,
            grouped: 0,
        })
    }
    pub fn clear(&mut self, timestamp: u32, first_sequence: u16, ssrc: u32) {
        // A rare huge unit (a large IDR) must not pin its buffer for the rest of the session.
        if self.bytes.capacity() > Self::RETAINED_BYTES {
            self.bytes = Vec::with_capacity(Self::INITIAL_BYTES.min(self.max_bytes));
        }
        self.bytes.clear();
        self.packets = 0;
        self.overflowed = false;
        self.grouped = 0;
        self.timestamp = timestamp;
        self.first_sequence = first_sequence;
        self.ssrc = ssrc;
    }
    pub fn append(&mut self, ts: &[u8; TS_BYTES]) {
        if self.overflowed {
            return;
        }
        let opening = self.grouped == 0 || self.grouped == 7;
        let needed = self.bytes.len() + TS_BYTES + if opening { Self::HEADER_BYTES } else { 0 };
        if needed > self.max_bytes {
            self.overflowed = true;
            return;
        }
        if needed > self.bytes.capacity() {
            self.grow(needed);
        }
        if opening {
            self.open = self.bytes.len();
            self.grouped = 0;
            let sequence = self.first_sequence.wrapping_add(self.packets as u16);
            self.bytes.extend_from_slice(&[0, 0, 0x80, 33]);
            self.bytes.extend_from_slice(&sequence.to_be_bytes());
            self.bytes.extend_from_slice(&self.timestamp.to_be_bytes());
            self.bytes.extend_from_slice(&self.ssrc.to_be_bytes());
            self.packets += 1;
        }
        self.bytes.extend_from_slice(ts);
        self.grouped += 1;
        let length = (12 + self.grouped * TS_BYTES) as u16;
        self.bytes[self.open..self.open + 2].copy_from_slice(&length.to_be_bytes());
    }
    fn grow(&mut self, needed: usize) {
        let mut capacity = self.bytes.capacity().max(TS_BYTES);
        while capacity < needed {
            capacity = capacity.saturating_mul(2);
        }
        self.bytes
            .reserve_exact(capacity.min(self.max_bytes) - self.bytes.len());
    }
    /// Each framed packet, length prefix included, in order.
    pub fn framed(&self) -> impl Iterator<Item = &[u8]> {
        let mut rest = self.bytes.as_slice();
        std::iter::from_fn(move || {
            let length = u16::from_be_bytes([*rest.first()?, rest[1]]) as usize + 2;
            let (packet, tail) = rest.split_at(length);
            rest = tail;
            Some(packet)
        })
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn len(&self) -> usize {
        self.bytes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
    /// Bytes allocated for the buffer, never more than [Self::max_bytes].
    pub fn capacity(&self) -> usize {
        self.bytes.capacity()
    }
    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }
    pub fn packet_count(&self) -> usize {
        self.packets
    }
    pub fn overflowed(&self) -> bool {
        self.overflowed
    }
    pub fn first_sequence(&self) -> u16 {
        self.first_sequence
    }
}

/// Thread-confined MPEG-TS RTP packetizer. The final group may contain fewer than seven packets.
pub struct RtpPacketizer {
    pub ssrc: u32,
    next_sequence: u16,
}
impl Default for RtpPacketizer {
    fn default() -> Self {
        Self::new(0x434c4144, 0)
    }
}
impl RtpPacketizer {
    pub fn new(ssrc: u32, sequence: u16) -> Self {
        Self {
            ssrc,
            next_sequence: sequence,
        }
    }
    pub fn packetize(&mut self, ts: &[Vec<u8>], pts_us: i64) -> Result<Vec<Vec<u8>>> {
        require(
            ts.iter().all(|p| p.len() == TS_BYTES && p[0] == 0x47),
            "invalid_ts_packet",
        )?;
        let mut unit = RtpUnit::new(usize::MAX)?;
        self.start(&mut unit, pts_us)?;
        for p in ts {
            unit.append(p.as_slice().try_into()?);
        }
        require(self.finish(&unit), "rtp_unit_overflow")?;
        Ok(unit.framed().map(|p| p[2..].to_vec()).collect())
    }
    /// Starts one muxer call in `unit`; its RTP packets share the timestamp of `pts_us`.
    pub fn start(&self, unit: &mut RtpUnit, pts_us: i64) -> Result<()> {
        let timestamp = oxp_media::ts::to_90khz(pts_us)? as u32;
        unit.clear(timestamp, self.next_sequence, self.ssrc);
        Ok(())
    }
    /// Commits the sequence numbers of a complete unit. False when the unit exceeded its bound: it
    /// must then be discarded, and the sequence stays where it was so no RTP gap is created.
    pub fn finish(&mut self, unit: &RtpUnit) -> bool {
        if unit.overflowed {
            return false;
        }
        self.next_sequence = unit.first_sequence.wrapping_add(unit.packets as u16);
        true
    }
    pub fn frame(packet: &[u8]) -> Result<Vec<u8>> {
        require((12..=65535).contains(&packet.len()), "invalid_rtp_packet")?;
        let mut out = (packet.len() as u16).to_be_bytes().to_vec();
        out.extend_from_slice(packet);
        Ok(out)
    }
}

/// Writes one framed packet; false declines the rest of the unit (the session is not playing).
pub type RtpWrite = Arc<dyn Fn(&[u8]) -> Result<bool> + Send + Sync>;

/// Hands one [RtpUnit] at a time from the media worker to a dedicated socket writer, so the media
/// worker waits at most `wait` for a stalled TCP send instead of blocking until the session
/// watchdog closes the socket. While a late write is in flight the media worker leaves media in its
/// bounded queues, which drop to the next keyframe (video) or drop oldest (audio). Only the unit in
/// flight is ever retained here. The writer's own wait stays bounded by the session watchdog.
pub struct RtpSender {
    wait: Duration,
    state: Mutex<SenderState>,
    changed: Condvar,
}
#[derive(Default)]
struct SenderState {
    job: Option<(RtpUnit, RtpWrite)>,
    writing: bool,
    /// A unit the writer finished and the media worker has not taken back.
    done: Option<RtpUnit>,
    retained_bytes: usize,
    submitted_at: Option<Instant>,
    late: bool,
    recovered: Option<Duration>,
    failure: Option<Error>,
}
impl Default for RtpSender {
    fn default() -> Self {
        Self::new(Self::DEFAULT_WAIT).unwrap()
    }
}
impl RtpSender {
    /// Longer than one large IDR takes on a slow link, and between VideoQueue's 600 ms and the audio
    /// queue's 1.5 s, so short Wi-Fi dips behave exactly as a blocking send did.
    pub const DEFAULT_WAIT: Duration = Duration::from_secs(1);
    const IDLE_POLL: Duration = Duration::from_millis(100);
    pub fn new(wait: Duration) -> Result<Self> {
        require(!wait.is_zero(), "invalid_rtp_wait")?;
        Ok(Self {
            wait,
            state: Mutex::new(SenderState::default()),
            changed: Condvar::new(),
        })
    }
    pub fn wait(&self) -> Duration {
        self.wait
    }
    /// Media worker: hands a filled unit to the writer and waits up to `wait`. The unit comes back
    /// once written (or once `write` declined part of it, as before); None while the write is still
    /// in flight, until [Self::reclaim] returns it. Surfaces a writer failure.
    pub fn submit(&self, unit: RtpUnit, write: RtpWrite) -> Result<Option<RtpUnit>> {
        let mut state = self.state.lock().unwrap();
        require(
            state.job.is_none() && !state.writing && state.done.is_none(),
            "rtp_write_in_flight",
        )?;
        if unit.is_empty() {
            return Ok(Some(unit));
        }
        let submitted = Instant::now();
        state.retained_bytes = unit.capacity();
        state.job = Some((unit, write));
        state.submitted_at = Some(submitted);
        self.changed.notify_all();
        let deadline = submitted + self.wait;
        while state.done.is_none() {
            let now = Instant::now();
            if now >= deadline {
                state.late = true;
                return Ok(None);
            }
            state = self.changed.wait_timeout(state, deadline - now).unwrap().0;
        }
        if let Some(e) = state.failure.take() {
            return Err(e);
        }
        state.retained_bytes = 0;
        Ok(state.done.take())
    }
    /// Media worker: the unit of a late write once it has finished; surfaces a writer failure.
    pub fn reclaim(&self) -> Result<Option<RtpUnit>> {
        let mut state = self.state.lock().unwrap();
        if let Some(e) = state.failure.take() {
            return Err(e);
        }
        let unit = state.done.take();
        if unit.is_some() {
            state.retained_bytes = 0;
        }
        Ok(unit)
    }
    /// Media worker: once a late write has finished, how long it took in total.
    pub fn take_recovered(&self) -> Option<Duration> {
        self.state.lock().unwrap().recovered.take()
    }
    /// Snapshot for diagnostics and tests: buffer bytes held here, and whether a write is in flight.
    pub fn retained(&self) -> (usize, bool) {
        let state = self.state.lock().unwrap();
        (state.retained_bytes, state.job.is_some() || state.writing)
    }
    /// Writer thread: writes each submitted unit, one framed packet per write, until `active` is false.
    pub fn run(&self, active: impl Fn() -> bool) {
        let mut state = self.state.lock().unwrap();
        loop {
            let Some((unit, write)) = state.job.take() else {
                if !active() {
                    return;
                }
                state = self.changed.wait_timeout(state, Self::IDLE_POLL).unwrap().0;
                continue;
            };
            state.writing = true;
            drop(state);
            let mut result = Ok(());
            for packet in unit.framed() {
                match write(packet) {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(e) => {
                        result = Err(e);
                        break;
                    }
                }
            }
            drop(write);
            state = self.state.lock().unwrap();
            state.writing = false;
            state.done = Some(unit);
            if let Err(e) = result {
                state.failure.get_or_insert(e);
            }
            if std::mem::take(&mut state.late) {
                state.recovered = state.submitted_at.map(|t| t.elapsed());
            }
            self.changed.notify_all();
        }
    }
}
