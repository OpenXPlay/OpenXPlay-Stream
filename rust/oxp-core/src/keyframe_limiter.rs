// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use crate::model::ControlEvent;
use std::collections::BTreeMap;
/// Pure time-driven coalescer. The hub arranges wakeups even when media is idle.
pub struct KeyframeLimiter {
    interval: i64,
    slots: BTreeMap<u32, (i64, Option<ControlEvent>)>,
}
impl Default for KeyframeLimiter {
    fn default() -> Self {
        Self::new(250_000_000)
    }
}
impl KeyframeLimiter {
    pub fn new(interval: i64) -> Self {
        assert!(interval > 0);
        Self {
            interval,
            slots: BTreeMap::new(),
        }
    }
    pub fn clear(&mut self) {
        self.slots.clear();
    }
    pub fn next_deadline_nanos(&self) -> Option<i64> {
        self.slots
            .values()
            .filter(|(_, p)| p.is_some())
            .map(|(s, _)| s.saturating_add(self.interval))
            .min()
    }
    pub fn poll(&mut self, now: i64) -> Vec<ControlEvent> {
        let mut due = Vec::new();
        for (sent, pending) in self.slots.values_mut() {
            if pending.is_some() && now.saturating_sub(*sent) >= self.interval {
                *sent = now;
                due.push(pending.take().unwrap());
            }
        }
        due
    }
    pub fn request(&mut self, event: ControlEvent, now: i64) -> Vec<ControlEvent> {
        let ControlEvent::KeyFrameRequest { screen, .. } = event else {
            panic!("Expected keyframe request")
        };
        let mut due = self.poll(now);
        if self
            .slots
            .get(&screen)
            .is_none_or(|(s, _)| now.saturating_sub(*s) >= self.interval)
        {
            self.slots.insert(screen, (now, None));
            due.push(event);
        } else {
            self.slots.get_mut(&screen).unwrap().1 = Some(event);
        }
        due
    }
}
