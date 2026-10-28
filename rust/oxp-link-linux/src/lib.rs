// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

#[cfg(any(target_os = "linux", test))]
mod att;
mod diagnostics;
pub mod join_policy;
mod retry;
pub use diagnostics::LinkFailure;
pub use retry::LinkBackoff;
#[cfg(any(target_os = "linux", test))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod bluez;
#[cfg(any(target_os = "linux", test))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod command;
#[cfg(any(target_os = "linux", test))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod network;
#[cfg(any(target_os = "linux", test))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod p2p;
#[cfg(any(target_os = "linux", test))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod p2p_dhcp;
#[cfg(any(target_os = "linux", test))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod p2p_radio;
#[cfg(any(target_os = "linux", test))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod p2p_supplicant;
#[cfg(any(target_os = "linux", test))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod peers;
#[cfg(any(target_os = "linux", test))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod raw;
#[cfg(unix)]
mod signals;
#[cfg(target_os = "linux")]
mod socket;
#[cfg(unix)]
pub use signals::ShutdownSignals;

use oxp_clink::{ble::Advertisement, ClinkAddresses, ClinkConnection, ClinkLink, Result};
use std::{sync::Arc, time::Duration};
#[cfg(target_os = "linux")]
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::Instant,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AddressType {
    Public = 1,
    Random = 2,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScanMode {
    #[default]
    Window,
    Continuous,
}
impl ScanMode {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "window" => Ok(Self::Window),
            "continuous" => Ok(Self::Continuous),
            _ => Err("--clink-scan must be window or continuous".into()),
        }
    }
}

pub fn p2p_reserved(value: &str) -> Result<bool> {
    match value {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err("P2P reservation must be on or off".into()),
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub bluetooth_adapter: String,
    pub wifi_interface: String,
    /// The relay must reserve P2P while CarPlay owns or is about to own Wi-Fi Direct.
    pub p2p_reserved_by_carplay: bool,
    pub address_type: AddressType,
    pub device_id: String,
    pub device_name: String,
    pub device_model: String,
    pub band: i32,
    pub scan_timeout: Duration,
    pub scan_mode: ScanMode,
    pub state_dir: Option<std::path::PathBuf>,
    pub phone_mac: Option<String>,
    pub trace_advertisements: bool,
    pub connect_timeout: Duration,
    pub handoff_timeout: Duration,
    pub join_timeout: Duration,
    pub command_timeout: Duration,
    pub connect_attempts: u32,
    /// Initial retry delay, clamped to Android's 1–15 second range.
    pub retry_delay: Duration,
    pub link_check_interval: Duration,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            bluetooth_adapter: "hci0".into(),
            wifi_interface: "wlan0".into(),
            p2p_reserved_by_carplay: false,
            address_type: AddressType::Random,
            device_id: "openxplay-pi".into(),
            device_name: "OpenXPlay".into(),
            device_model: "Linux adapter".into(),
            band: 1,
            scan_timeout: Duration::from_secs(8),
            scan_mode: ScanMode::Window,
            state_dir: None,
            phone_mac: None,
            trace_advertisements: false,
            connect_timeout: Duration::from_secs(3),
            handoff_timeout: Duration::from_secs(15),
            join_timeout: Duration::from_secs(15),
            command_timeout: Duration::from_secs(3),
            connect_attempts: 3,
            retry_delay: Duration::from_secs(1),
            link_check_interval: Duration::from_secs(1),
        }
    }
}
impl Config {
    pub fn validate(&self) -> Result<()> {
        let index = self
            .bluetooth_adapter
            .strip_prefix("hci")
            .ok_or("invalid_bluetooth_configuration")?;
        if index.is_empty()
            || !index.bytes().all(|b| b.is_ascii_digit())
            || self.wifi_interface.is_empty()
            || self.wifi_interface.len() > 15
            || !self
                .wifi_interface
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            || self.connect_attempts == 0
            || self.connect_attempts > 10
            || [
                self.scan_timeout,
                self.connect_timeout,
                self.handoff_timeout,
                self.join_timeout,
                self.command_timeout,
                self.link_check_interval,
            ]
            .iter()
            .any(Duration::is_zero)
        {
            return Err("invalid_link_configuration".into());
        }
        if let Some(mac) = &self.phone_mac {
            normalized_bluetooth_address(mac)?;
        }
        oxp_clink::ble::phone_request(&self.phone("00:00:00:00:00:00".into()))?;
        Ok(())
    }
    fn phone(&self, mac: String) -> oxp_clink::ble::PhoneRequest {
        oxp_clink::ble::PhoneRequest {
            id: self.device_id.clone(),
            name: self.device_name.clone(),
            model: self.device_model.clone(),
            band: self.band,
            mac: self
                .phone_mac
                .as_ref()
                .map(|value| value.to_ascii_lowercase())
                .unwrap_or(mac),
            kind: 1001,
        }
    }
}
#[derive(Clone, Debug)]
pub struct Device {
    pub address: String,
    pub advertisement: Advertisement,
}

