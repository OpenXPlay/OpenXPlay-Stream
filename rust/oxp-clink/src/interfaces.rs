// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use crate::{protocol::AuthIdentity, require, ClinkTransport, Result, TcpTransport};
use std::{
    net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream},
    sync::Arc,
    time::Duration,
};
#[derive(Clone, Debug)]
pub struct ClinkAddresses {
    pub head_unit_ipv4: Ipv4Addr,
    pub local_ipv4: Ipv4Addr,
    pub auth_port: u16,
    pub head_unit_id: String,
}
impl ClinkAddresses {
    pub fn new(head_unit_ipv4: Ipv4Addr, local_ipv4: Ipv4Addr) -> Self {
        Self {
            head_unit_ipv4,
            local_ipv4,
            auth_port: 57209,
            head_unit_id: head_unit_ipv4.to_string(),
        }
    }
}
#[derive(Clone)]
pub struct ClinkConnection {
    pub head_unit_id: String,
    pub local_ipv4: Ipv4Addr,
    pub transport: Arc<dyn ClinkTransport>,
    /// PIN supplied by a wired transport instead of an interactive provider.
    pub wired_pin: Option<String>,
    /// The transport has already obtained the user's consent to pair.
    pub auto_confirm: bool,
}
impl From<ClinkAddresses> for ClinkConnection {
    fn from(addresses: ClinkAddresses) -> Self {
        Self {
            head_unit_id: addresses.head_unit_id.clone(),
            local_ipv4: addresses.local_ipv4,
            transport: Arc::new(TcpTransport::new(addresses)),
            wired_pin: None,
            auto_confirm: false,
        }
    }
}
pub trait ClinkLink: Send + Sync {
    fn connect(
        &self,
        on_link_lost: Arc<dyn Fn(Option<String>) + Send + Sync>,
    ) -> Result<ClinkConnection>;
    fn disconnect(&self) -> Result<()>;
}
pub struct StaticClinkLink(pub ClinkAddresses);
impl ClinkLink for StaticClinkLink {
    fn connect(&self, _: Arc<dyn Fn(Option<String>) + Send + Sync>) -> Result<ClinkConnection> {
        Ok(self.0.clone().into())
    }
    fn disconnect(&self) -> Result<()> {
        Ok(())
    }
}
pub trait PinProvider: Send + Sync {
    fn request_pin(&self, head_unit_id: &str) -> Option<String>;
    fn confirm_pairing(&self, _: &str) -> bool {
        false
    }
    /// A supplied PIN was rejected; the shell may prompt again for a new session.
    fn pin_incorrect(&self, _: &str) {}
}
pub trait TrustStore: Send + Sync {
    fn load_identity(&self) -> Result<Option<AuthIdentity>>;
    fn save_identity(&self, identity: &AuthIdentity) -> Result<()>;
    fn load_peer_key(&self, head_unit_id: &str) -> Result<Option<Vec<u8>>>;
    fn save_peer_key(&self, head_unit_id: &str, key: &[u8]) -> Result<()>;
    fn forget_peer(&self, head_unit_id: &str) -> Result<()>;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClinkState {
    Connecting,
    Authenticating,
    Negotiating,
    Playing,
    Ended,
}
/// Cumulative media counters for the current Carlink session, independent of tracing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClinkStatistics {
    pub live_frames: u64,
    pub placeholder_frames: u64,
    pub dropped_frames: u64,
    pub live_bytes: u64,
    pub send_stalls: u64,
    pub codec: Option<crate::model::VideoCodec>,
    pub width: u32,
    pub height: u32,
}
pub trait ClinkListener: Send + Sync {
    fn state(&self, _: ClinkState) {}
    fn error(&self, _: &str) {}
    fn diagnostic(&self, _: &str) {}
    /// Head-unit view size on the first UIBC touch of a session and whenever it changes.
    fn view_size(&self, _: u32, _: u32) {}
    fn statistics(&self, _: ClinkStatistics) {}
    fn profile_mismatch(&self, _: &str, _: &[String]) {}
}
impl ClinkListener for () {}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClinkHevcMode {
    #[default]
    Auto,
    Off,
}
#[derive(Clone, Copy, Debug)]
pub struct ClinkPorts {
    pub auth: u16,
    pub control: u16,
    pub media: u16,
    pub cert: u16,
    pub rtsp: u16,
    pub uibc: u16,
    pub rtp: u16,
}
impl Default for ClinkPorts {
    fn default() -> Self {
        Self {
            auth: 57209,
            control: 57219,
            media: 57229,
            cert: 57249,
            rtsp: 7236,
            uibc: 4321,
            rtp: 15550,
        }
    }
}
impl ClinkPorts {
    pub fn validate(&self) -> Result<()> {
        let ports = [
            self.auth,
            self.control,
            self.media,
            self.cert,
            self.rtsp,
            self.uibc,
            self.rtp,
        ];
        require(
            ports.iter().all(|p| *p > 0)
                && ports
                    .iter()
                    .enumerate()
                    .all(|(i, p)| !ports[..i].contains(p)),
            "invalid_ports",
        )
    }
}
/// Inject listeners and outbound sockets for deterministic session tests.
pub trait SocketProvider: Send + Sync {
    fn listen(&self, local: Ipv4Addr, port: u16) -> Result<TcpListener>;
    fn connect(
        &self,
        local: Ipv4Addr,
        peer: Ipv4Addr,
        port: u16,
        timeout: Duration,
    ) -> Result<TcpStream>;
}
pub struct StdSockets;
impl SocketProvider for StdSockets {
    fn listen(&self, local: Ipv4Addr, port: u16) -> Result<TcpListener> {
        Ok(TcpListener::bind(SocketAddrV4::new(local, port))?)
    }
    fn connect(
        &self,
        local: Ipv4Addr,
        peer: Ipv4Addr,
        port: u16,
        timeout: Duration,
    ) -> Result<TcpStream> {
        let socket = TcpStream::connect_timeout(&SocketAddrV4::new(peer, port).into(), timeout)?;
        // std cannot bind an outbound TCP socket; reject an incorrectly routed connection.
        require(
            socket.local_addr()?.ip() == std::net::IpAddr::V4(local),
            "local_address_route_mismatch",
        )?;
        Ok(socket)
    }
}
