// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

use oxp_bin::{Args, FileTrust};
use oxp_clink::{protocol::AuthIdentity, session::SessionOptions, TrustStore};
use oxp_wire::Result;

pub struct Identity {
    pub device_id: String,
    pub device_name: String,
    ble: bool,
}
impl Identity {
    pub fn from_args(args: &Args) -> Result<Self> {
        let ble_link = args.get("--clink-link") == Some("ble");
        let ble =
            match args
                .get("--clink-identity")
                .unwrap_or(if ble_link { "ble" } else { "legacy" })
            {
                "ble" if ble_link => true,
                "legacy" => false,
                "ble" => return Err("--clink-identity ble requires --clink-link ble".into()),
                _ => return Err("--clink-identity must be ble or legacy".into()),
            };
        let options = SessionOptions::default();
        let config = oxp_link_linux::Config::default();
        Ok(Self {
            device_id: if ble {
                args.get("--clink-id").unwrap_or(&config.device_id).into()
            } else {
                options.device_id
            },
            device_name: if ble {
                args.get("--clink-name")
                    .unwrap_or(&config.device_name)
                    .into()
            } else {
                options.device_name
            },
            ble,
        })
    }
    pub fn trust(&self, store: FileTrust) -> IdentityTrust {
        IdentityTrust {
            store,
            ble: self.ble,
        }
    }
}
/// Legacy car keys keep their original names; BLE AUTH uses a separate namespace.
/// The relay's signing identity stays stable across modes.
pub struct IdentityTrust {
    store: FileTrust,
    ble: bool,
}
impl IdentityTrust {
    fn peer(&self, id: &str) -> String {
        if self.ble {
            serde_json::to_string(&["ble", id]).unwrap()
        } else {
            id.into()
        }
    }
}
impl TrustStore for IdentityTrust {
    fn load_identity(&self) -> Result<Option<AuthIdentity>> {
        self.store.load_identity()
    }
    fn save_identity(&self, identity: &AuthIdentity) -> Result<()> {
        self.store.save_identity(identity)
    }
    fn load_peer_key(&self, id: &str) -> Result<Option<Vec<u8>>> {
        self.store.load_peer_key(&self.peer(id))
    }
    fn save_peer_key(&self, id: &str, key: &[u8]) -> Result<()> {
        self.store.save_peer_key(&self.peer(id), key)
    }
    fn forget_peer(&self, id: &str) -> Result<()> {
        self.store.forget_peer(&self.peer(id))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn identity(values: &[&str]) -> Result<Identity> {
        let (args, _) = super::super::parse_arguments(values.iter().map(|s| s.to_string()))?;
        Identity::from_args(&args)
    }
    #[test]
    fn auth_matches_ble_id_and_name_and_legacy_is_explicit() {
        for values in [
            vec!["--clink-link", "ble"],
            vec![
                "--clink-link",
                "ble",
                "--clink-id",
                "relay-42",
                "--clink-name",
                "Configured relay",
            ],
        ] {
            let actual = identity(&values).unwrap();
            let config = oxp_link_linux::Config::default();
            assert_eq!(
                actual.device_id,
                if values.len() > 2 {
                    "relay-42"
                } else {
                    &config.device_id
                }
            );
            assert_eq!(
                actual.device_name,
                if values.len() > 2 {
                    "Configured relay"
                } else {
                    &config.device_name
                }
            );
        }
        let legacy = identity(&[
            "--clink-link",
            "ble",
            "--clink-identity",
            "legacy",
            "--clink-id",
            "new-id",
        ])
        .unwrap();
        assert_eq!(legacy.device_id, SessionOptions::default().device_id);
        assert!(identity(&["--clink-identity", "ble"]).is_err());
        assert!(identity(&["--clink-link", "ble", "--clink-identity", "bad"]).is_err());
    }
    #[test]
    fn switching_modes_keeps_both_pairings_and_replacement_is_scoped() {
        let directory =
            std::env::temp_dir().join(format!("oxp-identity-mode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        let raw = FileTrust::new(&directory).unwrap();
        raw.save_peer_key("car", b"legacy-key").unwrap();
        let legacy = identity(&["--clink-link", "ble", "--clink-identity", "legacy"])
            .unwrap()
            .trust(raw);
        let ble_mode = identity(&["--clink-link", "ble"]).unwrap();
        let ble = ble_mode.trust(FileTrust::new(&directory).unwrap());
        assert_eq!(ble.load_peer_key("car").unwrap(), None);
        ble.save_peer_key("car", b"ble-key").unwrap();
        assert_eq!(legacy.load_peer_key("car").unwrap().unwrap(), b"legacy-key");
        assert_eq!(
            ble_mode
                .trust(FileTrust::new(&directory).unwrap())
                .load_peer_key("car")
                .unwrap()
                .unwrap(),
            b"ble-key"
        );
        ble.forget_peer("car").unwrap();
        assert_eq!(ble.load_peer_key("car").unwrap(), None);
        assert_eq!(legacy.load_peer_key("car").unwrap().unwrap(), b"legacy-key");
        std::fs::remove_dir_all(directory).unwrap();
    }
}
