// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

use crate::command::{run, run_cleanup};
use oxp_clink::{ble::HeadUnitReply, Result};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

const SERVICE: &str = "fi.w1.wpa_supplicant1";
const ROOT: &str = "/fi/w1/wpa_supplicant1";
const INTERFACE: &str = "fi.w1.wpa_supplicant1.Interface";
const P2P: &str = "fi.w1.wpa_supplicant1.Interface.P2PDevice";
const GROUP: &str = "fi.w1.wpa_supplicant1.Group";

fn data(output: &str) -> Result<Value> {
    let value: Value = serde_json::from_str(output).map_err(|_| "p2p_bus_invalid")?;
    value
        .get("data")
        .cloned()
        .ok_or_else(|| "p2p_bus_invalid".into())
}
fn path(value: &Value) -> Result<String> {
    value
        .as_str()
        .or_else(|| value.get(0)?.as_str())
        .filter(|s| s.starts_with(&format!("{ROOT}/Interfaces/")))
        .map(str::to_owned)
        .ok_or_else(|| "p2p_bus_invalid".into())
}
fn bus(args: &[&str], deadline: Instant, cleanup: bool) -> Result<Value> {
    let mut argv = vec!["--system", "--json=short"];
    argv.extend_from_slice(args);
    let output = if cleanup {
        run_cleanup("busctl", &argv, deadline)?
    } else {
        run("busctl", &argv, deadline)?
    };
    if output.trim().is_empty() {
        Ok(Value::Null)
    } else {
        data(&output)
    }
}
fn property(
    object: &str,
    interface: &str,
    name: &str,
    deadline: Instant,
    cleanup: bool,
) -> Result<Value> {
    bus(
        &["get-property", SERVICE, object, interface, name],
        deadline,
        cleanup,
    )
}
fn interfaces(deadline: Instant, cleanup: bool) -> Result<BTreeSet<String>> {
    let value = property(ROOT, SERVICE, "Interfaces", deadline, cleanup)?;
    value
        .as_array()
        .ok_or("p2p_bus_invalid")?
        .iter()
        .map(path)
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ClientGroup {
    pub object: String,
    pub interface: String,
    pub ssid: Vec<u8>,
    pub client: bool,
}
pub(super) trait Supplicant {
    /// Failed adds must remove partial credentials or return wifi_join_p2p_cleanup_failed.
    fn add_credentials(&self, reply: &HeadUnitReply, deadline: Instant) -> Result<String>;
    fn start(&self, credentials: &str, frequency: i32, deadline: Instant) -> Result<()>;
    fn group(&self, deadline: Instant, cleanup: bool) -> Result<Option<ClientGroup>>;
    fn cancel(&self, deadline: Instant) -> Result<()>;
    fn remove_group(&self, group: &ClientGroup, deadline: Instant) -> Result<()>;
    fn remove_credentials(&self, credentials: &str, deadline: Instant) -> Result<()>;
}

/// New objects can belong to this attempt. Changes on the original interfaces are conflicts;
/// detecting them also prevents fallback from racing a supplicant that reused the parent.
pub(super) struct WpaSupplicant {
    object: String,
    parent: String,
    before: BTreeSet<String>,
}
impl WpaSupplicant {
    pub fn locate(interface: &str, deadline: Instant) -> Result<Self> {
        let before = interfaces(deadline, false)?;
        let mut selected = None;
        for name in [format!("p2p-dev-{interface}"), interface.to_owned()] {
            let object = bus(
                &["call", SERVICE, ROOT, SERVICE, "GetInterface", "s", &name],
                deadline,
                false,
            )
            .and_then(|v| path(&v));
            if let Ok(object) = object {
                // GetInterface alone does not mean the P2PDevice API is present or idle.
                if property(&object, P2P, "Group", deadline, false)
                    .is_ok_and(|v| v.as_str() == Some("/"))
                {
                    selected = Some(object);
                    break;
                }
            }
        }
        let object = selected.ok_or("p2p_device_unavailable")?;
        let parent = path(&bus(
            &[
                "call",
                SERVICE,
                ROOT,
                SERVICE,
                "GetInterface",
                "s",
                interface,
            ],
            deadline,
            false,
        )?)?;
        if property(&parent, P2P, "Group", deadline, false)?.as_str() != Some("/") {
            return Err("p2p_device_unavailable".into());
        }
        Ok(Self {
            object,
            parent,
            before,
        })
    }
    fn candidates(&self, current: BTreeSet<String>) -> Vec<String> {
        let mut candidates: Vec<_> = current.difference(&self.before).cloned().collect();
        // Prefer a new group interface. Only inspect the original interfaces if no new group
        // appeared, so a group reference on the management interface cannot mask the real child.
        candidates.extend([self.object.clone(), self.parent.clone()]);
        candidates
    }
    fn recover_credentials(&self, before: &BTreeSet<String>, tag: &str) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(3);
        let current = property(&self.object, P2P, "PersistentGroups", deadline, true)?;
        let current: BTreeSet<_> = current
            .as_array()
            .ok_or("p2p_bus_invalid")?
            .iter()
            .map(path)
            .collect::<Result<_>>()?;
        for object in current.difference(before) {
            let props = property(
                object,
                "fi.w1.wpa_supplicant1.PersistentGroup",
                "Properties",
                deadline,
                true,
            )?;
            let id = &props["id_str"];
            let id = id.as_str().or_else(|| id["data"].as_str());
            if id == Some(tag) || id == Some(format!("\"{tag}\"").as_str()) {
                self.remove_credentials(object, deadline)?;
            } else {
                return Err("wifi_join_p2p_cleanup_failed".into());
            }
        }
        Ok(())
    }
}
impl Supplicant for WpaSupplicant {
    fn add_credentials(&self, reply: &HeadUnitReply, deadline: Instant) -> Result<String> {
        // A client-mode persistent-group entry supplies credentials to GroupAdd. It stays in
        // memory only: never SaveConfig, and always remove this exact object at release.
        let before = property(&self.object, P2P, "PersistentGroups", deadline, false)?;
        let before = before
            .as_array()
            .ok_or("p2p_bus_invalid")?
            .iter()
            .map(path)
            .collect::<Result<BTreeSet<_>>>()?;
        let uuid = std::fs::read_to_string("/proc/sys/kernel/random/uuid")?;
        let tag = format!("oxp-clink-{}", uuid.trim());
        let bytes: Vec<_> = reply.ssid.as_bytes().iter().map(u8::to_string).collect();
        let count = bytes.len().to_string();
        let mut args = vec![
            "call",
            SERVICE,
            &self.object,
            P2P,
            "AddPersistentGroup",
            "a{sv}",
            "5",
            "ssid",
            "ay",
            &count,
        ];
        args.extend(bytes.iter().map(String::as_str));
        args.extend([
            "psk", "s", &reply.psk, "bssid", "s", &reply.mac, "mode", "i", "0", "id_str", "s", &tag,
        ]);
        let result = bus(&args, deadline, false).and_then(|v| path(&v));
        if result.is_err() && self.recover_credentials(&before, &tag).is_err() {
            return Err("wifi_join_p2p_cleanup_failed".into());
        }
        result
    }
    fn start(&self, credentials: &str, frequency: i32, deadline: Instant) -> Result<()> {
        bus(
            &[
                "call",
                SERVICE,
                &self.object,
                P2P,
                "GroupAdd",
                "a{sv}",
                "2",
                "persistent_group_object",
                "o",
                credentials,
                "frequency",
                "i",
                &frequency.to_string(),
            ],
            deadline,
            false,
        )?;
        Ok(())
    }
    fn group(&self, deadline: Instant, cleanup: bool) -> Result<Option<ClientGroup>> {
        for object in self.candidates(interfaces(deadline, cleanup)?) {
            // An unreadable group is uncertain state, not evidence that cleanup succeeded.
            let group = property(&object, P2P, "Group", deadline, cleanup)?;
            let Some(group) = group.as_str().filter(|s| *s != "/") else {
                continue;
            };
            let ssid = property(group, GROUP, "SSID", deadline, cleanup)?;
            let ssid: Option<Vec<u8>> = ssid
                .as_array()
                .and_then(|a| a.iter().map(|n| u8::try_from(n.as_u64()?).ok()).collect());
            return Ok(Some(ClientGroup {
                object: object.clone(),
                interface: property(&object, INTERFACE, "Ifname", deadline, cleanup)?
                    .as_str()
                    .ok_or("p2p_bus_invalid")?
                    .into(),
                client: property(group, GROUP, "Role", deadline, cleanup)?.as_str()
                    == Some("client"),
                ssid: ssid.ok_or("p2p_bus_invalid")?,
            }));
        }
        Ok(None)
    }
    fn cancel(&self, deadline: Instant) -> Result<()> {
        bus(
            &["call", SERVICE, &self.object, P2P, "Cancel"],
            deadline,
            true,
        )?;
        Ok(())
    }
    fn remove_group(&self, group: &ClientGroup, deadline: Instant) -> Result<()> {
        bus(
            &["call", SERVICE, &group.object, P2P, "Disconnect"],
            deadline,
            true,
        )?;
        Ok(())
    }
    fn remove_credentials(&self, credentials: &str, deadline: Instant) -> Result<()> {
        bus(
            &[
                "call",
                SERVICE,
                &self.object,
                P2P,
                "RemovePersistentGroup",
                "o",
                credentials,
            ],
            deadline,
            true,
        )?;
        Ok(())
    }
}
