// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use crate::{
    command::{run, run_cleanup},
    Config,
};
use oxp_clink::{ble::HeadUnitReply, ClinkAddresses, Result};
use std::{
    net::Ipv4Addr,
    time::{Duration, Instant},
};

pub(crate) struct Network {
    pub id: String,
    interface: String,
    timeout: Duration,
}
impl Network {
    pub(crate) fn leave(&self) -> Result<()> {
        self.leave_before(Instant::now() + self.timeout.min(Duration::from_millis(800)))
    }
    fn leave_before(&self, deadline: Instant) -> Result<()> {
        let profiles = run_cleanup(
            "nmcli",
            &["--get-values", "NAME", "connection", "show"],
            deadline,
        )?;
        if !profiles.lines().any(|name| name == self.id) {
            return Ok(());
        }
        // Deleting an active profile also deactivates it; never disconnect other profiles.
        run_cleanup("nmcli", &["connection", "delete", "id", &self.id], deadline)?;
        Ok(())
    }
    pub(crate) fn alive(&self, expected: &ClinkAddresses) -> Result<()> {
        let end = Instant::now() + self.timeout;
        let active = run(
            "nmcli",
            &[
                "--get-values",
                "GENERAL.CONNECTION",
                "device",
                "show",
                &self.interface,
            ],
            end,
        );
        if active?.trim() != self.id {
            return Err("network_link_lost".into());
        }
        let (local, peer) = endpoints(&self.interface, end)?;
        if local != expected.local_ipv4 || peer != expected.head_unit_ipv4 {
            return Err("network_link_lost".into());
        }
        Ok(())
    }
}
fn ipv4(s: &str) -> Result<Ipv4Addr> {
    let ip: Ipv4Addr = s.parse().map_err(|_| "network_ipv4_invalid")?;
    if ip.is_unspecified() || ip.is_loopback() || ip.is_multicast() || ip.is_broadcast() {
        return Err("network_ipv4_invalid".into());
    }
    Ok(ip)
}
fn parse_endpoints(local: &str, options: &str) -> Result<(Ipv4Addr, Ipv4Addr)> {
    let local = local
        .lines()
        .next()
        .ok_or("network_ipv4_unavailable")?
        .split('/')
        .next()
        .ok_or("network_ipv4_unavailable")?;
    // nmcli --get-values prints all DHCP options on one line separated by " | ".
    let option = |name| {
        options.split(['\n', '|']).find_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == name).then_some(value.trim())
        })
    };
    // SoftAP gateways need not share the DHCP server's address. Some P2P
    // servers omit routers, in which case their server identifier is the peer.
    let peer = option("routers")
        .and_then(|value| value.split_whitespace().next())
        .or_else(|| option("dhcp_server_identifier"))
        .ok_or("network_dhcp_server_unavailable")?;
    Ok((ipv4(local.trim())?, ipv4(peer)?))
}
fn endpoints(interface: &str, deadline: Instant) -> Result<(Ipv4Addr, Ipv4Addr)> {
    let local = run(
        "nmcli",
        &["--get-values", "IP4.ADDRESS", "device", "show", interface],
        deadline,
    )?;
    let options = run(
        "nmcli",
        &["--get-values", "DHCP4.OPTION", "device", "show", interface],
        deadline,
    )?;
    parse_endpoints(&local, &options)
}
pub(crate) fn join(
    c: &Config,
    reply: &HeadUnitReply,
    device_id: String,
    emit: &mut dyn FnMut(crate::LinkEvent),
) -> Result<(Network, ClinkAddresses)> {
    struct Backend<'a> {
        config: &'a Config,
        device_id: String,
    }
    impl crate::join_policy::WifiJoinBackend for Backend<'_> {
        type Connection = (Network, ClinkAddresses);
        fn p2p_capability(&self) -> crate::join_policy::P2pCapability {
            // A radio alone is insufficient: the Linux P2P client arrives in wave 3.
            crate::join_policy::P2pCapability::Unavailable
        }
        fn join_p2p(&mut self, _: &HeadUnitReply) -> Result<Self::Connection> {
            Err("wifi_join_p2p_unsupported".into())
        }
        fn join_infrastructure(&mut self, reply: &HeadUnitReply) -> Result<Self::Connection> {
            join_infrastructure(self.config, reply, self.device_id.clone())
        }
    }
    crate::join_policy::join_with(
        &mut Backend {
            config: c,
            device_id,
        },
        reply,
        emit,
    )
}
fn join_infrastructure(
    c: &Config,
    reply: &HeadUnitReply,
    device_id: String,
) -> Result<(Network, ClinkAddresses)> {
    let uuid = std::fs::read_to_string("/proc/sys/kernel/random/uuid")?;
    let id = format!("oxp-clink-{}-{}", c.wifi_interface, uuid.trim());
    let network = Network {
        id,
        interface: c.wifi_interface.clone(),
        timeout: c.command_timeout,
    };
    let deadline = Instant::now() + c.join_timeout;
    let result = (|| {
        run(
            "nmcli",
            &[
                "connection",
                "add",
                "save",
                "no",
                "type",
                "wifi",
                "ifname",
                &c.wifi_interface,
                "con-name",
                &network.id,
                "ssid",
                &reply.ssid,
                "802-11-wireless.mode",
                "infrastructure",
                "802-11-wireless.cloned-mac-address",
                "preserve",
                "wifi-sec.key-mgmt",
                "wpa-psk",
                "wifi-sec.psk",
                &reply.psk,
                "ipv4.method",
                "auto",
                "ipv4.never-default",
                "yes",
                "ipv4.ignore-auto-dns",
                "yes",
                "ipv6.method",
                "disabled",
                "connection.autoconnect",
                "no",
            ],
            deadline,
        )?;
        let wait = c.join_timeout.as_secs().max(1).to_string();
        run(
            "nmcli",
            &[
                "--wait",
                &wait,
                "connection",
                "up",
                "id",
                &network.id,
                "ifname",
                &c.wifi_interface,
            ],
            deadline,
        )?;
        let (local, peer) = endpoints(&c.wifi_interface, deadline)?;
        let mut addresses = ClinkAddresses::new(peer, local);
        addresses.head_unit_id = device_id;
        // The SDK advertises its AUTH port in the bootstrap; zero uses the protocol default.
        if reply.port != 0 {
            addresses.auth_port = reply.port;
        }
        Ok(addresses)
    })();
    match result {
        Ok(addresses) => Ok((network, addresses)),
        Err(e) => {
            let _ = network.leave();
            Err(e)
        }
    }
}
fn temporary_profile(id: &str, interface: &str) -> bool {
    let prefix = format!("oxp-clink-{interface}-");
    let Some(uuid) = id.strip_prefix(&prefix) else {
        return false;
    };
    uuid.len() == 36
        && uuid.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}