#[derive(Clone, Debug)]
pub enum LinkEvent {
    Failure(LinkFailure),
    Trace {
        stage: String,
        fields: std::collections::BTreeMap<String, String>,
    },
    Diagnostic {
        reason: String,
    },
    Scanning,
    Advertisement {
        advertisement: Advertisement,
        serial_same: bool,
    },
    Connecting {
        address: String,
        name: String,
    },
    Joined {
        address: String,
        name: String,
        ssid: String,
        interface: String,
        addresses: ClinkAddresses,
    },
    Lost {
        reason: String,
    },
}
pub type LinkListener = Arc<dyn Fn(LinkEvent) + Send + Sync>;

#[cfg(any(target_os = "linux", test))]
fn select_device(devices: Vec<Device>, address: Option<&str>) -> Result<Device> {
    if let Some(address) = address {
        devices
            .into_iter()
            .find(|d| d.address.eq_ignore_ascii_case(address))
            .ok_or_else(|| "selected_vehicle_not_discovered".into())
    } else {
        match devices.len() {
            0 => Err("vehicle_not_discovered".into()),
            1 => devices
                .into_iter()
                .next()
                .ok_or_else(|| "vehicle_not_discovered".into()),
            _ => Err("multiple_vehicles_select_address".into()),
        }
    }
}

pub fn normalized_bluetooth_address(mac: &str) -> Result<String> {
    let bytes = mac_bytes(mac)?;
    if bytes[0] & 1 != 0 || bytes == [0; 6] || bytes == [2, 0, 0, 0, 0, 0] {
        return Err("invalid_phone_bluetooth_address".into());
    }
    Ok(mac.to_ascii_lowercase())
}

pub(crate) fn mac_bytes(mac: &str) -> Result<[u8; 6]> {
    let parts: Vec<_> = mac.split(':').collect();
    if parts.len() != 6
        || parts
            .iter()
            .any(|s| s.len() != 2 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err("invalid_bluetooth_address".into());
    }
    let mut out = [0; 6];
    for (b, s) in out.iter_mut().zip(parts) {
        *b = u8::from_str_radix(s, 16).map_err(|_| "invalid_bluetooth_address")?;
    }
    Ok(out)
}

#[derive(Debug)]
struct UnsupportedBand(i32);
impl std::fmt::Display for UnsupportedBand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "head_unit_band_unsupported:{}", self.0)
    }
}
impl std::error::Error for UnsupportedBand {}

#[cfg(any(target_os = "linux", test))]
fn join_after_handoff<S, T>(
    config: &Config,
    socket: S,
    reply: &oxp_clink::ble::HeadUnitReply,
    join: impl FnOnce() -> Result<T>,
) -> Result<T> {
    drop(socket);
    let supported = (2400..=2500).contains(&reply.freq)
        || (config.band == 1 && (5000..6000).contains(&reply.freq));
    if !supported {
        return Err(Box::new(UnsupportedBand(reply.freq)));
    }
    join()
}

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Copy, Default)]
struct HandoffPolicy {
    legacy: bool,
    use_1002: bool,
}
#[cfg(any(target_os = "linux", test))]
fn device_key(device: &Device) -> String {
    peers::key(device)
}
#[cfg(any(target_os = "linux", test))]
fn exchange<S: att::Socket>(
    config: &Config,
    mac: &str,
    policy: &mut HandoffPolicy,
    end: std::time::Instant,
    mut connect: impl FnMut() -> Result<S>,
    diagnostic: &mut dyn FnMut(&str),
) -> Result<(S, oxp_clink::ble::HeadUnitReply)> {
    let mut rejected = false;
    loop {
        let mut phone = config.phone(mac.into());
        phone.kind = if policy.use_1002 { 1002 } else { 1001 };
        let payload = oxp_clink::ble::phone_request(&phone)?;
        diagnostic(if policy.use_1002 {
            "request_type:1002"
        } else {
            "request_type:1001"
        });
        loop {
            let mut socket = connect()?;
            socket.set_deadline(end);
            match att::handoff(&mut socket, &payload, policy.legacy, end, diagnostic) {
                Ok(reply) => return Ok((socket, reply)),
                Err(e) => {
                    // A timed out ATT request remains outstanding. Drop the bearer before retrying.
                    drop(socket);
                    if !policy.legacy && e.to_string() == "bluetooth_cccd_timeout" {
                        diagnostic("subscription_cccd_timeout_reconnect");
                        policy.legacy = true;
                        continue;
                    }
                    if e.to_string() == "head_unit_rejected_request" {
                        policy.use_1002 = !policy.use_1002;
                        if config.state_dir.is_some()
                            && !rejected
                            && std::time::Instant::now() < end
                        {
                            rejected = true;
                            break;
                        }
                    }
                    return Err(e);
                }
            }
        }
    }
}

