// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use std::{
    collections::{BTreeMap, VecDeque},
    net::IpAddr,
    time::{Duration, Instant},
};

const WINDOW: Duration = Duration::from_secs(60);
#[derive(Default)]
struct Attempts {
    failures: VecDeque<Instant>,
    blocked_until: Option<Instant>,
}
#[derive(Default)]
pub(crate) struct HelloRateLimit {
    addresses: BTreeMap<IpAddr, Attempts>,
}
impl HelloRateLimit {
    pub(crate) fn allow(
        &mut self,
        address: IpAddr,
        now: Instant,
        verify: impl FnOnce() -> bool,
    ) -> bool {
        self.addresses.retain(|_, attempts| {
            while attempts
                .failures
                .front()
                .is_some_and(|t| now.duration_since(*t) >= WINDOW)
            {
                attempts.failures.pop_front();
            }
            attempts.blocked_until.is_some_and(|t| now < t) || !attempts.failures.is_empty()
        });
        if self
            .addresses
            .get(&address)
            .is_some_and(|a| a.blocked_until.is_some_and(|t| now < t))
        {
            return false;
        }
        if verify() {
            return true;
        }
        let attempts = self.addresses.entry(address).or_default();
        attempts.failures.push_back(now);
        if attempts.failures.len() >= 5 {
            attempts.blocked_until = Some(now + WINDOW);
        }
        false
    }
}

/// Compare every byte, independent of the matching prefix. Lengths are public input sizes.
pub(crate) fn constant_time_token(expected: &str, supplied: &str) -> bool {
    let a = expected.as_bytes();
    let b = supplied.as_bytes();
    let mut difference = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        difference |= usize::from(a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0));
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rolling_window_cooldown_and_address_isolation() {
        let mut limit = HelloRateLimit::default();
        let now = Instant::now();
        let a = "127.0.0.1".parse().unwrap();
        let b = "127.0.0.2".parse().unwrap();
        for i in 0..4 {
            assert!(!limit.allow(a, now + Duration::from_secs(i), || false));
        }
        assert!(limit.allow(a, now + Duration::from_secs(4), || true));
        assert!(!limit.allow(a, now + Duration::from_secs(10), || false));
        assert!(!limit.allow(a, now + Duration::from_secs(69), || panic!(
            "Blocked request verified"
        )));
        assert!(limit.allow(b, now + Duration::from_secs(69), || true));
        assert!(limit.allow(a, now + Duration::from_secs(70), || true));
        for i in 71..75 {
            assert!(!limit.allow(a, now + Duration::from_secs(i), || false));
        }
        assert!(limit.allow(a, now + Duration::from_secs(75), || true));
    }
    #[test]
    fn failures_expire_at_boundary_and_blocked_attempts_do_not_extend_cooldown() {
        let mut limit = HelloRateLimit::default();
        let now = Instant::now();
        let a = "::1".parse().unwrap();
        for _ in 0..4 {
            assert!(!limit.allow(a, now, || false));
        }
        assert!(!limit.allow(a, now + WINDOW, || false));
        assert!(limit.allow(a, now + WINDOW, || true));
        for _ in 0..4 {
            assert!(!limit.allow(a, now + WINDOW, || false));
        }
        assert!(!limit.allow(a, now + Duration::from_secs(119), || true));
        assert!(limit.allow(a, now + WINDOW * 2, || true));
    }
    #[test]
    fn exact_token_comparison() {
        assert!(constant_time_token("00123456", "00123456"));
        for supplied in [
            "",
            "0012345",
            "001234560",
            "10123456",
            "00123457",
            "00123456\0",
        ] {
            assert!(!constant_time_token("00123456", supplied));
        }
    }
}
