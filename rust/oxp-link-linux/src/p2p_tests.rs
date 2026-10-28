// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

use super::*;
use crate::join_policy::{join_with, WifiJoinBackend};
use std::sync::Arc;

#[derive(Default)]
struct State {
    calls: Vec<&'static str>,
    credentials: bool,
    group: Option<ClientGroup>,
    failure: Option<&'static str>,
    cleanup_fails: bool,
}
#[derive(Clone, Default)]
struct FakeSupplicant(Arc<Mutex<State>>);
fn group() -> ClientGroup {
    ClientGroup {
        object: "/fi/w1/wpa_supplicant1/Interfaces/9".into(),
        interface: "p2p-wlan0-0".into(),
        ssid: b"DIRECT-car".to_vec(),
        client: true,
    }
}
fn reply() -> HeadUnitReply {
    HeadUnitReply {
        name: "car".into(),
        ssid: "DIRECT-car".into(),
        psk: "private-passphrase".into(),
        mac: "02:00:00:00:00:01".into(),
        freq: 5180,
        kind: 1001,
        port: 7777,
    }
}
impl Supplicant for FakeSupplicant {
    fn add_credentials(&self, credentials: &HeadUnitReply, _: Instant) -> Result<String> {
        assert_eq!(credentials.ssid, reply().ssid);
        assert_eq!(credentials.psk, reply().psk);
        assert_eq!(credentials.mac, reply().mac);
        let mut state = self.0.lock().unwrap();
        state.calls.push("credentials");
        state.credentials = true;
        Ok("private-object".into())
    }
    fn start(&self, credentials: &str, frequency: i32, _: Instant) -> Result<()> {
        assert_eq!(credentials, "private-object");
        assert_eq!(frequency, 5180);
        let mut state = self.0.lock().unwrap();
        state.calls.push("start");
        if state.failure != Some("timeout") {
            state.group = Some(group());
        }
        if let Some(failure) = state.failure.filter(|e| *e != "timeout") {
            return Err(failure.into());
        }
        Ok(())
    }
    fn group(&self, _: Instant, _: bool) -> Result<Option<ClientGroup>> {
        Ok(self.0.lock().unwrap().group.clone())
    }
    fn cancel(&self, _: Instant) -> Result<()> {
        self.0.lock().unwrap().calls.push("cancel");
        Ok(())
    }
    fn remove_group(&self, group: &ClientGroup, _: Instant) -> Result<()> {
        let mut state = self.0.lock().unwrap();
        assert_eq!(state.group.as_ref(), Some(group));
        state.calls.push("remove-group");
        if state.cleanup_fails {
            return Err("tool_failed".into());
        }
        state.group = None;
        Ok(())
    }
    fn remove_credentials(&self, _: &str, _: Instant) -> Result<()> {
        let mut state = self.0.lock().unwrap();
        state.calls.push("remove-credentials");
        state.credentials = false;
        Ok(())
    }
}
struct FakeDhcp(FakeSupplicant);
impl Dhcp for FakeDhcp {
    fn stop(&self) {
        self.0 .0.lock().unwrap().calls.push("stop-dhcp");
    }
}
fn lease(
    fake: FakeSupplicant,
) -> impl FnOnce(&ClientGroup, Instant) -> Result<(FakeDhcp, Ipv4Addr, Ipv4Addr)> {
    move |g, _| {
        assert_eq!(g, &group());
        fake.0.lock().unwrap().calls.push("dhcp");
        Ok((
            FakeDhcp(fake),
            Ipv4Addr::new(192, 168, 49, 2),
            Ipv4Addr::new(192, 168, 49, 1),
        ))
    }
}
#[test]
fn supplicant_joins_with_ble_credentials_and_releases_only_its_group_in_order() {
    let fake = FakeSupplicant::default();
    let (session, local, peer) = connect_with(
        fake.clone(),
        &Config::default(),
        &reply(),
        lease(fake.clone()),
    )
    .unwrap();
    assert_eq!(local, Ipv4Addr::new(192, 168, 49, 2));
    assert_eq!(peer, Ipv4Addr::new(192, 168, 49, 1));
    assert!(fake.0.lock().unwrap().credentials);
    session.release().unwrap();
    session.release().unwrap();
    drop(session);
    let state = fake.0.lock().unwrap();
    assert!(!state.credentials && state.group.is_none());
    assert_eq!(
        state.calls,
        [
            "credentials",
            "start",
            "dhcp",
            "stop-dhcp",
            "cancel",
            "remove-group",
            "remove-credentials"
        ]
    );
}

