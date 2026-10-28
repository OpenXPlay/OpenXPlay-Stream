// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

#[cfg(any(target_os = "linux", test))]
use crate::Device;
#[cfg(target_os = "linux")]
use crate::LinkEvent;
#[cfg(any(target_os = "linux", test))]
use oxp_clink::Result;
use std::time::Duration;

#[derive(Debug)]
pub struct LinkBackoff {
    delay: Duration,
}
impl Default for LinkBackoff {
    fn default() -> Self {
        Self {
            delay: Duration::from_secs(1),
        }
    }
}
impl LinkBackoff {
    pub fn with_initial_delay(delay: Duration) -> Self {
        Self {
            delay: delay.clamp(Duration::from_secs(1), Duration::from_secs(15)),
        }
    }
    pub fn next_delay(&mut self) -> Duration {
        let delay = self.delay;
        self.delay = (delay * 2).min(Duration::from_secs(15));
        delay
    }
    pub fn succeeded(&mut self) {
        *self = Self::default();
    }
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn select(
    devices: Vec<Device>,
    address: Option<&str>,
    car_id: Option<&str>,
) -> Result<Device> {
    if let Some(car_id) = car_id.filter(|id| !id.is_empty()) {
        // Once selected, only this car may move to a new random address.
        return super::select_device(
            devices
                .into_iter()
                .filter(|d| d.advertisement.car_id == car_id)
                .collect(),
            None,
        );
    }
    super::select_device(devices, address)
}
#[cfg(any(target_os = "linux", test))]
pub(crate) fn discover<T>(
    attempts: u32,
    mut attempt: impl FnMut(u32, &mut Option<String>) -> Result<T>,
    mut wait: impl FnMut() -> Result<()>,
) -> Result<T> {
    let mut car_id = None;
    for number in 1..=attempts {
        match attempt(number, &mut car_id) {
            Ok(value) => return Ok(value),
            Err(error) => {
                if number == attempts
                    || super::diagnostics::code(error.as_ref()) == "shutdown_requested"
                {
                    return Err(error);
                }
                wait()?;
            }
        }
    }
    Err("invalid_link_configuration".into())
}
#[cfg(target_os = "linux")]
pub(crate) fn retry_event(delay: Duration) -> LinkEvent {
    LinkEvent::Trace {
        stage: "link_retry".into(),
        fields: [("delayMs".into(), delay.as_millis().to_string())].into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn android_backoff_caps_and_success_resets() {
        let mut backoff = LinkBackoff::default();
        assert_eq!(
            (0..7)
                .map(|_| backoff.next_delay().as_secs())
                .collect::<Vec<_>>(),
            [1, 2, 4, 8, 15, 15, 15]
        );
        backoff.succeeded();
        assert_eq!(backoff.next_delay(), Duration::from_secs(1));
    }
    #[test]
    fn every_retry_rescans_and_follows_only_the_selected_identity() {
        let mut scans = 0;
        let mut waits = 0;
        let device = discover(
            3,
            |_, car| {
                scans += 1;
                let mut target = super::super::tests::device(&format!("aa:bb:cc:dd:ee:0{scans}"));
                target.advertisement.car_id = "target".into();
                let mut other = target.clone();
                other.advertisement.car_id = "other".into();
                let selected = select(
                    vec![target, other],
                    Some("aa:bb:cc:dd:ee:01"),
                    car.as_deref(),
                )?;
                *car = Some(selected.advertisement.car_id.clone());
                if scans < 3 {
                    return Err("bluetooth_connect_failed".into());
                }
                Ok(selected)
            },
            || {
                waits += 1;
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(scans, 3);
        assert_eq!(waits, 2);
        assert_eq!(device.address, "aa:bb:cc:dd:ee:03");
        assert!(select(
            vec![super::super::tests::device("aa:bb:cc:dd:ee:04")],
            None,
            Some("target")
        )
        .is_err());
    }
    #[test]
    fn shutdown_does_not_consume_retries() {
        let mut attempts = 0;
        discover::<()>(
            3,
            |_, _| {
                attempts += 1;
                Err("shutdown_requested".into())
            },
            || panic!("shutdown waited"),
        )
        .unwrap_err();
        assert_eq!(attempts, 1);
    }
}