#[cfg(target_os = "linux")]
struct Active {
    network: Arc<network::Network>,
    stop: Arc<AtomicBool>,
}
pub struct LinuxClinkLink {
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    config: Config,
    address: Option<String>,
    listener: Option<LinkListener>,
    #[cfg(target_os = "linux")]
    backoff: std::sync::Mutex<LinkBackoff>,
    #[cfg(target_os = "linux")]
    active: Mutex<Option<Active>>,
}
impl LinuxClinkLink {
    pub fn new(config: Config) -> Result<Self> {
        config.validate()?;
        #[cfg(target_os = "linux")]
        let backoff = std::sync::Mutex::new(LinkBackoff::with_initial_delay(config.retry_delay));
        Ok(Self {
            config,
            address: None,
            listener: None,
            #[cfg(target_os = "linux")]
            backoff,
            #[cfg(target_os = "linux")]
            active: Mutex::new(None),
        })
    }
    pub fn with_address(mut self, address: String) -> Result<Self> {
        mac_bytes(&address)?;
        self.address = Some(address);
        Ok(self)
    }
    pub fn with_listener(mut self, listener: LinkListener) -> Self {
        self.listener = Some(listener);
        self
    }
    fn notify(&self, event: LinkEvent) {
        if let Some(listener) = &self.listener {
            listener(event);
        }
    }
    fn report_connect_result<T>(&self, result: Result<T>) -> Result<T> {
        if let Err(error) = &result {
            let reason = diagnostics::code(error.as_ref());
            self.notify(LinkEvent::Lost { reason });
        }
        result
    }
    pub fn scan(&self) -> Result<Vec<Device>> {
        self.notify(LinkEvent::Scanning);
        #[cfg(target_os = "linux")]
        {
            self.scan_selected(None, None)
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err("linux_link_unavailable".into())
        }
    }
    #[cfg(target_os = "linux")]
    fn scan_selected(&self, address: Option<&str>, car_id: Option<&str>) -> Result<Vec<Device>> {
        let peers = self.load_peers()?;
        bluez::scan(
            &self.config,
            address,
            car_id,
            &mut |device| {
                self.notify(LinkEvent::Advertisement {
                    serial_same: peers.serial_same(device),
                    advertisement: device.advertisement.clone(),
                });
            },
            &mut |event| self.notify(event),
        )
    }
    #[cfg(target_os = "linux")]
    fn load_peers(&self) -> Result<peers::Peers> {
        if let Some(directory) = &self.config.state_dir {
            peers::Peers::load(directory)
        } else {
            Ok(peers::memory()
                .lock()
                .map_err(|_| "link_lock_poisoned")?
                .clone())
        }
    }
    /// The address must be seen in a new scan, even when explicitly selected.
    pub fn connect_address(
        &self,
        address: Option<&str>,
        on_link_lost: Arc<dyn Fn(Option<String>) + Send + Sync>,
    ) -> Result<ClinkAddresses> {
        let started = std::time::Instant::now();
        let result = diagnostics::diagnosed(
            "link_start",
            0,
            started,
            &mut |event| self.notify(event),
            || self.try_connect_address(address, on_link_lost, started),
        );
        self.report_connect_result(result)
    }
    fn try_connect_address(
        &self,
        address: Option<&str>,
        on_link_lost: Arc<dyn Fn(Option<String>) + Send + Sync>,
        started: std::time::Instant,
    ) -> Result<ClinkAddresses> {
        #[cfg(target_os = "linux")]
        {
            let mut active = self.active.lock().map_err(|_| "link_lock_poisoned")?;
            if active.is_some() {
                return Err("link_already_connected".into());
            }
            if let Some(address) = address {
                mac_bytes(address)?;
            }
            network::leave(&self.config)?;
            let (device, socket, reply, attempt) = self.handoff(address, started)?;
            self.notify(LinkEvent::Trace {
                stage: "ble_exchange_done".into(),
                fields: [
                    ("frequency".into(), reply.freq.to_string()),
                    ("replyType".into(), reply.kind.to_string()),
                ]
                .into(),
            });
            // Join immediately: the head unit's network may expire quickly.
            let (network, addresses) = diagnostics::diagnosed(
                "wifi_join",
                attempt,
                started,
                &mut |event| self.notify(event),
                || {
                    join_after_handoff(&self.config, socket, &reply, || {
                        network::join(
                            &self.config,
                            &reply,
                            device.advertisement.car_id.clone(),
                            &mut |event| self.notify(event),
                        )
                    })
                },
            )?;
            let network = Arc::new(network);
            let stop = Arc::new(AtomicBool::new(false));
            let monitor_network = network.clone();
            let monitor_stop = stop.clone();
            let expected = addresses.clone();
            let interval = self.config.link_check_interval;
            let listener = self.listener.clone();
            let worker = std::thread::Builder::new()
                .name("oxp-clink-link".into())
                .spawn(move || {
                    loop {
                        // Short sleeps let explicit disconnect silence the monitor promptly.
                        let next = Instant::now() + interval;
                        while Instant::now() < next {
                            if monitor_stop.load(Ordering::Acquire) {
                                return;
                            }
                            std::thread::sleep(
                                Duration::from_millis(100)
                                    .min(next.saturating_duration_since(Instant::now())),
                            );
                        }
                        if let Err(error) = monitor_network.alive(&expected) {
                            if !monitor_stop.swap(true, Ordering::AcqRel) {
                                let result = diagnostics::diagnosed::<()>(
                                    "link_lost",
                                    attempt,
                                    started,
                                    &mut |event| {
                                        if let Some(listener) = &listener {
                                            listener(event);
                                        }
                                    },
                                    || Err(error),
                                );
                                let reason = result.unwrap_err().to_string();
                                if let Some(listener) = &listener {
                                    listener(LinkEvent::Lost {
                                        reason: reason.clone(),
                                    });
                                }
                                on_link_lost(Some(reason));
                            }
                            return;
                        }
                    }
                });
            if let Err(error) = worker {
                let _ = network.leave();
                return diagnostics::diagnosed(
                    "link_monitor",
                    attempt,
                    started,
                    &mut |event| self.notify(event),
                    || Err(error.into()),
                );
            }
            self.backoff
                .lock()
                .map_err(|_| "link_lock_poisoned")?
                .succeeded();
            self.notify(LinkEvent::Joined {
                address: device.address,
                name: device.advertisement.name,
                ssid: reply.ssid,
                interface: network.interface().into(),
                addresses: addresses.clone(),
            });
            *active = Some(Active { network, stop });
            Ok(addresses)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (address, on_link_lost, started);
            Err("linux_link_unavailable".into())
        }
    }
    #[cfg(target_os = "linux")]
    fn handoff(
        &self,
        address: Option<&str>,
        started: Instant,
    ) -> Result<(
        Device,
        socket::NativeSocket,
        oxp_clink::ble::HeadUnitReply,
        u32,
    )> {
        retry::discover(
            self.config.connect_attempts,
            |attempt, car_id| {
                self.notify(LinkEvent::Scanning);
                let device = diagnostics::diagnosed(
                    "ble_scan",
                    attempt,
                    started,
                    &mut |event| self.notify(event),
                    || {
                        // A remembered car identity follows rotating addresses; never reuse the old address.
                        let devices = self.scan_selected(
                            if car_id.is_some() { None } else { address },
                            car_id.as_deref(),
                        )?;
                        retry::select(devices, address, car_id.as_deref())
                    },
                )?;
                *car_id = Some(device.advertisement.car_id.clone());
                self.notify(LinkEvent::Connecting {
                    address: device.address.clone(),
                    name: device.advertisement.name.clone(),
                });
                let (local, mac) = diagnostics::diagnosed(
                    "link_start",
                    attempt,
                    started,
                    &mut |event| self.notify(event),
                    || {
                        let local = bluez::adapter_address(&self.config)?;
                        let mac = if let Some(mac) = &self.config.phone_mac {
                            normalized_bluetooth_address(mac)?
                        } else {
                            std::fs::read_to_string(format!(
                                "/sys/class/net/{}/address",
                                self.config.wifi_interface
                            ))?
                        };
                        mac_bytes(mac.trim())?;
                        Ok((local, mac))
                    },
                )?;
                let (socket, reply) =
                    self.exchange_device(&device, &local, mac.trim(), attempt, started)?;
                Ok((device, socket, reply, attempt))
            },
            || self.wait_retry(),
        )
    }
    #[cfg(target_os = "linux")]
    fn exchange_device(
        &self,
        device: &Device,
        local: &str,
        mac: &str,
        attempt: u32,
        started: Instant,
    ) -> Result<(socket::NativeSocket, oxp_clink::ble::HeadUnitReply)> {
        let mut peers = if self.config.state_dir.is_some() {
            self.load_peers()?
        } else {
            // The static map preserves the original behaviour without a state directory.
            peers::Peers::default()
        };
        let key = device_key(device);
        let policies = peers::memory();
        let mut policy = policies
            .lock()
            .map_err(|_| "link_lock_poisoned")?
            .known_choice(&key)
            .unwrap_or_else(|| peers.choice(&key, self.config.state_dir.is_some()));
        let persisted_legacy = self.config.state_dir.is_some()
            && peers.known_choice(&key).is_some_and(|choice| choice.legacy);
        let end = Instant::now() + self.config.handoff_timeout.min(Duration::from_secs(15));
        let result = diagnostics::diagnosed(
            "ble_exchange",
            attempt,
            started,
            &mut |event| self.notify(event),
            || {
                exchange(
                    &self.config,
                    mac.trim(),
                    &mut policy,
                    end,
                    || {
                        diagnostics::diagnosed(
                            "gatt_connect",
                            attempt,
                            started,
                            &mut |event| self.notify(event),
                            || {
                                socket::NativeSocket::connect(
                                    local,
                                    &device.address,
                                    self.config.address_type,
                                    end.min(Instant::now() + self.config.connect_timeout),
                                )
                            },
                        )
                    },
                    &mut |reason| {
                        self.notify(LinkEvent::Diagnostic {
                            reason: reason.into(),
                        })
                    },
                )
            },
        );
        if result.is_err() && persisted_legacy {
            policy.legacy = false;
        }
        if let Some(directory) = &self.config.state_dir {
            if result.is_ok() || persisted_legacy {
                // A rejection toggles only the in-process retry, never the saved successful type.
                let saved = if result.is_ok() {
                    policy
                } else {
                    HandoffPolicy {
                        legacy: false,
                        ..peers.choice(&key, true)
                    }
                };
                peers.remember(
                    key.clone(),
                    saved,
                    result.as_ref().ok().map(|_| device.advertisement.serial),
                );
                if peers.save(directory).is_err() {
                    self.notify(LinkEvent::Diagnostic {
                        reason: "ble_peer_save_failed".into(),
                    });
                }
            }
        }
        policies.lock().map_err(|_| "link_lock_poisoned")?.remember(
            key,
            policy,
            result.as_ref().ok().map(|_| device.advertisement.serial),
        );
        result
    }
    #[cfg(target_os = "linux")]
    fn wait_retry(&self) -> Result<()> {
        let delay = self
            .backoff
            .lock()
            .map_err(|_| "link_lock_poisoned")?
            .next_delay();
        self.notify(retry::retry_event(delay));
        let end = std::time::Instant::now() + delay;
        while std::time::Instant::now() < end {
            #[cfg(target_os = "linux")]
            signals::check()?;
            std::thread::sleep(
                Duration::from_millis(50)
                    .min(end.saturating_duration_since(std::time::Instant::now())),
            );
        }
        Ok(())
    }
    /// Remove this crate's temporary profiles on the configured Wi-Fi interface.
    pub fn leave(&self) -> Result<()> {
        self.disconnect()?;
        #[cfg(target_os = "linux")]
        {
            network::leave(&self.config)
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err("linux_link_unavailable".into())
        }
    }
}
impl ClinkLink for LinuxClinkLink {
    fn connect(
        &self,
        on_link_lost: Arc<dyn Fn(Option<String>) + Send + Sync>,
    ) -> Result<ClinkConnection> {
        self.connect_address(self.address.as_deref(), on_link_lost)
            .map(Into::into)
    }
    fn disconnect(&self) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            let mut active = self.active.lock().map_err(|_| "link_lock_poisoned")?;
            if let Some(a) = active.as_ref() {
                a.stop.store(true, Ordering::Release);
                a.network.leave()?;
            }
            *active = None;
        }
        Ok(())
    }
}
impl Drop for LinuxClinkLink {
    fn drop(&mut self) {
        let _ = self.disconnect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn device(address: &str) -> Device {
        Device {
            address: address.into(),
            advertisement: Advertisement {
                major: 1,
                minor: 0,
                serial: 0,
                car_id: "car".into(),
                model_id: "model".into(),
                vendor_data: String::new(),
                name: "unit".into(),
            },
        }
    }
    struct TrackedBle {
        fake: att::tests::Fake,
        history: std::rc::Rc<std::cell::RefCell<Vec<Vec<Vec<u8>>>>>,
        deadlines: std::rc::Rc<std::cell::RefCell<Vec<std::time::Instant>>>,
    }
    impl att::Socket for TrackedBle {
        fn send(&mut self, pdu: &[u8]) -> Result<()> {
            self.fake.send(pdu)
        }
        fn receive(&mut self) -> Result<Vec<u8>> {
            self.fake.receive()
        }
        fn set_deadline(&mut self, deadline: std::time::Instant) {
            self.deadlines.borrow_mut().push(deadline);
        }
    }
    impl Drop for TrackedBle {
        fn drop(&mut self) {
            self.history.borrow_mut().push(self.fake.sent.clone());
        }
    }
    #[test]
    fn cccd_timeout_drops_bearer_before_reconnect_and_remembers_legacy() {
        use att::tests::{cccd_discovery, discovery, Fake, JSON};
        use std::{cell::RefCell, rc::Rc, time::Instant};
        let history = Rc::new(RefCell::new(Vec::new()));
        let deadlines = Rc::new(RefCell::new(Vec::new()));
        let mut first = discovery(512);
        first.extend(cccd_discovery());
        first.push_back(vec![]); // Simulate the CCCD response deadline.
        let mut legacy = discovery(512);
        legacy.extend([vec![0x13], [vec![0x1d, 5, 0], JSON.to_vec()].concat()]);
        let mut scripts = [first, legacy.clone(), legacy].into_iter();
        let mut count = 0;
        let mut connect = || {
            assert_eq!(history.borrow().len(), count);
            count += 1;
            Ok(TrackedBle {
                fake: Fake {
                    replies: scripts.next().unwrap(),
                    sent: Vec::new(),
                    echo: false,
                },
                history: history.clone(),
                deadlines: deadlines.clone(),
            })
        };
        let mut policy = HandoffPolicy::default();
        let end = Instant::now() + Duration::from_secs(15);
        let mut events = Vec::new();
        let (socket, _) = exchange(
            &Config::default(),
            "aa:bb:cc:dd:ee:ff",
            &mut policy,
            end,
            &mut connect,
            &mut |s| events.push(s.to_owned()),
        )
        .unwrap();
        assert!(policy.legacy);
        assert_eq!(history.borrow()[0].last().unwrap(), &[0x12, 6, 0, 2, 0]);
        assert!(deadlines
            .borrow()
            .iter()
            .any(|d| *d < end
                && d.saturating_duration_since(Instant::now()) <= Duration::from_secs(2)));
        drop(socket);
        let (socket, _) = exchange(
            &Config::default(),
            "aa:bb:cc:dd:ee:ff",
            &mut policy,
            end,
            &mut connect,
            &mut |_| {},
        )
        .unwrap();
        drop(socket);
        for sent in &history.borrow()[1..] {
            assert!(!sent
                .iter()
                .any(|p| p[0] == 4 || (p[0] == 0x12 && p[1] == 6)));
        }
        assert!(events
            .iter()
            .any(|e| e == "subscription_cccd_timeout_reconnect"));
    }
    #[test]
    fn rejection_switches_next_request_to_1002_and_softap_joins() {
        use att::tests::{discovery, discovery_with_service_changed, Fake, JSON};
        use std::{cell::RefCell, rc::Rc, time::Instant};
        let mut first = discovery_with_service_changed();
        first.push_back(vec![0x1d, 22, 0, 1, 0, 255, 255]);
        let softap = String::from_utf8(JSON.to_vec())
            .unwrap()
            .replace("1001", "1002")
            .replace("5180", "2412")
            .replace("WiFi", "Car hotspot");
        let mut second = discovery(512);
        second.extend([vec![0x13], [vec![0x1d, 5, 0], softap.into_bytes()].concat()]);
        let history = Rc::new(RefCell::new(Vec::new()));
        let mut scripts = [first, second].into_iter();
        let mut connect = || {
            Ok(TrackedBle {
                fake: Fake {
                    replies: scripts.next().unwrap(),
                    sent: Vec::new(),
                    echo: false,
                },
                history: history.clone(),
                deadlines: Rc::new(RefCell::new(Vec::new())),
            })
        };
        let mut policy = HandoffPolicy {
            legacy: true,
            ..HandoffPolicy::default()
        };
        let config = Config {
            band: 0,
            ..Config::default()
        };
        let end = Instant::now() + Duration::from_secs(15);
        let error = match exchange(
            &config,
            "aa:bb:cc:dd:ee:ff",
            &mut policy,
            end,
            &mut connect,
            &mut |_| {},
        ) {
            Err(e) => e,
            Ok(_) => panic!("expected rejection"),
        };
        assert_eq!(error.to_string(), "head_unit_rejected_request");
        assert!(policy.use_1002);
        let (socket, reply) = exchange(
            &config,
            "aa:bb:cc:dd:ee:ff",
            &mut policy,
            end,
            &mut connect,
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(reply.kind, 1002);
        assert_eq!(reply.ssid, "Car hotspot");
        join_after_handoff(&config, socket, &reply, || {
            assert_eq!(history.borrow().len(), 2);
            Ok(())
        })
        .unwrap();
        for (sent, kind) in history.borrow().iter().zip([1001, 1002]) {
            let write = sent.iter().find(|p| p[0] == 0x12 && p[1] == 3).unwrap();
            assert!(std::str::from_utf8(&write[3..])
                .unwrap()
                .ends_with(&format!("\"type\":{kind}}}")));
        }
        let mut unsupported = reply;
        unsupported.freq = 5180;
        assert_eq!(
            join_after_handoff(&config, (), &unsupported, || -> Result<()> {
                panic!("unsupported SoftAP joined")
            })
            .unwrap_err()
            .to_string(),
            "head_unit_band_unsupported:5180"
        );
    }
    #[test]
    fn phone_mac_matches_android_validation() {
        assert_eq!(
            normalized_bluetooth_address("AA:BB:CC:DD:EE:FF").unwrap(),
            "aa:bb:cc:dd:ee:ff"
        );
        for value in [
            "",
            "00:00:00:00:00:00",
            "02:00:00:00:00:00",
            "FF:FF:FF:FF:FF:FF",
            "01:12:34:56:78:90",
            "aa:bb:cc:dd:ee",
            "aa:bb:cc:dd:ee:gg",
            "+2:00:00:00:00:01",
            " aa:bb:cc:dd:ee:ff",
        ] {
            assert!(normalized_bluetooth_address(value).is_err(), "{value}");
        }
        let config = Config {
            phone_mac: Some("12:34:56:78:9a:bc".into()),
            ..Config::default()
        };
        config.validate().unwrap();
        assert_eq!(
            config.phone("00:11:22:33:44:55".into()).mac,
            "12:34:56:78:9a:bc"
        );
    }
    #[test]
    fn persistent_default_rejection_retries_the_other_type_once() {
        use att::tests::{discovery, discovery_with_service_changed, Fake, JSON};
        let mut rejected = discovery_with_service_changed();
        rejected.push_back(vec![0x1d, 22, 0, 1, 0, 255, 255]);
        let mut accepted = discovery(512);
        accepted.extend([vec![0x13], [vec![0x1d, 5, 0], JSON.to_vec()].concat()]);
        let mut scripts = [rejected, accepted].into_iter();
        let mut choice = HandoffPolicy {
            legacy: true,
            use_1002: true,
        };
        let config = Config {
            state_dir: Some("unused".into()),
            ..Config::default()
        };
        exchange(
            &config,
            "aa:bb:cc:dd:ee:ff",
            &mut choice,
            std::time::Instant::now() + Duration::from_secs(15),
            || {
                Ok(Fake {
                    replies: scripts.next().unwrap(),
                    sent: vec![],
                    echo: false,
                })
            },
            &mut |_| {},
        )
        .unwrap();
        assert!(!choice.use_1002);
    }
    #[test]
    fn policy_key_prefers_car_identity_over_address() {
        let mut a = device("AA:BB:CC:DD:EE:01");
        let b = device("AA:BB:CC:DD:EE:02");
        assert_eq!(device_key(&a), device_key(&b));
        a.advertisement.car_id.clear();
        assert_eq!(device_key(&a).len(), 64);
        assert_ne!(device_key(&a), device_key(&b));
    }
    #[test]
    fn band_guard_disconnects_ble_before_join_and_never_joins_unsupported_groups() {
        use std::{cell::Cell, sync::Mutex};
        struct Ble<'a>(&'a Cell<bool>);
        impl Drop for Ble<'_> {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }
        for band in [0, 1] {
            for freq in [
                0, 2399, 2400, 2412, 2500, 2501, 4999, 5000, 5180, 5999, 6000, 6500,
            ] {
                let config = Config {
                    band,
                    ..Config::default()
                };
                let reply = oxp_clink::ble::head_unit_reply(format!(
                    r#"{{"name":"unit","ssid":"DIRECT-unit","psk":"secret-password","mac":"aa:bb:cc:dd:ee:ff","freq":{freq},"port":57209,"type":1001}}"#
                ).as_bytes()).unwrap();
                let disconnected = Cell::new(false);
                let joined = Cell::new(false);
                let events = Arc::new(Mutex::new(Vec::new()));
                let sink = events.clone();
                let link = LinuxClinkLink::new(config.clone())
                    .unwrap()
                    .with_listener(Arc::new(move |event| sink.lock().unwrap().push(event)));
                let result = link.report_connect_result(join_after_handoff(
                    &config,
                    Ble(&disconnected),
                    &reply,
                    || {
                        assert!(disconnected.get());
                        joined.set(true);
                        Ok(())
                    },
                ));
                let supported =
                    (2400..=2500).contains(&freq) || (band == 1 && (5000..6000).contains(&freq));
                assert!(disconnected.get());
                assert_eq!(joined.get(), supported);
                assert_eq!(result.is_ok(), supported);
                let events = events.lock().unwrap();
                if supported {
                    assert!(events.is_empty());
                } else {
                    let expected = format!("head_unit_band_unsupported:{freq}");
                    assert_eq!(result.unwrap_err().to_string(), expected);
                    assert_eq!(events.len(), 1);
                    assert!(
                        matches!(&events[0], LinkEvent::Lost { reason } if reason == &expected)
                    );
                }
            }
        }
    }
    #[test]
    fn unrelated_connect_errors_do_not_expose_credentials() {
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = events.clone();
        let link = LinuxClinkLink::new(Config::default())
            .unwrap()
            .with_listener(Arc::new(move |event| sink.lock().unwrap().push(event)));
        let _ = link.report_connect_result::<()>(Err("secret-password".into()));
        assert!(
            matches!(&events.lock().unwrap()[0], LinkEvent::Lost { reason } if reason == "unknown_error")
        );
    }
    #[test]
    fn selected_address_never_falls_back_to_another_head_unit() {
        let a = "AA:BB:CC:DD:EE:01";
        let b = "AA:BB:CC:DD:EE:02";
        assert_eq!(
            select_device(vec![device(a), device(b)], Some(&b.to_lowercase()))
                .unwrap()
                .address,
            b
        );
        assert!(select_device(vec![device(a)], Some(b)).is_err());
        assert!(select_device(vec![device(a), device(b)], None).is_err());
        assert!(select_device(vec![], None).is_err());
        assert_eq!(select_device(vec![device(a)], None).unwrap().address, a);
    }
    #[test]
    fn address_builder_validates_before_hardware_access() {
        assert!(LinuxClinkLink::new(Config::default())
            .unwrap()
            .with_address("bad".into())
            .is_err());
        let link = LinuxClinkLink::new(Config::default())
            .unwrap()
            .with_address("AA:BB:CC:DD:EE:FF".into())
            .unwrap();
        assert_eq!(link.address.as_deref(), Some("AA:BB:CC:DD:EE:FF"));
    }
}
