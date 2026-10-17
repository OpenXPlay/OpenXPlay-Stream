// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

//! Time bookkeeping shared by transcoder implementations; free of I/O so it is unit-tested.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

/// Drops decoded frames so the encoder sees at most the target rate, keeping a steady cadence.
pub struct FramePacer {
    interval_us: i64,
    next_due: Option<i64>,
}
impl FramePacer {
    pub fn new(fps: u32) -> Self {
        Self {
            interval_us: 1_000_000 / fps.max(1) as i64,
            next_due: None,
        }
    }
    pub fn accept(&mut self, pts_us: i64) -> bool {
        // Tolerate capture jitter of an eighth of a frame.
        let slack = self.interval_us / 8;
        match self.next_due {
            Some(due)
                if pts_us.saturating_add(slack) < due && pts_us >= due - 4 * self.interval_us =>
            {
                false
            }
            Some(due) if pts_us.saturating_add(slack) >= due => {
                // After a gap, restart the cadence at this frame instead of bursting.
                self.next_due =
                    Some((due + self.interval_us).max(pts_us + self.interval_us - slack));
                true
            }
            _ => {
                // First frame, or a timestamp jump backwards.
                self.next_due = Some(pts_us + self.interval_us);
                true
            }
        }
    }
    pub fn reset(&mut self) {
        self.next_due = None;
    }
}

/// Detects a transcoder that stopped producing output while it still has work.
pub struct Watchdog {
    limit: Duration,
    last_progress: Instant,
    submitted_since_output: usize,
}
impl Watchdog {
    pub fn new(limit: Duration, now: Instant) -> Self {
        Self {
            limit,
            last_progress: now,
            submitted_since_output: 0,
        }
    }
    pub fn submitted(&mut self, now: Instant) {
        if self.submitted_since_output == 0 {
            self.last_progress = now;
        }
        self.submitted_since_output += 1;
    }
    pub fn output(&mut self, now: Instant) {
        self.submitted_since_output = 0;
        self.last_progress = now;
    }
    /// `blocked` reports input waiting for a transcoder that refuses it. A codec may hold a few
    /// frames for reordering, so a short silence with little input is not a stall.
    pub fn stalled(&self, blocked: bool, now: Instant) -> bool {
        let silent = now.duration_since(self.last_progress) >= self.limit;
        silent && (blocked || self.submitted_since_output >= 16)
    }
}

/// Maps the integer cookies passed through codec buffer timestamps back to presentation times,
/// so codecs never see negative or reordered values and nothing is allocated per frame.
pub struct TimestampRing {
    entries: VecDeque<(u64, i64)>,
    capacity: usize,
    next: u64,
}
impl TimestampRing {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(capacity),
            capacity,
            next: 0,
        }
    }
    /// Returns a fresh nonzero cookie for `pts_us`.
    pub fn insert(&mut self, pts_us: i64) -> u64 {
        self.next += 1;
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back((self.next, pts_us));
        self.next
    }
    pub fn get(&self, cookie: u64) -> Option<i64> {
        self.entries
            .iter()
            .find(|(c, _)| *c == cookie)
            .map(|(_, pts)| *pts)
    }
    /// Looks up and forgets `cookie` together with every older entry.
    pub fn take(&mut self, cookie: u64) -> Option<i64> {
        let i = self.entries.iter().position(|(c, _)| *c == cookie)?;
        let pts = self.entries[i].1;
        self.entries.drain(..=i);
        Some(pts)
    }
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pacer_halves_sixty_and_keeps_thirty() {
        let mut p = FramePacer::new(30);
        let accepted: Vec<_> = (0..12)
            .map(|i| i * 16_667)
            .filter(|pts| p.accept(*pts))
            .collect();
        assert_eq!(accepted, vec![0, 33_334, 66_668, 100_002, 133_336, 166_670]);
        let mut p = FramePacer::new(30);
        // Jittered 30 fps input passes untouched.
        for pts in [0, 31_000, 68_000, 99_000, 135_000, 166_000] {
            assert!(p.accept(pts), "{pts}");
        }
    }

    #[test]
    fn pacer_restarts_after_gaps_and_backward_jumps() {
        let mut p = FramePacer::new(30);
        assert!(p.accept(0));
        assert!(p.accept(5_000_000));
        assert!(!p.accept(5_010_000));
        assert!(p.accept(5_033_333));
        assert!(p.accept(1_000));
        assert!(!p.accept(2_000));
        p.reset();
        assert!(p.accept(2_000));
    }

    #[test]
    fn watchdog_needs_silence_with_pending_work() {
        let start = Instant::now();
        let limit = Duration::from_secs(5);
        let mut w = Watchdog::new(limit, start);
        assert!(!w.stalled(true, start + Duration::from_secs(4)));
        assert!(w.stalled(true, start + limit));
        w.output(start + limit);
        let later = start + Duration::from_secs(20);
        // A static screen: a few frames held by the codec and no new input.
        for _ in 0..3 {
            w.submitted(later);
        }
        assert!(!w.stalled(false, later + Duration::from_secs(60)));
        for _ in 0..13 {
            w.submitted(later);
        }
        assert!(!w.stalled(false, later + Duration::from_secs(4)));
        assert!(w.stalled(false, later + limit));
    }

    #[test]
    fn timestamp_cookies_round_trip_and_evict() {
        let mut ring = TimestampRing::new(3);
        let a = ring.insert(-40);
        let b = ring.insert(10);
        let c = ring.insert(20);
        assert!(a > 0 && a < b && b < c);
        assert_eq!(ring.get(b), Some(10));
        let d = ring.insert(30);
        assert_eq!(ring.get(a), None);
        assert_eq!(ring.take(c), Some(20));
        assert_eq!(ring.get(b), None);
        assert_eq!(ring.get(d), Some(30));
        ring.clear();
        assert_eq!(ring.take(d), None);
    }
}
