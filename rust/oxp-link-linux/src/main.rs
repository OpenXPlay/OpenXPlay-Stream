// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

#![forbid(unsafe_code)]
use oxp_clink::Result;
use oxp_link_linux::{AddressType, Config, LinuxClinkLink};
#[cfg(target_os = "linux")]
use std::sync::Arc;
use std::time::Duration;

fn main() {
    if let Err(e) = run() {
        eprintln!("oxp-link: {e}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or("usage: oxp-link scan|connect|leave [--address MAC] [--adapter hci0] [--interface wlan0] [--address-type random|public] [--name NAME] [--model MODEL] [--id ID] [--scan-timeout SEC] [--handoff-timeout SEC] [--join-timeout SEC] [--p2p-reserved on|off]")?;
    let mut c = Config::default();
    let mut address = None;
    while let Some(flag) = args.next() {
        let value = args.next().ok_or("missing_option_value")?;
        match flag.as_str() {
            "--address" if command == "connect" => address = Some(value),
            "--adapter" => c.bluetooth_adapter = value,
            "--interface" => c.wifi_interface = value,
            "--p2p-reserved" => c.p2p_reserved_by_carplay = oxp_link_linux::p2p_reserved(&value)?,
            "--address-type" => {
                c.address_type = match value.as_str() {
                    "random" => AddressType::Random,
                    "public" => AddressType::Public,
                    _ => return Err("invalid_address_type".into()),
                }
            }
            "--name" => c.device_name = value,
            "--model" => c.device_model = value,
            "--id" => c.device_id = value,
            "--scan-timeout" => c.scan_timeout = Duration::from_secs(value.parse()?),
            "--connect-timeout" => c.connect_timeout = Duration::from_secs(value.parse()?),
            "--handoff-timeout" => c.handoff_timeout = Duration::from_secs(value.parse()?),
            "--join-timeout" => c.join_timeout = Duration::from_secs(value.parse()?),
            _ => return Err("unknown_option".into()),
        }
    }
    let link = LinuxClinkLink::new(c)?;
    match command.as_str() {
        "scan" => {
            let devices: Vec<_> = link.scan()?.iter().map(|d| serde_json::json!({"address":d.address,"name":d.advertisement.name,"car_id":d.advertisement.car_id,"model_id":d.advertisement.model_id})).collect();
            println!("{}", serde_json::to_string(&devices)?);
        }
        "connect" => connect(&link, address.as_deref())?,
        "leave" => link.leave()?,
        _ => return Err("unknown_subcommand".into()),
    }
    Ok(())
}
// The signal source owns the wait duration; tests inject events without sleeping.
#[cfg(any(target_os = "linux", test))]
fn wait_and_leave(
    mut signal: impl FnMut() -> bool,
    mut lost: impl FnMut() -> bool,
    mut leave: impl FnMut() -> Result<()>,
) -> Result<()> {
    let interrupted = loop {
        if signal() {
            break true;
        }
        if lost() {
            break false;
        }
    };
    let cleanup = leave();
    if interrupted {
        cleanup
    } else {
        match cleanup {
            Ok(()) => Err("head unit network link lost; temporary connection removed".into()),
            Err(e) => Err(format!("head unit network link lost; leave failed: {e}").into()),
        }
    }
}

#[cfg(target_os = "linux")]
fn connect(link: &LinuxClinkLink, address: Option<&str>) -> Result<()> {
    use oxp_link_linux::ShutdownSignals;
    use std::sync::atomic::{AtomicBool, Ordering};
    // Install before bootstrap so interruptions cannot bypass network cleanup.
    let signals = ShutdownSignals::install()?;
    let lost = Arc::new(AtomicBool::new(false));
    let monitor = lost.clone();
    let connected = link.connect_address(
        address,
        Arc::new(move |_| {
            monitor.store(true, Ordering::Release);
        }),
    );
    // Bootstrap operations have deadlines. A signal received during bootstrap is
    // honored once it unwinds, including any partially created network profile.
    if signals.requested() {
        return link.leave();
    }
    let a = match connected {
        Ok(a) => a,
        Err(e) => {
            return match link.leave() {
                Ok(()) => Err(e),
                Err(cleanup) => Err(format!("{e}; leave failed: {cleanup}").into()),
            };
        }
    };
    println!(
        "{}",
        serde_json::json!({"head_unit_ipv4":a.head_unit_ipv4.to_string(),"local_ipv4":a.local_ipv4.to_string(),"auth_port":a.auth_port,"head_unit_id":a.head_unit_id})
    );
    wait_and_leave(
        || {
            if !signals.requested() {
                std::thread::sleep(Duration::from_millis(50));
            }
            signals.requested()
        },
        || lost.load(Ordering::Acquire),
        || link.leave(),
    )
}
#[cfg(not(target_os = "linux"))]
fn connect(_: &LinuxClinkLink, _: Option<&str>) -> Result<()> {
    Err("linux_link_unavailable".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[derive(Default)]
    struct FakeNetwork {
        present: Cell<bool>,
        leaves: Cell<usize>,
        lost: Cell<bool>,
        fail: bool,
    }
    impl FakeNetwork {
        fn leave(&self) -> Result<()> {
            self.leaves.set(self.leaves.get() + 1);
            if self.fail {
                return Err("fake delete failed".into());
            }
            self.present.set(false);
            Ok(())
        }
    }

    #[test]
    fn either_signal_leaves_once_and_succeeds() {
        for signal in ["SIGINT", "SIGTERM"] {
            let network = FakeNetwork::default();
            network.present.set(true);
            let mut events = [None, None, Some(signal)].into_iter();
            wait_and_leave(
                || events.next().unwrap().is_some(),
                || network.lost.get(),
                || network.leave(),
            )
            .unwrap();
            assert!(!network.present.get());
            assert_eq!(network.leaves.get(), 1);
            network.leave().unwrap();
            assert!(!network.present.get());
        }
    }

    #[test]
    fn link_loss_leaves_and_reports_failure() {
        let network = FakeNetwork::default();
        network.present.set(true);
        let mut polls = 0;
        let error = wait_and_leave(
            || {
                polls += 1;
                network.lost.set(polls == 3);
                false
            },
            || network.lost.get(),
            || network.leave(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("head unit network link lost"));
        assert!(!network.present.get());
        assert_eq!(network.leaves.get(), 1);
    }

    #[test]
    fn signal_wins_if_loss_arrives_together() {
        let network = FakeNetwork::default();
        network.lost.set(true);
        wait_and_leave(|| true, || network.lost.get(), || network.leave()).unwrap();
        assert_eq!(network.leaves.get(), 1);
    }

    #[test]
    fn cleanup_failure_is_reported_for_signal_and_loss() {
        for interrupted in [true, false] {
            let network = FakeNetwork {
                fail: true,
                ..FakeNetwork::default()
            };
            network.present.set(true);
            let error = wait_and_leave(|| interrupted, || true, || network.leave()).unwrap_err();
            assert!(error.to_string().contains("fake delete failed"));
            if !interrupted {
                assert!(error.to_string().contains("head unit network link lost"));
            }
            assert_eq!(network.leaves.get(), 1);
            assert!(network.present.get());
        }
    }
}