pub(crate) fn leave(c: &Config) -> Result<()> {
    let deadline = Instant::now() + c.command_timeout.min(Duration::from_millis(800));
    let out = run_cleanup(
        "nmcli",
        &["--get-values", "NAME", "connection", "show"],
        deadline,
    )?;
    for id in out
        .lines()
        .filter(|s| temporary_profile(s, &c.wifi_interface))
    {
        Network {
            id: id.into(),
            interface: c.wifi_interface.clone(),
            timeout: c.command_timeout,
        }
        .leave_before(deadline)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_profiles_must_match_interface_and_generated_uuid() {
        let uuid = "01234567-89ab-cdef-0123-456789abcdef";
        assert!(temporary_profile(
            &format!("oxp-clink-wlan0-{uuid}"),
            "wlan0"
        ));
        for id in [
            "home-wifi".to_owned(),
            "oxp-clink-wlan0-personal".into(),
            format!("oxp-clink-wlan1-{uuid}"),
            format!("oxp-clink-wlan0-other-{uuid}"),
        ] {
            assert!(!temporary_profile(&id, "wlan0"));
        }
    }
    #[test]
    fn dhcp_server_without_default_gateway() {
        let (local, peer) = parse_endpoints(
            "198.51.100.71/24\n",
            "routers = 198.51.100.254\ndhcp_server_identifier = 198.51.100.9\n",
        )
        .unwrap();
        assert_eq!(local.to_string(), "198.51.100.71");
        assert_eq!(peer.to_string(), "198.51.100.254");
        assert_eq!(
            parse_endpoints("198.51.100.71/24", "routers = 198.51.100.1")
                .unwrap()
                .1
                .to_string(),
            "198.51.100.1"
        );
        assert!(parse_endpoints("198.51.100.71/24", "dhcp_server_identifier = invalid").is_err());
        // Real nmcli --get-values output: one line, options separated by " | ".
        let (_, peer) = parse_endpoints(
            "192.168.49.71/24\n",
            "broadcast_address = 192.168.49.255 | dhcp_lease_time = 3600 | dhcp_server_identifier = 192.168.49.1 | ip_address = 192.168.49.71\n",
        )
        .unwrap();
        assert_eq!(peer.to_string(), "192.168.49.1");
    }
}
