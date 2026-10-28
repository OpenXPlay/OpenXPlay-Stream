// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

use crate::{diagnostics, LinkEvent};
use oxp_clink::{ble::HeadUnitReply, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum P2pCapability {
    Unavailable,
    ReservedByCarPlay,
    Available,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinMethod {
    P2pCredentials,
    Infrastructure,
}
impl JoinMethod {
    pub fn first(reply_type: i32, capability: P2pCapability) -> Self {
        if reply_type != 1002 && capability == P2pCapability::Available {
            Self::P2pCredentials
        } else {
            Self::Infrastructure
        }
    }
}
/// A backend owns and cleans up its P2P attempt before returning an error.
/// Unavailable includes platforms without a P2P client implementation.
pub trait WifiJoinBackend {
    type Connection;
    fn p2p_capability(&self) -> P2pCapability;
    fn join_p2p(&mut self, reply: &HeadUnitReply) -> Result<Self::Connection>;
    fn join_infrastructure(&mut self, reply: &HeadUnitReply) -> Result<Self::Connection>;
}
fn choice(method: JoinMethod, reason: &str, emit: &mut dyn FnMut(LinkEvent)) {
    emit(LinkEvent::Trace {
        stage: "wifi_join_method".into(),
        fields: [
            (
                "method".into(),
                match method {
                    JoinMethod::P2pCredentials => "p2p_credentials",
                    JoinMethod::Infrastructure => "infrastructure",
                }
                .into(),
            ),
            ("reason".into(), reason.into()),
        ]
        .into(),
    });
}
pub fn join_with<B: WifiJoinBackend>(
    backend: &mut B,
    reply: &HeadUnitReply,
    emit: &mut dyn FnMut(LinkEvent),
) -> Result<B::Connection> {
    let capability = backend.p2p_capability();
    let method = JoinMethod::first(reply.kind, capability);
    let reason = if reply.kind == 1002 {
        "head_unit_soft_ap"
    } else {
        match capability {
            P2pCapability::Unavailable => "p2p_unavailable",
            P2pCapability::ReservedByCarPlay => "carplay_reserved",
            P2pCapability::Available => "p2p_available",
        }
    };
    choice(method, reason, emit);
    if method == JoinMethod::P2pCredentials {
        match backend.join_p2p(reply) {
            Ok(connection) => return Ok(connection),
            Err(error) => {
                let code = diagnostics::code(error.as_ref());
                if code == "shutdown_requested" {
                    return Err(error);
                }
                emit(LinkEvent::Trace {
                    stage: "wifi_join_p2p_failed".into(),
                    fields: [("code".into(), code)].into(),
                });
                choice(JoinMethod::Infrastructure, "p2p_fallback", emit);
            }
        }
    }
    backend.join_infrastructure(reply)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Backend {
        capability: P2pCapability,
        p2p_error: Option<&'static str>,
        calls: Vec<&'static str>,
    }
    impl WifiJoinBackend for Backend {
        type Connection = ();
        fn p2p_capability(&self) -> P2pCapability {
            self.capability
        }
        fn join_p2p(&mut self, _: &HeadUnitReply) -> Result<()> {
            self.calls.push("p2p");
            self.p2p_error.map_or(Ok(()), |e| Err(e.into()))
        }
        fn join_infrastructure(&mut self, _: &HeadUnitReply) -> Result<()> {
            self.calls.push("infrastructure");
            Ok(())
        }
    }
    #[test]
    fn android_reply_capability_fallback_and_cancel_decisions() {
        for (kind, capability, error, expected) in [
            (1002, P2pCapability::Available, None, vec!["infrastructure"]),
            (
                1001,
                P2pCapability::Unavailable,
                None,
                vec!["infrastructure"],
            ),
            (
                1001,
                P2pCapability::ReservedByCarPlay,
                None,
                vec!["infrastructure"],
            ),
            (1001, P2pCapability::Available, None, vec!["p2p"]),
            (
                1001,
                P2pCapability::Available,
                Some("command_timeout"),
                vec!["p2p", "infrastructure"],
            ),
            (
                1001,
                P2pCapability::Available,
                Some("shutdown_requested"),
                vec!["p2p"],
            ),
        ] {
            let mut backend = Backend {
                capability,
                p2p_error: error,
                calls: vec![],
            };
            let reply = HeadUnitReply {
                name: "unit".into(),
                ssid: "private-ssid".into(),
                psk: "secret-passphrase".into(),
                mac: "aa:bb:cc:dd:ee:ff".into(),
                freq: 5180,
                port: 0,
                kind,
            };
            let mut events = vec![];
            let result = join_with(&mut backend, &reply, &mut |e| events.push(e));
            assert_eq!(backend.calls, expected);
            assert_eq!(result.is_err(), error == Some("shutdown_requested"));
            assert!(!format!("{events:?}").contains("secret-passphrase"));
            if expected.len() == 2 {
                assert!(format!("{events:?}").contains("p2p_fallback"));
            }
        }
    }
}
