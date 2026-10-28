// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

use crate::{
    command::run,
    join_policy::P2pCapability,
    p2p_dhcp::Lease,
    p2p_supplicant::{ClientGroup, Supplicant, WpaSupplicant},
    Config, LinkEvent,
};
use oxp_clink::{ble::HeadUnitReply, ClinkAddresses, Result};
use std::{
    net::Ipv4Addr,
    sync::Mutex,
    time::{Duration, Instant},
};

pub(super) fn check(deadline: Instant) -> Result<()> {
    #[cfg(target_os = "linux")]
    crate::signals::check()?;
    if Instant::now() >= deadline {
        return Err("p2p_join_timeout".into());
    }
    Ok(())
}
fn valid_interface(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 15
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}

pub(super) fn probe(
    config: &Config,
    reply: &HeadUnitReply,
    emit: &mut dyn FnMut(LinkEvent),
) -> (P2pCapability, Option<WpaSupplicant>) {
    if reply.kind == 1002 {
        return (P2pCapability::Unavailable, None);
    }
    let mut device = "unknown";
    let mut concurrent = "unknown";
    let mut capability = P2pCapability::Unavailable;
    let mut supplicant = None;
    let reason = if config.p2p_reserved_by_carplay {
        capability = P2pCapability::ReservedByCarPlay;
        "carplay_reserved"
    } else {
        let deadline = Instant::now() + config.command_timeout;
        let result = (|| -> Result<&str> {
            let info = run("iw", &["dev", &config.wifi_interface, "info"], deadline)?;
            let phy = info
                .lines()
                .find_map(|l| l.trim().strip_prefix("wiphy "))
                .filter(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
                .ok_or("p2p_phy_unavailable")?;
            let phy_info = run("iw", &["phy", &format!("phy{phy}"), "info"], deadline)?;
            let devices = run("iw", &["dev"], deadline)?;
            let radio = crate::p2p_radio::inspect(&phy_info, &devices, phy, reply.freq);
            device = if radio.device { "true" } else { "false" };
            concurrent = if radio.ap_concurrent { "true" } else { "false" };
            if !radio.device {
                return Ok("device_unsupported");
            }
            if !radio.usable {
                return Ok("interface_or_channel_busy");
            }
            if !crate::p2p_dhcp::available(deadline) {
                return Ok("dhcp_unavailable");
            }
            supplicant = Some(WpaSupplicant::locate(&config.wifi_interface, deadline)?);
            capability = P2pCapability::Available;
            Ok("ready")
        })();
        result.unwrap_or("supplicant_unavailable")
    };
    emit(LinkEvent::Trace {
        stage: "wifi_p2p_capability".into(),
        fields: [
            ("deviceSupported".into(), device.into()),
            ("apConcurrent".into(), concurrent.into()),
            ("reason".into(), reason.into()),
        ]
        .into(),
    });
    (capability, supplicant)
}

trait Dhcp {
    fn stop(&self);
}
impl Dhcp for Lease {
    fn stop(&self) {
        self.stop();
    }
}

struct Session<S: Supplicant, L: Dhcp> {
    supplicant: S,
    ssid: Vec<u8>,
    parent: String,
    credentials: Option<String>,
    started: bool,
    group: Option<ClientGroup>,
    lease: Option<L>,
    released: Mutex<bool>,
    cleanup_timeout: Duration,
}
impl<S: Supplicant, L: Dhcp> Session<S, L> {
    fn owns(&self, group: &ClientGroup) -> bool {
        group.client
            && group.ssid == self.ssid
            && valid_interface(&group.interface)
            && group.interface != self.parent
    }
    fn release(&self) -> Result<()> {
        let mut released = self.released.lock().unwrap();
        if *released {
            return Ok(());
        }
        let deadline = Instant::now() + self.cleanup_timeout;
        if let Some(lease) = &self.lease {
            lease.stop();
        }
        let mut clean = true;
        if self.started {
            clean &= self.supplicant.cancel(deadline).is_ok();
            match self.supplicant.group(deadline, true) {
                Ok(Some(group))
                    if self.owns(&group) && self.group.as_ref().is_none_or(|g| *g == group) =>
                {
                    clean &= self.supplicant.remove_group(&group, deadline).is_ok();
                    while clean {
                        match self.supplicant.group(deadline, true) {
                            Ok(None) => break,
                            Ok(Some(current)) if current == group && Instant::now() < deadline => {
                                std::thread::sleep(Duration::from_millis(50))
                            }
                            _ => {
                                clean = false;
                                break;
                            }
                        }
                    }
                }
                Ok(None) => {}
                _ => clean = false,
            }
        }
        if let Some(credentials) = &self.credentials {
            clean &= self
                .supplicant
                .remove_credentials(credentials, deadline)
                .is_ok();
        }
        *released = true;
        if clean {
            Ok(())
        } else {
            Err("wifi_join_p2p_cleanup_failed".into())
        }
    }
}
impl<S: Supplicant, L: Dhcp> Drop for Session<S, L> {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

fn connect_with<S: Supplicant, L: Dhcp>(
    supplicant: S,
    config: &Config,
    reply: &HeadUnitReply,
    lease: impl FnOnce(&ClientGroup, Instant) -> Result<(L, Ipv4Addr, Ipv4Addr)>,
) -> Result<(Session<S, L>, Ipv4Addr, Ipv4Addr)> {
    let mut session = Session {
        supplicant,
        ssid: reply.ssid.as_bytes().to_vec(),
        parent: config.wifi_interface.clone(),
        credentials: None,
        started: false,
        group: None,
        lease: None,
        released: Mutex::new(false),
        cleanup_timeout: config.command_timeout,
    };
    // Android gives P2P credentials seven seconds, then makes a fresh infrastructure attempt.
    let deadline = Instant::now() + config.join_timeout.min(Duration::from_secs(7));
    let result = (|| {
        check(deadline)?;
        session.credentials = Some(session.supplicant.add_credentials(reply, deadline)?);
        session.started = true;
        session.supplicant.start(
            session.credentials.as_deref().unwrap(),
            reply.freq,
            deadline,
        )?;
        let group = loop {
            check(deadline)?;
            if let Some(group) = session.supplicant.group(deadline, false)? {
                if !session.owns(&group) {
                    return Err("p2p_group_mismatch".into());
                }
                break group;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        session.group = Some(group.clone());
        let (dhcp, local, peer) = lease(&group, deadline)?;
        session.lease = Some(dhcp);
        Ok((local, peer))
    })();
    match result {
        Ok((local, peer)) => Ok((session, local, peer)),
        Err(error) => {
            session.release()?;
            Err(error)
        }
    }
}

pub(super) struct P2pNetwork {
    session: Session<WpaSupplicant, Lease>,
    local: Ipv4Addr,
    peer: Ipv4Addr,
    timeout: Duration,
}
impl P2pNetwork {
    pub fn join(
        supplicant: WpaSupplicant,
        config: &Config,
        reply: &HeadUnitReply,
        device_id: String,
    ) -> Result<(Self, ClinkAddresses)> {
        let (session, local, peer) = connect_with(supplicant, config, reply, |group, deadline| {
            let lease = Lease::start(&group.interface)?;
            let (local, peer) = lease.configure(&group.interface, deadline)?;
            Ok((lease, local, peer))
        })?;
        let mut addresses = ClinkAddresses::new(peer, local);
        addresses.head_unit_id = device_id;
        if reply.port != 0 {
            addresses.auth_port = reply.port;
        }
        Ok((
            Self {
                session,
                local,
                peer,
                timeout: config.command_timeout,
            },
            addresses,
        ))
    }
    pub fn leave(&self) -> Result<()> {
        self.session.release()
    }
    pub fn interface(&self) -> &str {
        &self.session.group.as_ref().unwrap().interface
    }
    pub fn alive(&self, expected: &ClinkAddresses) -> Result<()> {
        if *self.session.released.lock().unwrap()
            || expected.local_ipv4 != self.local
            || expected.head_unit_ipv4 != self.peer
        {
            return Err("network_link_lost".into());
        }
        let deadline = Instant::now() + self.timeout;
        let current = self
            .session
            .supplicant
            .group(deadline, false)?
            .ok_or("network_link_lost")?;
        if self.session.group.as_ref() != Some(&current) {
            return Err("network_link_lost".into());
        }
        self.session
            .lease
            .as_ref()
            .ok_or("p2p_dhcp_stopped")?
            .alive(self.local, self.peer, &current.interface, deadline)
    }
}

#[cfg(test)]
#[path = "p2p_tests.rs"]
mod tests;