struct Backend {
    fake: FakeSupplicant,
    config: Config,
}
impl WifiJoinBackend for Backend {
    type Connection = ();
    fn p2p_capability(&self) -> P2pCapability {
        if self.config.p2p_reserved_by_carplay {
            P2pCapability::ReservedByCarPlay
        } else {
            P2pCapability::Available
        }
    }
    fn join_p2p(&mut self, reply: &HeadUnitReply) -> Result<()> {
        connect_with::<_, FakeDhcp>(self.fake.clone(), &self.config, reply, |_, _| {
            Err("dhcp_failed".into())
        })
        .map(|_| ())
    }
    fn join_infrastructure(&mut self, _: &HeadUnitReply) -> Result<()> {
        let mut state = self.fake.0.lock().unwrap();
        assert!(
            !state.credentials && state.group.is_none(),
            "fallback ran before cleanup"
        );
        state.calls.push("infrastructure");
        Ok(())
    }
}
#[test]
fn start_dhcp_timeout_and_cancel_failures_unwind_before_policy_fallback() {
    for failure in [
        None,
        Some("start_failed"),
        Some("timeout"),
        Some("shutdown_requested"),
    ] {
        let fake = FakeSupplicant::default();
        fake.0.lock().unwrap().failure = failure;
        let mut backend = Backend {
            fake: fake.clone(),
            config: Config {
                join_timeout: Duration::from_millis(1),
                ..Config::default()
            },
        };
        let mut events = Vec::new();
        let result = join_with(&mut backend, &reply(), &mut |e| events.push(e));
        assert_eq!(result.is_err(), failure == Some("shutdown_requested"));
        let state = fake.0.lock().unwrap();
        assert!(!state.credentials && state.group.is_none());
        assert_eq!(
            state.calls.contains(&"infrastructure"),
            failure != Some("shutdown_requested")
        );
        assert!(!format!("{events:?}").contains("private-passphrase"));
    }
}
#[test]
fn reservation_and_soft_ap_bypass_p2p_and_cleanup_failure_blocks_fallback() {
    for (reserved, kind) in [(true, 1001), (false, 1002)] {
        let mut backend = Backend {
            fake: FakeSupplicant::default(),
            config: Config {
                p2p_reserved_by_carplay: reserved,
                ..Config::default()
            },
        };
        let mut reply = reply();
        reply.kind = kind;
        join_with(&mut backend, &reply, &mut |_| {}).unwrap();
        assert_eq!(backend.fake.0.lock().unwrap().calls, ["infrastructure"]);
    }
    let mut backend = Backend {
        fake: FakeSupplicant::default(),
        config: Config::default(),
    };
    backend.fake.0.lock().unwrap().cleanup_fails = true;
    let error = join_with(&mut backend, &reply(), &mut |_| {}).unwrap_err();
    assert_eq!(error.to_string(), "wifi_join_p2p_cleanup_failed");
    assert!(!backend
        .fake
        .0
        .lock()
        .unwrap()
        .calls
        .contains(&"infrastructure"));
}
#[test]
fn cleanup_never_removes_a_go_wrong_ssid_or_parent_interface() {
    for change in 0..3 {
        let fake = FakeSupplicant::default();
        let session = Session::<_, FakeDhcp> {
            supplicant: fake.clone(),
            ssid: b"DIRECT-car".to_vec(),
            parent: "wlan0".into(),
            credentials: None,
            started: true,
            group: None,
            lease: None,
            released: Mutex::new(false),
            cleanup_timeout: Duration::from_secs(1),
        };
        let mut group = group();
        match change {
            0 => group.client = false,
            1 => group.ssid = b"other".to_vec(),
            _ => group.interface = "wlan0".into(),
        }
        fake.0.lock().unwrap().group = Some(group);
        assert_eq!(
            session.release().unwrap_err().to_string(),
            "wifi_join_p2p_cleanup_failed"
        );
        assert!(!fake.0.lock().unwrap().calls.contains(&"remove-group"));
    }
}
#[test]
fn reservation_is_reported_without_probing_or_disclosing_credentials() {
    let mut events = Vec::new();
    let config = Config {
        p2p_reserved_by_carplay: true,
        ..Config::default()
    };
    let (capability, supplicant) = probe(&config, &reply(), &mut |e| events.push(e));
    assert_eq!(capability, P2pCapability::ReservedByCarPlay);
    assert!(supplicant.is_none());
    assert!(format!("{events:?}").contains("carplay_reserved"));
    assert!(!format!("{events:?}").contains("private-passphrase"));
    assert!(crate::p2p_reserved("on").unwrap());
    assert!(!crate::p2p_reserved("off").unwrap());
    assert!(crate::p2p_reserved("true").is_err());
}
