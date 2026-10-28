// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

use serde_json::{json, Value};
use std::time::{Duration, Instant};
use std::{error::Error, io};

#[derive(Default)]
pub(crate) struct WriteProgress(Option<Instant>);
impl WriteProgress {
    pub(crate) fn attempted(&mut self, now: Instant, result: &io::Result<usize>, length: usize) {
        if length == 0 {
            return;
        }
        if result.as_ref().is_ok_and(|n| *n > 0) {
            self.0 = None;
        } else {
            self.0.get_or_insert(now);
        }
    }
    pub(crate) fn timed_out(&self, now: Instant) -> bool {
        self.0
            .is_some_and(|start| now.duration_since(start) >= Duration::from_secs(5))
    }
}

/// Host-only observation, never accepted from receiver JSON. Peer reason text is not logged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Disconnect {
    code: u16,
    reason: &'static str,
}
impl Disconnect {
    pub(crate) fn closed(code: u16) -> Self {
        Self {
            code,
            reason: "closed",
        }
    }
    pub(crate) fn timeout(code: u16) -> Self {
        Self {
            code,
            reason: "timeout",
        }
    }
    pub(crate) fn error(error: &(dyn Error + 'static)) -> Self {
        let timed_out = error
            .downcast_ref::<io::Error>()
            .is_some_and(|e| e.kind() == io::ErrorKind::TimedOut)
            || matches!(error.downcast_ref::<tungstenite::Error>(), Some(tungstenite::Error::Io(e)) if e.kind() == io::ErrorKind::TimedOut);
        Self {
            code: if timed_out
                || error.is::<io::Error>()
                || matches!(
                    error.downcast_ref::<tungstenite::Error>(),
                    Some(
                        tungstenite::Error::Io(_)
                            | tungstenite::Error::ConnectionClosed
                            | tungstenite::Error::Protocol(
                                tungstenite::error::ProtocolError::ResetWithoutClosingHandshake
                            )
                    )
                ) {
                1006
            } else {
                1002
            },
            reason: if timed_out { "timeout" } else { "error" },
        }
    }
    pub(crate) fn json(self) -> Value {
        json!({"type":"receiver-disconnected","code":self.code,"reason":self.reason})
    }
}
