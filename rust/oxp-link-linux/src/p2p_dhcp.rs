// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

use crate::command::{run, Process};
use oxp_clink::Result;
use std::{
    fs::{self, DirBuilder, OpenOptions},
    io::Write,
    net::Ipv4Addr,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::Instant,
};

static NEXT: AtomicU64 = AtomicU64::new(1);
const SCRIPT: &str = r#"#!/bin/sh
# SPDX-FileCopyrightText: 2026 OpenXPlay.org
# SPDX-License-Identifier: AGPL-3.0-only
# Created by riko chen on 2026-10-27
case "$1" in
    bound|renew)
        printf '%s\n' "$ip" "$subnet" "$serverid" > "$0.state.new" &&
            busybox mv "$0.state.new" "$0.state"
        ;;
    deconfig|nak|leasefail)
        : > "$0.state.new"
        busybox mv "$0.state.new" "$0.state"
        ;;
esac
"#;
pub(super) fn available(deadline: Instant) -> bool {
    run("busybox", &["--list"], deadline).is_ok_and(|list| {
        ["udhcpc", "ifconfig", "mv"]
            .iter()
            .all(|a| list.lines().any(|line| line == *a))
    })
}
fn parse(text: &str) -> Result<(Ipv4Addr, Ipv4Addr, Ipv4Addr)> {
    let mut fields = text.lines();
    let local: Ipv4Addr = fields
        .next()
        .ok_or("p2p_dhcp_invalid")?
        .parse()
        .map_err(|_| "p2p_dhcp_invalid")?;
    let mask: Ipv4Addr = fields
        .next()
        .ok_or("p2p_dhcp_invalid")?
        .parse()
        .map_err(|_| "p2p_dhcp_invalid")?;
    let peer: Ipv4Addr = fields
        .next()
        .ok_or("p2p_dhcp_invalid")?
        .parse()
        .map_err(|_| "p2p_dhcp_invalid")?;
    let valid = |ip: Ipv4Addr| {
        ip.octets()[0] > 0 && ip.octets()[0] < 224 && !ip.is_loopback() && !ip.is_link_local()
    };
    let bits = u32::from(mask);
    if !valid(local)
        || !valid(peer)
        || local == peer
        || bits == 0
        || (!bits).checked_add(1).is_none_or(|n| n & !bits != 0)
        || u32::from(local) & bits != u32::from(peer) & bits
    {
        return Err("p2p_dhcp_invalid".into());
    }
    Ok((local, peer, mask))
}

/// DHCP never installs a default route or DNS. Only the new P2P group interface is configured.
pub(super) struct Lease {
    directory: PathBuf,
    child: Mutex<Option<Process>>,
}
impl Lease {
    pub fn start(interface: &str) -> Result<Self> {
        let directory = std::env::temp_dir().join(format!(
            "oxp-p2p-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        DirBuilder::new().mode(0o700).create(&directory)?;
        let lease = Self {
            directory,
            child: Mutex::new(None),
        };
        let script = lease.directory.join("lease.sh");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&script)?;
        // udhcpc supplies untrusted DHCP values as environment variables. Quote them and parse
        // addresses in Rust before applying anything; no router, resolver or global hook runs.
        file.write_all(SCRIPT.as_bytes())?;
        let child = Command::new("busybox")
            .args(["udhcpc", "-f", "-n", "-i", interface, "-s"])
            .arg(&script)
            .args(["-t", "3", "-T", "1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        *lease.child.lock().unwrap() = Some(Process(child));
        Ok(lease)
    }
    fn read(&self) -> Result<(Ipv4Addr, Ipv4Addr, Ipv4Addr)> {
        let mut child = self.child.lock().unwrap();
        if child
            .as_mut()
            .ok_or("p2p_dhcp_stopped")?
            .0
            .try_wait()?
            .is_some()
        {
            return Err("p2p_dhcp_stopped".into());
        }
        let state = self.directory.join("lease.sh.state");
        if fs::metadata(&state)?.len() > 128 {
            return Err("p2p_dhcp_invalid".into());
        }
        parse(&fs::read_to_string(state)?)
    }
    pub fn configure(&self, interface: &str, deadline: Instant) -> Result<(Ipv4Addr, Ipv4Addr)> {
        loop {
            crate::p2p::check(deadline)?;
            if let Ok((local, peer, mask)) = self.read() {
                run(
                    "busybox",
                    &[
                        "ifconfig",
                        interface,
                        &local.to_string(),
                        "netmask",
                        &mask.to_string(),
                        "up",
                    ],
                    deadline,
                )?;
                return Ok((local, peer));
            }
            if self
                .child
                .lock()
                .unwrap()
                .as_mut()
                .ok_or("p2p_dhcp_stopped")?
                .0
                .try_wait()?
                .is_some()
            {
                return Err("p2p_dhcp_failed".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    pub fn alive(
        &self,
        local: Ipv4Addr,
        peer: Ipv4Addr,
        interface: &str,
        deadline: Instant,
    ) -> Result<()> {
        let (actual_local, actual_peer, _) = self.read()?;
        let state = run("busybox", &["ifconfig", interface], deadline)?;
        let matches = state
            .split_whitespace()
            .any(|s| s.strip_prefix("addr:") == Some(&local.to_string()));
        if actual_local != local || actual_peer != peer || !matches {
            return Err("network_link_lost".into());
        }
        Ok(())
    }
    pub fn stop(&self) {
        self.child.lock().unwrap().take();
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.stop();
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dhcp_requires_distinct_routable_endpoints_and_a_valid_connected_subnet() {
        assert_eq!(
            parse("192.168.49.2\n255.255.255.0\n192.168.49.1\n")
                .unwrap()
                .1,
            Ipv4Addr::new(192, 168, 49, 1)
        );
        for (local, peer, mask) in [
            ("169.254.1.2", "192.168.49.1", "255.255.255.0"),
            ("0.1.1.1", "0.1.1.2", "255.0.0.0"),
            ("192.168.49.2", "192.168.49.2", "255.255.255.0"),
            ("192.168.49.2", "192.168.50.1", "255.255.255.0"),
            ("192.168.49.2", "192.168.49.1", "255.0.255.0"),
            ("192.168.49.2", "192.168.49.1", "0.0.0.0"),
        ] {
            assert!(parse(&format!("{local}\n{mask}\n{peer}\n")).is_err());
        }
    }
}
