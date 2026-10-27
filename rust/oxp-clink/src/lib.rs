// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

#![deny(unsafe_code)]
pub mod ble;
#[cfg(unix)]
#[allow(unsafe_code)]
mod bound_socket;
pub mod interfaces;
pub mod media;
pub mod model;
pub mod output;
pub mod placeholder;
pub mod protocol;
pub mod session;
pub mod trace;
pub mod transport;
pub use transport::{ClinkStream, ClinkTransport, TcpTransport};
pub mod uibc;
pub mod video_delivery;
mod voice_buffer;
mod voice_delivery;
pub mod wfd;
pub use interfaces::*;
pub use output::ClinkOutput;
pub use session::ClinkSession;
pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;
pub(crate) fn require(value: bool, message: &'static str) -> Result<()> {
    if value {
        Ok(())
    } else {
        Err(message.into())
    }
}
