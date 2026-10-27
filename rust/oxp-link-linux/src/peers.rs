// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

use crate::{Device, HandoffPolicy};
use oxp_clink::Result;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy)]
struct Peer {
    choice: HandoffPolicy,
    serial: Option<u8>,
    updated: u64,
}
#[derive(Clone, Default)]
pub(crate) struct Peers {
    peers: BTreeMap<String, Peer>,
}
pub(crate) fn key(device: &Device) -> String {
    let identity = if device.advertisement.car_id.trim().is_empty() {
        format!("address:{}", device.address.to_ascii_lowercase())
    } else {
        device.advertisement.car_id.clone()
    };
    format!("{:x}", Sha256::digest(identity.as_bytes()))
}
pub(crate) fn memory() -> &'static std::sync::Mutex<Peers> {
    static MEMORY: std::sync::OnceLock<std::sync::Mutex<Peers>> = std::sync::OnceLock::new();
    MEMORY.get_or_init(std::sync::Mutex::default)
}
impl Peers {
    pub(crate) fn load(directory: &Path) -> Result<Self> {
        let bytes = match fs::read(directory.join("carlink/ble-peers.json")) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e.into()),
        };
        let value: Value = serde_json::from_slice(&bytes)?;
        let mut out = Self::default();
        for (key, value) in value.as_object().ok_or("invalid_ble_peers")? {
            let Some(kind) = value["requestType"]
                .as_u64()
                .filter(|v| [1001, 1002].contains(v))
            else {
                continue;
            };
            if key.len() != 64
                || !key
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                continue;
            }
            out.peers.insert(
                key.clone(),
                Peer {
                    choice: HandoffPolicy {
                        legacy: value["legacy"].as_bool().unwrap_or(false),
                        use_1002: kind == 1002,
                    },
                    serial: value["serial"].as_u64().and_then(|v| u8::try_from(v).ok()),
                    updated: value["updated"].as_u64().unwrap_or(0),
                },
            );
        }
        out.trim();
        Ok(out)
    }
    pub(crate) fn known_choice(&self, key: &str) -> Option<HandoffPolicy> {
        self.peers.get(key).map(|p| p.choice)
    }
    pub(crate) fn choice(&self, key: &str, persistent: bool) -> HandoffPolicy {
        self.peers
            .get(key)
            .map(|p| p.choice)
            .unwrap_or(HandoffPolicy {
                use_1002: persistent,
                legacy: false,
            })
    }
    pub(crate) fn serial_same(&self, device: &Device) -> bool {
        self.peers
            .get(&key(device))
            .is_some_and(|p| p.serial == Some(device.advertisement.serial))
    }
    pub(crate) fn remember(&mut self, key: String, choice: HandoffPolicy, serial: Option<u8>) {
        let updated = self
            .peers
            .values()
            .map(|p| p.updated)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let serial = serial.or_else(|| self.peers.get(&key).and_then(|p| p.serial));
        self.peers.insert(
            key,
            Peer {
                choice,
                serial,
                updated,
            },
        );
        self.trim();
    }
    fn trim(&mut self) {
        while self.peers.len() > 32 {
            let oldest = self
                .peers
                .iter()
                .min_by_key(|(_, p)| p.updated)
                .map(|(k, _)| k.clone())
                .unwrap();
            self.peers.remove(&oldest);
        }
    }
    pub(crate) fn save(&self, directory: &Path) -> Result<()> {
        let directory = directory.join("carlink");
        fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        }
        let value: BTreeMap<_, _> = self.peers.iter().map(|(key, p)| (key, json!({
            "requestType": if p.choice.use_1002 { 1002 } else { 1001 }, "legacy": p.choice.legacy,
            "serial": p.serial, "updated": p.updated,
        }))).collect();
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let temp: PathBuf = directory.join(format!(
            ".ble-peers-{}-{}.tmp",
            std::process::id(),
            SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let result = (|| -> Result<()> {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temp)?;
            file.write_all(&serde_json::to_vec(&value)?)?;
            file.sync_all()?;
            fs::rename(&temp, directory.join("ble-peers.json"))?;
            fs::File::open(&directory)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn persistence_defaults_privacy_and_oldest_eviction() {
        let directory = std::env::temp_dir().join(format!("oxp-peers-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        let mut peers = Peers::load(&directory).unwrap();
        assert!(peers.choice("new", true).use_1002);
        assert!(!peers.choice("new", false).use_1002);
        for n in 0..33 {
            peers.remember(
                format!("{n:064x}"),
                HandoffPolicy {
                    use_1002: n % 2 == 0,
                    legacy: true,
                },
                Some(n as u8),
            );
        }
        peers.save(&directory).unwrap();
        let peers = Peers::load(&directory).unwrap();
        assert_eq!(peers.peers.len(), 32);
        assert!(!peers.peers.contains_key(&format!("{:064x}", 0)));
        assert!(peers.choice(&format!("{:064x}", 2), true).legacy);
        assert!(!peers.choice(&format!("{:064x}", 1), true).use_1002);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(directory.join("carlink/ble-peers.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        assert_eq!(fs::read_dir(directory.join("carlink")).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn serial_comparison_survives_reload_without_storing_car_identity() {
        let directory =
            std::env::temp_dir().join(format!("oxp-peers-serial-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        let mut device = Device {
            address: "aa:bb:cc:dd:ee:ff".into(),
            advertisement: oxp_clink::ble::Advertisement {
                major: 1,
                minor: 2,
                serial: 3,
                car_id: "private-car-id".into(),
                model_id: "model".into(),
                vendor_data: String::new(),
                name: "unit".into(),
            },
        };
        let mut peers = Peers::default();
        assert!(!peers.serial_same(&device));
        peers.remember(
            key(&device),
            HandoffPolicy {
                legacy: true,
                use_1002: true,
            },
            Some(3),
        );
        peers.save(&directory).unwrap();
        let text = fs::read_to_string(directory.join("carlink/ble-peers.json")).unwrap();
        assert!(!text.contains("private-car-id"));
        assert!(!text.contains(&device.address));
        let peers = Peers::load(&directory).unwrap();
        assert!(peers.serial_same(&device));
        device.address = "aa:bb:cc:dd:ee:01".into();
        assert!(peers.serial_same(&device));
        device.advertisement.serial = 4;
        assert!(!peers.serial_same(&device));
        fs::remove_dir_all(directory).unwrap();
    }
}
