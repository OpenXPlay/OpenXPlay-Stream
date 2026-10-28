// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

#![forbid(unsafe_code)]

#[path = "../clink_identity.rs"]
mod clink_identity;

use oxp_bin::{
    clink_control::read_control_with,
    clink_trace::{SessionTrace, SummaryTrace, TraceRecorder},
};
use oxp_bin::{clink_support::*, *};
use oxp_clink::trace::{now_nanos, ClinkTrace, ClinkTraceEvent};
use oxp_clink::{
    ClinkAddresses, ClinkLink, ClinkListener, ClinkOutput, ClinkState, ClinkStatistics,
    StaticClinkLink,
};
use oxp_core::{Hub, ProjectionOutput};
use oxp_web::{Pairing, Tls, WebOutput, WebOutputConfig};
use oxp_wire::{Message, Result};
use serde_json::json;
use std::{
    net::Ipv4Addr,
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{sync_channel, RecvTimeoutError},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};
const USAGE: &str = "oxp-relay (--unix PATH | --stdio) [--relay-name NAME --relay-version VERSION] [--web-port N --web-root DIR [--web-bind ADDR] [--web-transcode auto|off] [--plugins-root DIR --plugins-enabled IDS --plugins-state-dir DIR [--plugins-topics a,b,c]] [--advertise-address IPV4] [--pairing] [--token T] [--source-url URL] (--tls-cert PEM --tls-key PEM [--tls-name NAME --tls-self-signed STATE_DIR [--tls-san NAME]...] | --tls-self-signed STATE_DIR [--tls-san NAME]... | --no-tls)] [--clink-link static|ble --state-dir DIR [--pin DIGITS | --pin-file PATH] [--port k=v,...] [--clink-transcode auto|off] [--clink-hevc auto|off|signal] [--clink-vehicle-data on|off] [--clink-trace DIR]] [--control-stdin]\nStatic (default): --clink-head-unit IP --clink-local IP. BLE: [--clink-adapter NAME (hci0)] [--clink-interface NAME (wlan0)] [--clink-address MAC] [--clink-name NAME] [--clink-model MODEL] [--clink-id ID] [--clink-band 0|1] [--clink-p2p-reserved on|off]. --clink-state-dir DIR saves handoff choices in DIR/carlink/ble-peers.json; absent keeps in-memory choices. --clink-scan window|continuous defaults to an 8 s window; continuous waits for the first match and renews discovery every 10 min. --clink-phone-mac MAC sends a validated unicast Bluetooth address instead of the Wi-Fi MAC. --clink-identity ble|legacy defaults to ble for BLE links: ble uses --clink-id/name for AUTH and keeps separate car trust; legacy restores the original AUTH defaults and pairing. BLE forbids static addresses. --clink-p2p-reserved defaults to off; the host must pass on while CarPlay reserves Wi-Fi Direct. Otherwise P2P requires supplicant D-Bus access, iw-confirmed interface/channel capacity and BusyBox udhcpc/ifconfig/mv; failures clean up before infrastructure fallback. --pin-file clears stale input and waits up to 120 seconds per request. Replacement pairing requires explicit approval through --control-stdin. Incorrect PINs emit type=clink-pin, result=incorrect; session diagnostics emit type=diagnostic. Events use stdout with --unix, stderr with --stdio.\n--clink-trace DIR records Carlink JSONL and enables TS capture; --control-stdin reads JSON clear/capture, web-tls, clink-restart and clink-pairing commands and requires --unix.\nweb-tls replaces the trusted identity and self-signed SANs for new handshakes without disconnecting clients; requires --tls-self-signed at startup. Send one JSON line (4096 bytes maximum): {\"type\":\"web-tls\",\"id\":\"revision\",\"trusted\":{\"name\":\"relay.example\",\"cert\":\"/cert.pem\",\"key\":\"/key.pem\"},\"sans\":[\"192.168.1.1\"]}. trusted:null removes trusted TLS; sans is required (at most 24 DNS/IP names). Replies echo id with type=web-tls, applied=true and the served fingerprints/name, or error; advertise the new name only after success.\n--clink-hevc auto (default) uses H.265 for head units advertising iccoa_support_hevc: supported; off uses H.264; signal also sends the M4 support flag. The session codec stays fixed until reconnect.\nclink-restart replaces only Carlink and keeps web running; omitted picture/hevc/vehicleData settings retain their current choices. phoneMac is a unicast Bluetooth address validated like --clink-phone-mac: a string replaces the override before the next BLE link/handshake, null clears it to the Wi-Fi MAC, and absence retains the current choice. Invalid settings reject the command before retiring Carlink. clink-pairing answers the matching request with approve, pin or cancel. --clink-vehicle-data on|off defaults to off.\n--clink-transcode auto (default) uses a suitable V4L2 M2M encoder for Carlink codec/profile conversion and active-area crop/stretch with touch remapping. A clink-restart picture fixes the outgoing frame size (for example 1280x720), independently of the input canvas. H.264 input can become H.265 for an HEVC Carlink session while web keeps H.264. off forwards unchanged.\n--web-transcode auto (default) converts incompatible iPhone codecs/profiles (including H.265 to H.264) for the browser when a suitable encoder exists; off forwards unchanged. Without an encoder both outputs retain passthrough behavior. type=transcode-capabilities events at web startup and each Carlink generation (including off) report output, enabled, available, decode, encode (codec/maxWidth/maxHeight), scaling and description; null maxima mean unknown; available reports a usable hardware pair, with actual conversion validated at runtime.\nBrowser socket termination emits type=receiver-disconnected with client, code and reason (closed, timeout or error), using the receiver-info client ID. code retains the received/server close code; 1005 means no received code, 1006 means abnormal closure, 1008/timeout means hello timeout, and 1011/timeout means stalled writing. Peer close text is not reported; established idle pages do not time out.\n--advertise-address overrides the IPv4 address in the pairing URL. Otherwise prefer the default-route interface, then Wi-Fi/Ethernet, then private address range.\n--tls-name requires PEM and self-signed TLS together: matching SNI uses PEM, ignoring ASCII case and a trailing dot; all other clients use self-signed TLS. NAME must be a DNS name with LDH labels, at most 253 characters. Unloadable PEM falls back to self-signed TLS. Without --tls-name, PEM is served to every client.\n--host-ui advertises receiver UI support and forwards one-shot host-ui requests to the recent touch client (5 s), otherwise all clients.\nPairing is off by default. --pairing generates an 8-digit code; --token T enables pairing with T and takes precedence over --pairing.";
fn arguments() -> Result<(Args, Vec<String>)> {
    parse_arguments(std::env::args().skip(1))
}
fn parse_arguments(raw: impl IntoIterator<Item = String>) -> Result<(Args, Vec<String>)> {
    let mut raw = raw.into_iter();
    let mut values = Vec::new();
    let mut sans = Vec::new();
    while let Some(k) = raw.next() {
        if k == "--tls-san" {
            let v = raw
                .next()
                .filter(|v| !v.starts_with("--"))
                .ok_or("Missing --tls-san name")?;
            sans.push(v);
        } else {
            values.push(k);
        }
    }
    let args = Args::parse_from(
        values,
        &[
            "--unix",
            "--web-port",
            "--web-root",
            "--plugins-root",
            "--plugins-enabled",
            "--plugins-topics",
            "--plugins-state-dir",
            "--web-bind",
            "--web-transcode",
            "--advertise-address",
            "--token",
            "--pairing",
            "--source-url",
            "--host-ui",
            "--relay-name",
            "--relay-version",
            "--tls-cert",
            "--tls-key",
            "--tls-name",
            "--tls-self-signed",
            "--no-tls",
            "--clink-head-unit",
            "--clink-local",
            "--pin",
            "--pin-file",
            "--clink-link",
            "--clink-adapter",
            "--clink-interface",
            "--clink-p2p-reserved",
            "--clink-address",
            "--clink-name",
            "--clink-model",
            "--clink-id",
            "--clink-identity",
            "--clink-band",
            "--clink-state-dir",
            "--clink-scan",
            "--clink-phone-mac",
            "--state-dir",
            "--port",
            "--clink-transcode",
            "--clink-hevc",
            "--clink-vehicle-data",
            "--clink-trace",
            "--control-stdin",
        ],
    )?;
    if args.get("--relay-name").is_some() != args.get("--relay-version").is_some()
        || ["--relay-name", "--relay-version"].iter().any(|k| {
            args.get(k).is_some_and(|v| {
                v.trim().is_empty() || v.len() > 128 || v.chars().any(char::is_control)
            })
        })
    {
        return Err(
            "Supply both --relay-name and --relay-version (1–128 bytes, no control characters)"
                .into(),
        );
    }
    advertise_address(&args)?;
    web_transcode_enabled(args.get("--web-transcode"))?;
    if args.get("--web-transcode").is_some() && args.get("--web-port").is_none() {
        return Err("--web-transcode requires --web-port".into());
    }
    if args.get("--control-stdin").is_some() && args.get("--unix").is_none() {
        return Err("--control-stdin requires --unix".into());
    }
    if args
        .get("--clink-trace")
        .is_some_and(|v| v.trim().is_empty())
    {
        return Err("--clink-trace requires a nonempty directory".into());
    }
    Ok((args, sans))
}
#[cfg(test)]
fn clink_link(args: &Args) -> Result<Option<Arc<dyn ClinkLink>>> {
    clink_link_factory(args)?
        .map(|build| build(Arc::new(|_| {}), clink_phone_mac(args)?))
        .transpose()
}
// Each generation gets its own link and listener, including the Linux monitor's late callbacks.
type LinkFactory = Arc<
    dyn Fn(oxp_link_linux::LinkListener, Option<String>) -> Result<Arc<dyn ClinkLink>>
        + Send
        + Sync,
>;
fn clink_link_factory(args: &Args) -> Result<Option<LinkFactory>> {
    let mode = args.get("--clink-link").unwrap_or("static");
    if !["static", "ble"].contains(&mode) {
        return Err("--clink-link must be static or ble".into());
    }
    let ble_options = [
        "--clink-adapter",
        "--clink-interface",
        "--clink-p2p-reserved",
        "--clink-address",
        "--clink-name",
        "--clink-model",
        "--clink-id",
        "--clink-band",
        "--clink-state-dir",
        "--clink-scan",
        "--clink-phone-mac",
    ];
    if mode == "static" && ble_options.iter().any(|k| args.get(k).is_some()) {
        return Err("BLE options require --clink-link ble".into());
    }
    let enabled = mode == "ble"
        || args.get("--clink-link").is_some()
        || args.get("--clink-head-unit").is_some()
        || args.get("--clink-local").is_some();
    if !enabled {
        if [
            "--pin",
            "--pin-file",
            "--clink-identity",
            "--state-dir",
            "--port",
            "--clink-transcode",
            "--clink-hevc",
            "--clink-vehicle-data",
            "--clink-trace",
        ]
        .iter()
        .any(|k| args.get(k).is_some())
        {
            return Err("Clink options require static addresses or --clink-link ble".into());
        }
        return Ok(None);
    }
    if args.get("--pin").is_some() && args.get("--pin-file").is_some() {
        return Err("Choose --pin or --pin-file".into());
    }
    if args
        .get("--pin")
        .is_some_and(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err("PIN must contain digits".into());
    }
    if args.get("--pin-file").is_some_and(str::is_empty) {
        return Err("--pin-file requires a nonempty path".into());
    }
    if mode == "ble"
        && ["--clink-head-unit", "--clink-local"]
            .iter()
            .any(|k| args.get(k).is_some())
    {
        return Err("--clink-link ble forbids --clink-head-unit and --clink-local".into());
    }
    if mode == "static"
        && ["--clink-head-unit", "--clink-local"]
            .iter()
            .any(|k| args.get(k).is_none())
    {
        return Err("Static Clink requires --clink-head-unit and --clink-local".into());
    }
    args.need("--state-dir")?;
    clink_identity::Identity::from_args(args)?;
    parse_ports(args.get("--port"))?;
    transcode_enabled(args.get("--clink-transcode"))?;
    hevc_mode(args.get("--clink-hevc"))?;
    vehicle_enabled(args.get("--clink-vehicle-data"))?;
    if mode == "static" {
        let addresses = ClinkAddresses::new(
            args.need("--clink-head-unit")?.parse()?,
            args.need("--clink-local")?.parse()?,
        );
        return Ok(Some(Arc::new(move |_, _| {
            Ok(Arc::new(StaticClinkLink(addresses.clone())))
        })));
    }
    let config = ble_config(args)?;
    let address = args.get("--clink-address").map(str::to_owned);
    let build: LinkFactory = Arc::new(move |listener, phone_mac| {
        let mut config = config.clone();
        config.phone_mac = phone_mac;
        let mut link = oxp_link_linux::LinuxClinkLink::new(config)
            .map_err(|_| "Invalid BLE adapter, interface, or relay identity")?
            .with_listener(listener);
        if let Some(address) = &address {
            link = link
                .with_address(address.clone())
                .map_err(|_| "--clink-address must be a Bluetooth MAC address")?;
        }
        Ok(Arc::new(link))
    });
    // Reject invalid startup options before opening the wire or retiring any output.
    build(Arc::new(|_| {}), clink_phone_mac(args)?)?;
    Ok(Some(build))
}

fn ble_config(args: &Args) -> Result<oxp_link_linux::Config> {
    let mut config = oxp_link_linux::Config::default();
    for (option, field) in [
        ("--clink-adapter", &mut config.bluetooth_adapter),
        ("--clink-interface", &mut config.wifi_interface),
        ("--clink-name", &mut config.device_name),
        ("--clink-model", &mut config.device_model),
        ("--clink-id", &mut config.device_id),
    ] {
        if let Some(value) = args.get(option) {
            *field = value.into();
        }
    }
    // The head unit creates its Wi-Fi group on this band: 0 is 2.4 GHz, 1 allows 5 GHz.
    if let Some(band) = args.get("--clink-band") {
        config.band = match band {
            "0" => 0,
            "1" => 1,
            _ => return Err("--clink-band must be 0 (2.4 GHz) or 1 (5 GHz)".into()),
        };
    }
    config.state_dir = args.get("--clink-state-dir").map(Into::into);
    if config
        .state_dir
        .as_ref()
        .is_some_and(|p| p.as_os_str().is_empty())
    {
        return Err("--clink-state-dir requires a nonempty directory".into());
    }
    config.scan_mode =
        oxp_link_linux::ScanMode::parse(args.get("--clink-scan").unwrap_or("window"))?;
    config.phone_mac = clink_phone_mac(args)?;
    config.trace_advertisements = args.get("--clink-trace").is_some();
    config.p2p_reserved_by_carplay =
        oxp_link_linux::p2p_reserved(args.get("--clink-p2p-reserved").unwrap_or("off"))?;
    Ok(config)
}

fn clink_phone_mac(args: &Args) -> Result<Option<String>> {
    args.get("--clink-phone-mac")
        .map(oxp_link_linux::normalized_bluetooth_address)
        .transpose()
}

fn deliver_link(
    active: &Mutex<bool>,
    trace: &dyn ClinkTrace,
    generation: u64,
    state: oxp_link_linux::LinkEvent,
    emit: impl FnOnce(serde_json::Value),
) {
    let active = active.lock().unwrap();
    if *active {
        trace_link(trace, &state);
        let mut value = link_event(state);
        value["generation"] = json!(generation);
        emit(value);
    }
}

/// One bounded stdin reader dispatches TLS and session controls before trace/capture fallback.
fn relay_control(
    line: &[u8],
    web: Option<&WebOutput>,
    carlink: Option<&CarlinkRuntime>,
) -> Option<serde_json::Value> {
    if let Some(reply) = oxp_bin::web_control::handle(line, web) {
        return Some(reply);
    }
    let value: serde_json::Value = serde_json::from_slice(line).ok()?;
    if !matches!(
        value["type"].as_str(),
        Some("clink-restart" | "clink-pairing")
    ) {
        return None;
    }
    Some(match carlink {
        Some(carlink) => carlink.control(&value),
        None => json!({"type":value["type"],"request":value["request"],"error":"carlink_off"}),
    })
}

fn trace_link(trace: &dyn ClinkTrace, event: &oxp_link_linux::LinkEvent) {
    use oxp_link_linux::LinkEvent;
    let (stage, fields): (&str, std::collections::BTreeMap<String, String>) = match event {
        LinkEvent::Trace { stage, fields } => (stage, fields.clone()),
        LinkEvent::Failure(failure) => (
            "link_failure",
            [
                ("stage".into(), failure.stage.clone()),
                ("code".into(), failure.code.clone()),
                ("elapsedMs".into(), failure.elapsed_ms.to_string()),
                (
                    "totalElapsedMs".into(),
                    failure.total_elapsed_ms.to_string(),
                ),
                ("attempt".into(), failure.attempt.to_string()),
                (
                    "osCode".into(),
                    failure.os_code.map(|v| v.to_string()).unwrap_or_default(),
                ),
                (
                    "operation".into(),
                    failure.operation.clone().unwrap_or_default(),
                ),
                (
                    "status".into(),
                    failure.status.map(|v| v.to_string()).unwrap_or_default(),
                ),
            ]
            .into(),
        ),
        LinkEvent::Scanning => ("link_scanning", Default::default()),
        LinkEvent::Advertisement {
            advertisement: a,
            serial_same,
        } => (
            "ble_advertisement",
            [
                ("major".into(), a.major.to_string()),
                ("minor".into(), a.minor.to_string()),
                ("serial".into(), a.serial.to_string()),
                ("carId".into(), a.car_id.clone()),
                ("modelId".into(), a.model_id.clone()),
                ("vendorData".into(), a.vendor_data.clone()),
                ("name".into(), a.name.clone()),
                ("serialSame".into(), serial_same.to_string()),
            ]
            .into(),
        ),
        LinkEvent::Connecting { address, name } => (
            "link_connecting",
            [
                ("address".into(), address.clone()),
                ("name".into(), name.clone()),
            ]
            .into(),
        ),
        LinkEvent::Joined {
            address,
            name,
            ssid,
            interface,
            addresses,
        } => (
            "link_joined",
            [
                ("address".into(), address.clone()),
                ("name".into(), name.clone()),
                ("ssid".into(), ssid.clone()),
                ("interface".into(), interface.clone()),
                ("headUnitIp".into(), addresses.head_unit_ipv4.to_string()),
                ("localIp".into(), addresses.local_ipv4.to_string()),
            ]
            .into(),
        ),
        LinkEvent::Lost { reason } => ("link_lost", [("reason".into(), reason.clone())].into()),
        LinkEvent::Diagnostic { reason } => (
            "link_diagnostic",
            [("reason".into(), reason.clone())].into(),
        ),
    };
    trace.record(ClinkTraceEvent::Link {
        at_nanos: now_nanos(),
        session: 0,
        stage: stage.into(),
        fields,
    });
}

fn tls_from_args(args: &Args, sans: Vec<String>) -> Result<Tls> {
    let cert = args.get("--tls-cert");
    let key = args.get("--tls-key");
    let state_dir = args.get("--tls-self-signed");
    let no_tls = args.get("--no-tls").is_some();
    if cert.is_some() != key.is_some() {
        return Err("TLS requires --tls-cert and --tls-key".into());
    }
    if let Some(name) = args.get("--tls-name") {
        if !oxp_web::tls::valid_dns_name(name) {
            return Err(
                "--tls-name must be a DNS name with LDH labels, at most 253 characters".into(),
            );
        }
        if let (Some(cert), Some(key), Some(state_dir), false) = (cert, key, state_dir, no_tls) {
            return Ok(Tls::Sni {
                name: name.into(),
                cert: cert.into(),
                key: key.into(),
                state_dir: state_dir.into(),
                sans,
            });
        }
        return Err(
            "--tls-name requires --tls-cert, --tls-key and --tls-self-signed, without --no-tls"
                .into(),
        );
    }
    if cert.is_some() && state_dir.is_some() {
        return Err("Combining PEM and --tls-self-signed requires --tls-name".into());
    }
    if [cert.is_some(), state_dir.is_some(), no_tls]
        .into_iter()
        .filter(|v| *v)
        .count()
        != 1
    {
        return Err("Choose PEM TLS, self-signed TLS, or --no-tls".into());
    }
    if !sans.is_empty() && state_dir.is_none() {
        return Err("--tls-san requires --tls-self-signed".into());
    }
    if let (Some(cert), Some(key)) = (cert, key) {
        Ok(Tls::Pem {
            cert: cert.into(),
            key: key.into(),
        })
    } else if let Some(state_dir) = state_dir {
        Ok(Tls::SelfSigned {
            state_dir: state_dir.into(),
            sans,
        })
    } else {
        Ok(Tls::Disabled)
    }
}

fn pairing_from_args(args: &Args) -> Result<Pairing> {
    if let Some(token) = args.get("--token") {
        Ok(Pairing::Required(token.into()))
    } else if args.get("--pairing").is_some() {
        Ok(Pairing::Required(oxp_web::tls::random_token()?))
    } else {
        Ok(Pairing::Disabled)
    }
}

fn advertise_address(args: &Args) -> Result<Option<Ipv4Addr>> {
    args.get("--advertise-address")
        .map(|value| {
            let ip = value
                .parse::<Ipv4Addr>()
                .map_err(|_| "--advertise-address requires an IPv4 address")?;
            if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() || ip.is_broadcast() {
                return Err(
                    "--advertise-address requires a non-loopback unicast IPv4 address".into(),
                );
            }
            Ok(ip)
        })
        .transpose()
}

fn pairing_url(host: &str, tls: bool, pairing: &Pairing) -> String {
    let mut url = format!("{}://{host}/", if tls { "https" } else { "http" });
    if let Pairing::Required(token) = pairing {
        url.push_str("#k=");
        for b in token.bytes() {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                url.push(b as char);
            } else {
                url.push_str(&format!("%{b:02X}"));
            }
        }
    }
    url
}

type WireShutdown = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct RelayShutdown {
    stopped: Arc<AtomicBool>,
    session_failed: AtomicBool,
    keep_running: AtomicBool,
    generation: AtomicU64,
    session_trace: OnceLock<Arc<SessionTrace>>,
    ended: AtomicBool,
    has_output: AtomicBool,
    link: Mutex<Option<Arc<dyn ClinkLink>>>,
    output: Mutex<Option<Arc<ClinkOutput>>>,
    hub: Mutex<Option<Arc<Hub>>>,
    wire: Mutex<Option<WireShutdown>>,
    trace: OnceLock<Arc<TraceRecorder>>,
}
impl RelayShutdown {
    fn finish(&self) -> Result<()> {
        self.stopped.store(true, Ordering::Release);
        if let Some(hub) = self.hub.lock().unwrap().take() {
            hub.close();
        }
        if let Some(output) = self.output.lock().unwrap().take() {
            output.close();
            output.await_closed(Duration::from_millis(400));
        }
        let link = self.link.lock().unwrap().take();
        let cleanup = link.map(|link| link.disconnect());
        if let Some(shutdown) = self.wire.lock().unwrap().take() {
            shutdown();
        }
        self.emit_ended();
        if let Some(trace) = self.trace.get() {
            trace.shutdown();
        }
        if let Some(trace) = self.session_trace.get() {
            trace.summary.shutdown();
        }
        cleanup.unwrap_or(Ok(()))
    }
    fn trace_state(&self, state: ClinkState) {
        if let Some(trace) = self.session_trace.get() {
            trace.record(ClinkTraceEvent::Link {
                at_nanos: now_nanos(),
                session: 0,
                stage: "state".into(),
                fields: [("state".into(), format!("{state:?}"))].into(),
            });
        }
    }
    fn state_event(&self, state: ClinkState) {
        event(
            json!({"type":"state","state":format!("{state:?}"),"generation":self.generation.load(Ordering::Acquire)}),
        );
    }
    fn emit_ended(&self) {
        if self.has_output.load(Ordering::Acquire) && !self.ended.swap(true, Ordering::AcqRel) {
            self.trace_state(ClinkState::Ended);
            self.state_event(ClinkState::Ended);
        }
    }
}
struct RelayListener(Arc<RelayShutdown>);
impl ClinkListener for RelayListener {
    fn state(&self, state: ClinkState) {
        if state == ClinkState::Ended {
            self.0.emit_ended();
            if !self.0.stopped.load(Ordering::Acquire)
                && !self.0.keep_running.load(Ordering::Acquire)
            {
                self.0.session_failed.store(true, Ordering::Release);
            }
        } else {
            self.0.trace_state(state);
            self.0.state_event(state);
        }
    }
    fn error(&self, message: &str) {
        if let Some(trace) = self.0.session_trace.get() {
            // Listener failures arrive as text; keep a bounded stable description.
            trace.record(ClinkTraceEvent::Error {
                at_nanos: now_nanos(),
                session: 0,
                code: "listener_error".into(),
                exception_class: message.chars().take(120).collect(),
                location: "RelayListener".into(),
            });
        }
        Listener.error(message);
        if !self.0.keep_running.load(Ordering::Acquire) {
            self.0.session_failed.store(true, Ordering::Release);
        }
    }
    fn diagnostic(&self, message: &str) {
        if let Some(trace) = self.0.session_trace.get() {
            trace.record(ClinkTraceEvent::Link {
                at_nanos: now_nanos(),
                session: 0,
                stage: "diagnostic".into(),
                fields: [("detail".into(), message.into())].into(),
            });
        }
        Listener.diagnostic(message);
    }
    fn view_size(&self, width: u32, height: u32) {
        if let Some(trace) = self.0.session_trace.get() {
            trace.record(ClinkTraceEvent::Link {
                at_nanos: now_nanos(),
                session: 0,
                stage: "view".into(),
                fields: [("size".into(), format!("{width}x{height}"))].into(),
            });
        }
        Listener.view_size(width, height);
    }
    fn statistics(&self, stats: ClinkStatistics) {
        if let Some(trace) = self.0.session_trace.get() {
            trace.record(ClinkTraceEvent::Link {
                at_nanos: now_nanos(),
                session: 0,
                stage: "statistics".into(),
                fields: [
                    ("stream".into(), format!("{}x{}", stats.width, stats.height)),
                    (
                        "codec".into(),
                        stats
                            .codec
                            .map_or("unknown", |codec| match codec {
                                oxp_clink::model::VideoCodec::H264 => "H264",
                                oxp_clink::model::VideoCodec::H265 => "H265",
                            })
                            .into(),
                    ),
                    ("liveFrames".into(), stats.live_frames.to_string()),
                ]
                .into(),
            });
        }
        Listener.statistics(stats);
    }
    fn vehicle(&self, origin: oxp_clink::model::Origin, data: oxp_clink::model::VehicleData) {
        let mut value = vehicle_event(origin, data);
        value["generation"] = json!(self.0.generation.load(Ordering::Acquire));
        event(value);
    }
    fn vehicle_state(&self, active: bool, malformed: u64) {
        event(
            json!({"type":"clink-vehicle-state","generation":self.0.generation.load(Ordering::Acquire),"sensorActive":active,"malformedFrames":malformed}),
        );
    }
    fn profile_mismatch(&self, input: &str, decoders: &[String]) {
        Listener.profile_mismatch(input, decoders);
    }
}

fn vehicle_enabled(value: Option<&str>) -> Result<bool> {
    match value.unwrap_or("off") {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err("--clink-vehicle-data must be on or off".into()),
    }
}
struct CarlinkFactory {
    link: LinkFactory,
    trust: Arc<dyn oxp_clink::TrustStore>,
    digits: Option<String>,
    file: Option<String>,
    options: oxp_clink::session::SessionOptions,
    phone_mac: Option<String>,
    trace: Arc<SessionTrace>,
    transcode: bool,
    controlled: bool,
}
impl CarlinkFactory {
    fn from_args(args: &Args, link: LinkFactory, trace: Arc<SessionTrace>) -> Result<Self> {
        let identity = clink_identity::Identity::from_args(args)?;
        let trust = identity.trust(FileTrust::new(Path::new(args.need("--state-dir")?))?);
        Ok(Self {
            link,
            trust: Arc::new(trust),
            digits: args.get("--pin").map(str::to_owned),
            file: args.get("--pin-file").map(str::to_owned),
            options: oxp_clink::session::SessionOptions {
                device_id: identity.device_id,
                device_name: identity.device_name,
                ports: parse_ports(args.get("--port"))?,
                hevc: hevc_mode(args.get("--clink-hevc"))?,
                vehicle_data: vehicle_enabled(args.get("--clink-vehicle-data"))?,
                ..Default::default()
            },
            phone_mac: clink_phone_mac(args)?,
            trace,
            transcode: transcode_enabled(args.get("--clink-transcode"))?,
            controlled: args.get("--control-stdin").is_some(),
        })
    }
}
struct CarlinkGeneration {
    active: Arc<Mutex<bool>>,
    stopped: Arc<AtomicBool>,
    pins: Arc<PairingRequests>,
}
struct CarlinkCurrent {
    generation: Option<CarlinkGeneration>,
    // Retained choices also survive pairing cancellation or a failed replacement link.
    options: oxp_clink::session::SessionOptions,
    phone_mac: Option<String>,
}
struct CarlinkRuntime {
    factory: CarlinkFactory,
    shutdown: Arc<RelayShutdown>,
    hub: Arc<Hub>,
    // Serializes controls, retirement, and replacement. Media callbacks never take this lock.
    current: Mutex<CarlinkCurrent>,
}
impl CarlinkRuntime {
    fn new(factory: CarlinkFactory, shutdown: Arc<RelayShutdown>, hub: Arc<Hub>) -> Self {
        let current = Mutex::new(CarlinkCurrent {
            generation: None,
            options: factory.options.clone(),
            phone_mac: factory.phone_mac.clone(),
        });
        Self {
            factory,
            shutdown,
            hub,
            current,
        }
    }
    fn retire(&self, current: &mut Option<CarlinkGeneration>) -> Result<()> {
        if let Some(old) = current.as_ref() {
            *old.active.lock().unwrap() = false;
            old.stopped.store(true, Ordering::Release);
            old.pins.cancel();
        }
        self.hub.remove_output("clink");
        let output = self.shutdown.output.lock().unwrap().clone();
        if let Some(output) = output {
            output.close();
            // Cancel a platform join as well as the session's sockets.
            if let Some(link) = self.shutdown.link.lock().unwrap().as_ref() {
                link.disconnect()?;
            }
            if !output.await_closed(Duration::from_secs(2)) {
                return Err("carlink_retirement_timeout".into());
            }
        }
        self.shutdown.output.lock().unwrap().take();
        current.take();
        Ok(())
    }
    fn restart(&self, value: &serde_json::Value) -> Result<u64> {
        let mut current = self.current.lock().unwrap();
        let mut options = current.options.clone();
        if let Some(picture) = value.get("picture") {
            options.picture_size = if picture.is_null() {
                None
            } else {
                let width = picture["width"]
                    .as_u64()
                    .filter(|n| (320..=3840).contains(n) && n % 2 == 0)
                    .ok_or("invalid_picture")?;
                let height = picture["height"]
                    .as_u64()
                    .filter(|n| (240..=2160).contains(n) && n % 2 == 0)
                    .ok_or("invalid_picture")?;
                Some((width as u32, height as u32))
            };
        }
        if let Some(hevc) = value.get("hevc") {
            options.hevc = hevc_mode(Some(hevc.as_str().ok_or("invalid_hevc")?))?;
        }
        if let Some(vehicle) = value.get("vehicleData") {
            options.vehicle_data = vehicle.as_bool().ok_or("invalid_vehicle_data")?;
        }
        let phone_mac = match value.get("phoneMac") {
            None => current.phone_mac.clone(),
            Some(serde_json::Value::Null) => None,
            Some(mac) => Some(oxp_link_linux::normalized_bluetooth_address(
                mac.as_str().ok_or("invalid_phone_mac")?,
            )?),
        };
        if self.shutdown.stopped.load(Ordering::Acquire) {
            return Err("relay_stopped".into());
        }
        self.retire(&mut current.generation)?;
        current.options = options.clone();
        current.phone_mac = phone_mac.clone();
        let generation = self.shutdown.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let active = Arc::new(Mutex::new(true));
        let stopped = Arc::new(AtomicBool::new(false));
        let pins = Arc::new(PairingRequests::new(generation));
        let trace = self.factory.trace.clone();
        let link_active = active.clone();
        let link = (self.factory.link)(
            Arc::new(move |state| {
                deliver_link(
                    &link_active,
                    trace.as_ref(),
                    generation,
                    state,
                    private_event,
                );
            }),
            phone_mac,
        )?;
        *self.shutdown.link.lock().unwrap() = Some(link.clone());
        self.shutdown.ended.store(false, Ordering::Release);
        self.shutdown.session_failed.store(false, Ordering::Release);
        self.factory.trace.record(ClinkTraceEvent::Link {
            at_nanos: now_nanos(),
            session: 0,
            stage: "link_start".into(),
            fields: [("generation".into(), generation.to_string())].into(),
        });
        let mut output = ClinkOutput::new(
            link,
            Arc::new(Pin {
                digits: self.factory.digits.clone(),
                file: self.factory.file.clone(),
                stopped: stopped.clone(),
                auto_confirm: false,
                requests: self.factory.controlled.then_some(pins.clone()),
            }),
            self.factory.trust.clone(),
        )?
        .with_options(options.clone())?
        .with_listener(Arc::new(GenerationListener {
            active: active.clone(),
            listener: RelayListener(self.shutdown.clone()),
        }))
        .with_trace(Arc::new(GenerationTrace {
            active: active.clone(),
            trace: self.factory.trace.clone(),
        }));
        if let Some(recorder) = &self.factory.trace.recorder {
            output = output.with_media_tap(recorder.capture());
        }
        let output = Arc::new(output);
        self.shutdown.has_output.store(true, Ordering::Release);
        *self.shutdown.output.lock().unwrap() = Some(output.clone());
        let transcoder = oxp_transcode_linux::default_factory();
        event(transcode_capabilities(
            "clink",
            self.factory.transcode,
            transcoder.as_ref(),
        ));
        let projection: Arc<dyn ProjectionOutput> = if self.factory.transcode {
            diagnostic(&transcoder.describe());
            Arc::new(oxp_transcode_linux::TranscodingOutput::carlink(
                output,
                transcoder,
                Arc::new(diagnostic),
                options.picture_size,
            ))
        } else {
            output
        };
        current.generation = Some(CarlinkGeneration {
            active,
            stopped,
            pins,
        });
        self.hub.add_output(projection)?;
        Ok(generation)
    }
    fn control(&self, value: &serde_json::Value) -> serde_json::Value {
        let result = if value["type"] == "clink-restart" {
            self.restart(value)
                .map(|generation| json!({"generation":generation,"result":"started"}))
        } else {
            let mut current = self.current.lock().unwrap();
            let answer = current
                .generation
                .as_ref()
                .ok_or("stale_request")
                .and_then(|g| g.pins.respond(value));
            match answer {
                Ok(true) => self.retire(&mut current.generation).map(|()| {
                    self.shutdown.emit_ended();
                    json!({"result":"cancelled"})
                }),
                Ok(false) => Ok(json!({"result":"accepted"})),
                Err(error) => Err(error.into()),
            }
        };
        let mut response = result.unwrap_or_else(|e| json!({"error":e.to_string()}));
        response["type"] = value["type"].clone();
        response["request"] = value["request"].clone();
        response["generation"] = json!(self.shutdown.generation.load(Ordering::Acquire));
        response
    }
}
struct GenerationTrace {
    active: Arc<Mutex<bool>>,
    trace: Arc<SessionTrace>,
}
impl ClinkTrace for GenerationTrace {
    fn record(&self, event: ClinkTraceEvent) {
        let active = self.active.lock().unwrap();
        if *active {
            self.trace.record(event);
        }
    }
}
struct GenerationListener {
    active: Arc<Mutex<bool>>,
    listener: RelayListener,
}
impl GenerationListener {
    fn deliver(&self, callback: impl FnOnce(&RelayListener)) {
        let active = self.active.lock().unwrap();
        if *active {
            callback(&self.listener);
        }
    }
}
impl ClinkListener for GenerationListener {
    fn state(&self, state: ClinkState) {
        self.deliver(|l| l.state(state));
    }
    fn error(&self, error: &str) {
        self.deliver(|l| l.error(error));
    }
    fn diagnostic(&self, message: &str) {
        self.deliver(|l| l.diagnostic(message));
    }
    fn view_size(&self, width: u32, height: u32) {
        self.deliver(|l| l.view_size(width, height));
    }
    fn statistics(&self, stats: ClinkStatistics) {
        self.deliver(|l| l.statistics(stats));
    }
    fn profile_mismatch(&self, input: &str, decoders: &[String]) {
        self.deliver(|l| l.profile_mismatch(input, decoders));
    }
    fn vehicle(&self, origin: oxp_clink::model::Origin, data: oxp_clink::model::VehicleData) {
        self.deliver(|l| l.vehicle(origin, data));
    }
    fn vehicle_state(&self, active: bool, malformed: u64) {
        self.deliver(|l| l.vehicle_state(active, malformed));
    }
}

fn supervise(
    shutdown: Arc<RelayShutdown>,
    mut signal: impl FnMut() -> bool,
    work: impl FnOnce(Arc<RelayShutdown>) -> Result<()> + Send + 'static,
) -> Result<()> {
    let (tx, rx) = sync_channel(1);
    let state = shutdown.clone();
    std::thread::spawn(move || {
        let _ = tx.send(work(state));
    });
    let (result, interrupted) = loop {
        if signal() {
            break (Ok(()), true);
        }
        if shutdown.session_failed.load(Ordering::Acquire) {
            break (Err("Carlink session failed".into()), false);
        }
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(result) => break (result, signal()),
            Err(RecvTimeoutError::Timeout) => {}
            Err(_) => break (Err("Relay worker stopped unexpectedly".into()), false),
        }
    };
    shutdown.stopped.store(true, Ordering::Release);
    let (tx, rx) = sync_channel(1);
    let cleanup_state = shutdown.clone();
    std::thread::spawn(move || {
        let _ = tx.send(cleanup_state.finish());
    });
    // Includes output stop, link leave and wire close, with margin for the shell's 3 s grace.
    let cleanup = rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap_or_else(|_| Err("Relay cleanup timed out".into()));
    if let Some(close) = shutdown.wire.lock().unwrap().take() {
        close();
    }
    if interrupted {
        if cleanup.is_err() {
            event(json!({"type":"error","message":"Relay cleanup did not complete"}));
        }
        shutdown.emit_ended();
        Ok(())
    } else {
        result.and(cleanup)
    }
}

fn run() -> Result<()> {
    #[cfg(target_os = "linux")]
    let signals = oxp_link_linux::ShutdownSignals::install()?;
    supervise(
        Arc::new(RelayShutdown::default()),
        || {
            #[cfg(target_os = "linux")]
            {
                signals.requested()
            }
            #[cfg(not(target_os = "linux"))]
            {
                false
            }
        },
        run_relay,
    )
}

fn run_relay(shutdown: Arc<RelayShutdown>) -> Result<()> {
    let (args, sans) = arguments()?;
    if args.get("--help").is_some() {
        println!("{USAGE}");
        return Ok(());
    }
    if args.get("--unix").is_none() && args.get("--stdio").is_none() {
        return Err("Choose --unix PATH or --stdio".into());
    }
    let advertise = advertise_address(&args)?;
    let web = args.get("--web-port").is_some();
    event_stdout(args.get("--stdio").is_none());
    let trace = args
        .get("--clink-trace")
        .map(|directory| Arc::new(TraceRecorder::new(directory, Arc::new(event))));
    if let Some(trace) = &trace {
        let _ = shutdown.trace.set(trace.clone());
    }
    let session_trace = Arc::new(SessionTrace {
        summary: Arc::new(SummaryTrace::new(Arc::new(event))),
        recorder: trace.clone(),
    });
    let _ = shutdown.session_trace.set(session_trace.clone());
    let link = clink_link_factory(&args)?;
    shutdown.keep_running.store(
        web || args.get("--control-stdin").is_some(),
        Ordering::Release,
    );
    let clink = link.is_some();
    if !web && !clink {
        return Err("At least one output is required".into());
    }
    if !web
        && (!sans.is_empty()
            || [
                "--web-root",
                "--web-bind",
                "--advertise-address",
                "--token",
                "--pairing",
                "--source-url",
                "--tls-cert",
                "--tls-key",
                "--tls-name",
                "--tls-self-signed",
                "--no-tls",
            ]
            .iter()
            .any(|k| args.get(k).is_some()))
    {
        return Err("Web options require --web-port".into());
    }
    let mut outputs: Vec<Arc<dyn ProjectionOutput>> = Vec::new();
    let mut web_output = None;
    let mut pairing = None;
    if web {
        let tls = tls_from_args(&args, sans)?;
        let web_pairing = pairing_from_args(&args)?;
        // A relay serves head units on the LAN; plain HTTP is only for local testing.
        let default_bind = if matches!(tls, Tls::Disabled) {
            "127.0.0.1"
        } else {
            "0.0.0.0"
        };
        let bind = std::net::SocketAddr::new(
            args.get("--web-bind").unwrap_or(default_bind).parse()?,
            args.need("--web-port")?.parse()?,
        );
        pairing = Some((web_pairing.clone(), !matches!(tls, Tls::Disabled)));
        let mut config =
            WebOutputConfig::new(bind, args.need("--web-root")?.into(), web_pairing, tls);
        if let Some(root) = args.get("--plugins-root") {
            config.plugins = Some(oxp_web::PluginsConfig {
                root: root.into(),
                enabled: args
                    .get("--plugins-enabled")
                    .unwrap_or("")
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect(),
                state_dir: args.get("--plugins-state-dir").map(Into::into),
                topics: args
                    .get("--plugins-topics")
                    .map(|s| {
                        s.split(',')
                            .filter(|t| !t.is_empty())
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_else(|| oxp_web::PluginsConfig::default().topics),
            });
        } else if args.get("--plugins-enabled").is_some()
            || args.get("--plugins-state-dir").is_some()
            || args.get("--plugins-topics").is_some()
        {
            return Err("Plugin options require --plugins-root".into());
        }
        if let Some(url) = args.get("--source-url") {
            config.source_url = url.into();
        }
        config.host_ui = args.get("--host-ui").is_some();
        config.relay = args.get("--relay-name").map(|name| oxp_web::RelayInfo {
            name: name.into(),
            version: args.get("--relay-version").unwrap().into(),
        });
        config.diagnostics = Some(Arc::new(|id, value| {
            if value["type"] == "receiver-info" {
                let mut info = value.clone();
                info.as_object_mut().unwrap().remove("type");
                event(json!({"type":"receiver-info","client":id,"info":info}));
            } else {
                // Host-only receiver-disconnected records use the same client ID as receiver-info.
                let mut stats = value.clone();
                stats["client"] = json!(id);
                event(stats);
            }
        }));
        let enabled = web_transcode_enabled(args.get("--web-transcode"))?;
        let transcoder = oxp_transcode_linux::default_factory();
        config.common_video_decoders = enabled && transcoder.support().is_some();
        event(transcode_capabilities("web", enabled, transcoder.as_ref()));
        let output = Arc::new(WebOutput::new(config)?);
        web_output = Some(output.clone());
        let output: Arc<dyn ProjectionOutput> = if enabled {
            with_transcoding(output, transcoder, Arc::new(diagnostic))
        } else {
            output
        };
        outputs.push(output);
    }
    let carlink_factory = if let Some(link) = link {
        Some(CarlinkFactory::from_args(&args, link, session_trace)?)
    } else {
        None
    };
    let mut connection = connect(&args, true)?;
    *shutdown.wire.lock().unwrap() = Some(connection.shutdown.clone());
    if shutdown.stopped.load(Ordering::Acquire) {
        (connection.shutdown)();
        return Ok(());
    }
    handshake(&mut connection, "consumer", "relay")?;
    let (tx, rx) = sync_channel::<Message>(256);
    let mut writer = std::mem::replace(&mut connection.writer, Box::new(std::io::sink()));
    let stopped = shutdown.stopped.clone();
    let failed = Arc::new(AtomicBool::new(false));
    let worker_stopped = stopped.clone();
    let worker_failed = failed.clone();
    let wire_shutdown = connection.shutdown.clone();
    std::thread::spawn(move || loop {
        let m = match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(m) => m,
            Err(RecvTimeoutError::Timeout) if !worker_stopped.load(Ordering::Acquire) => {
                continue;
            }
            Err(_) => break,
        };
        if let Err(e) = oxp_wire::write(&mut writer, &m) {
            event(json!({"type":"error","message":e.to_string()}));
            worker_failed.store(true, Ordering::Release);
            wire_shutdown();
            break;
        }
    });
    let log = EventLog::global().clone();
    let host = Arc::new(Sink {
        tx: tx.clone(),
        state: Mutex::new(("default".into(), [0; 256])),
        clock: Instant::now(),
        shutdown: connection.shutdown.clone(),
        failed: failed.clone(),
        log: log.clone(),
    });
    let hub = Hub::new(host);
    *shutdown.hub.lock().unwrap() = Some(hub.clone());
    let carlink = carlink_factory
        .map(|factory| Arc::new(CarlinkRuntime::new(factory, shutdown.clone(), hub.clone())));
    let result = (|| -> Result<()> {
        for output in outputs {
            if shutdown.stopped.load(Ordering::Acquire) {
                return Ok(());
            }
            hub.add_output(output)?;
        }
        if let Some(carlink) = &carlink {
            carlink.restart(&json!({}))?;
        }
        if args.get("--control-stdin").is_some() {
            let carlink = carlink.clone();
            let trace = trace.clone();
            let web = web_output.clone();
            std::thread::Builder::new()
                .name("relay-control".into())
                .spawn(move || {
                    read_control_with(
                        std::io::stdin().lock(),
                        trace.as_deref(),
                        |line| relay_control(line, web.as_deref(), carlink.as_deref()),
                        event,
                    );
                })?;
        }
        if let Some(web) = web_output {
            let addr = web.await_listening(Duration::from_secs(5))?;
            if let Some((web_pairing, tls)) = pairing {
                let host = if advertise.is_some() || addr.ip().is_unspecified() {
                    match lan_address(advertise) {
                        Some(ip) => std::net::SocketAddr::new(ip.into(), addr.port()).to_string(),
                        None => format!("localhost:{}", addr.port()),
                    }
                } else {
                    addr.to_string()
                };
                event(json!({"type":"pairing-url","url":pairing_url(&host, tls, &web_pairing)}));
            }
        }
        let mut consumer = Consumer::default();
        while let Some(message) = oxp_wire::read(&mut connection.reader)? {
            if let Message::Json(v) = &message {
                if v["type"] == "session" {
                    log.try_log(v.clone());
                }
            }
            consumer.receive(message, hub.as_ref())?;
        }
        Ok(())
    })();
    if let Err(e) = &result {
        let _ = tx.try_send(json_message(
            json!({"type":"error","code":"WIRE","message":e.to_string()}),
        ));
    }
    if failed.load(Ordering::Acquire) {
        return Err("Wire writer failed or callback queue overflowed".into());
    }
    result
}
fn main() {
    let result = run();
    // Events queued on the wire thread precede the final error, as when they were written directly.
    EventLog::flush_global(Duration::from_millis(500));
    if let Err(e) = result {
        event(json!({"type":"error","message":e.to_string()}));
        std::process::exit(1);
    }
}

/// The address a head unit most likely shares with the relay: prefer the default-route
/// interface, then Wi-Fi/Ethernet over USB gadgets and virtual interfaces, then private ranges
/// 192.168/16, 10/8 and 172.16/12. Loopback and non-private addresses are never auto-selected.
fn lan_address(advertise: Option<Ipv4Addr>) -> Option<Ipv4Addr> {
    let routes = std::fs::read_to_string("/proc/net/route").unwrap_or_default();
    lan_choice(&local_interfaces(), &default_routes(&routes), advertise)
}

fn local_interfaces() -> Vec<(String, Ipv4Addr)> {
    let commands: [(&str, &[&str]); 2] = [("ip", &["-o", "-4", "addr", "show"]), ("ifconfig", &[])];
    for (program, args) in commands {
        if let Ok(out) = std::process::Command::new(program).args(args).output() {
            if out.status.success() {
                let interfaces = parse_interfaces(&String::from_utf8_lossy(&out.stdout));
                if !interfaces.is_empty() {
                    return interfaces;
                }
            }
        }
    }
    Vec::new()
}

// Accept both ip -o output and Linux/BSD ifconfig output, including BusyBox's inet addr: form.
fn parse_interfaces(text: &str) -> Vec<(String, Ipv4Addr)> {
    let mut interfaces = Vec::new();
    let mut name = "";
    for line in text.lines() {
        let words: Vec<_> = line.split_whitespace().collect();
        if words.is_empty() {
            continue;
        }
        if words[0].trim_end_matches(':').parse::<u32>().is_ok() {
            name = words.get(1).copied().unwrap_or("");
        } else if !line.starts_with(char::is_whitespace) {
            name = words[0];
        }
        let name = name.trim_end_matches(':').split('@').next().unwrap_or("");
        for pair in words.windows(2) {
            if pair[0] == "inet" {
                if let Ok(ip) = pair[1]
                    .trim_start_matches("addr:")
                    .split('/')
                    .next()
                    .unwrap_or("")
                    .parse()
                {
                    interfaces.push((name.into(), ip));
                }
            }
        }
    }
    interfaces
}

fn default_routes(text: &str) -> Vec<(String, u32)> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.get(1)? != &"00000000" || fields.get(7)? != &"00000000" {
                return None;
            }
            let flags = u32::from_str_radix(fields.get(3)?, 16).ok()?;
            if flags & 1 == 0 || flags & 0x200 != 0 {
                return None;
            }
            Some((fields[0].into(), fields.get(6)?.parse().ok()?))
        })
        .collect()
}

fn lan_choice(
    interfaces: &[(String, Ipv4Addr)],
    routes: &[(String, u32)],
    advertise: Option<Ipv4Addr>,
) -> Option<Ipv4Addr> {
    if advertise.is_some() {
        return advertise;
    }
    let default = routes.iter().min_by_key(|(name, metric)| (*metric, name));
    interfaces
        .iter()
        .filter(|(name, ip)| name != "lo" && name != "lo0" && !ip.is_loopback())
        .filter_map(|(name, ip)| {
            let range = match ip.octets() {
                [192, 168, ..] => 0,
                [10, ..] => 1,
                [172, b, ..] if (16..32).contains(&b) => 2,
                _ => return None,
            };
            let virtual_interface = [
                "usb", "rndis", "docker", "veth", "br-", "virbr", "tun", "tap", "wg",
            ]
            .iter()
            .any(|prefix| name.starts_with(prefix));
            let default_interface = default.is_some_and(|(interface, _)| interface == name);
            Some((!default_interface, virtual_interface, range, *ip))
        })
        .min()
        .map(|(_, _, _, ip)| ip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_transcode_flag_is_independent_validated_and_documented() {
        for mode in ["auto", "off"] {
            assert!(parse_arguments(
                ["--web-port", "8443", "--web-transcode", mode].map(str::to_owned)
            )
            .is_ok());
        }
        assert!(parse_arguments(["--web-transcode", "off"].map(str::to_owned)).is_err());
        assert!(parse_arguments(
            ["--web-port", "8443", "--web-transcode", "on"].map(str::to_owned)
        )
        .is_err());
        assert!(USAGE.contains("--web-transcode auto|off"));
        assert!(USAGE.contains("transcode-capabilities"));
    }
    #[test]
    fn merged_help_describes_the_shared_channel_and_current_carlink_policy() {
        for text in [
            "8 s window",
            "--clink-identity ble|legacy",
            "web-tls",
            "clink-restart",
            "clink-pairing",
            "explicit approval",
            "--clink-vehicle-data on|off",
            "--clink-hevc auto|off|signal",
            "--web-transcode auto|off",
            "type=transcode-capabilities",
            "receiver-disconnected",
            "receiver-info client ID",
            "phoneMac",
            "null clears",
            "absence retains",
        ] {
            assert!(USAGE.contains(text), "{text}");
        }
        assert!(!USAGE.contains("confirms Carlink pairing automatically"));
        assert!(!USAGE.contains("5 s window"));
    }

    #[test]
    fn carlink_factory_preserves_ble_identity_scoped_trust_and_session_options() {
        let root = std::env::temp_dir().join(format!("oxp-merged-factory-{}", std::process::id()));
        let trace = Arc::new(SessionTrace {
            summary: Arc::new(SummaryTrace::new(Arc::new(|_| {}))),
            recorder: None,
        });
        let mut factories = Vec::new();
        for mode in ["ble", "legacy"] {
            let args = args(&[
                "--unix",
                "wire",
                "--control-stdin",
                "--clink-link",
                "ble",
                "--state-dir",
                root.to_str().unwrap(),
                "--clink-identity",
                mode,
                "--clink-id",
                "relay-42",
                "--clink-name",
                "Configured relay",
                "--clink-hevc",
                "signal",
                "--clink-vehicle-data",
                "on",
                "--clink-phone-mac",
                "AA:BB:CC:DD:EE:FF",
            ]);
            let factory = CarlinkFactory::from_args(
                &args,
                clink_link_factory(&args).unwrap().unwrap(),
                trace.clone(),
            )
            .unwrap();
            let defaults = oxp_clink::session::SessionOptions::default();
            assert_eq!(
                factory.options.device_id,
                if mode == "ble" {
                    "relay-42"
                } else {
                    &defaults.device_id
                }
            );
            assert_eq!(
                factory.options.device_name,
                if mode == "ble" {
                    "Configured relay"
                } else {
                    &defaults.device_name
                }
            );
            assert_eq!(factory.options.hevc, oxp_clink::ClinkHevcMode::Signal);
            assert_eq!(factory.phone_mac.as_deref(), Some("aa:bb:cc:dd:ee:ff"));
            assert!(factory.options.vehicle_data && factory.controlled);
            factory.trust.save_peer_key("car", mode.as_bytes()).unwrap();
            factories.push(factory);
        }
        assert_eq!(
            factories[0].trust.load_peer_key("car").unwrap().unwrap(),
            b"ble"
        );
        assert_eq!(
            factories[1].trust.load_peer_key("car").unwrap().unwrap(),
            b"legacy"
        );
        factories[0].trust.forget_peer("car").unwrap();
        assert_eq!(
            factories[1].trust.load_peer_key("car").unwrap().unwrap(),
            b"legacy"
        );
        trace.summary.shutdown();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn raw_capture_is_opt_in_even_with_an_always_on_summary() {
        let summary = SummaryTrace::new(Arc::new(|_| {}));
        for extra in [vec![], vec!["--clink-trace", "/tmp/trace"]] {
            let enabled = !extra.is_empty();
            let mut values = vec!["--clink-link", "ble", "--state-dir", "/tmp/state"];
            values.extend(extra);
            let config = ble_config(&args(&values)).unwrap();
            assert_eq!(config.trace_advertisements, enabled);
            assert_eq!(config.scan_timeout, Duration::from_secs(8));
        }
        summary.shutdown();
    }

    #[test]
    fn shared_stdin_dispatches_tls_session_and_trace_commands_with_one_bound() {
        use std::io::{BufReader, Cursor};
        let root = std::env::temp_dir().join(format!("oxp-merged-control-{}", std::process::id()));
        let web = WebOutput::new(WebOutputConfig::new(
            "127.0.0.1:0".parse().unwrap(),
            root.clone(),
            Pairing::Disabled,
            Tls::SelfSigned {
                state_dir: root.clone(),
                sans: Vec::new(),
            },
        ))
        .unwrap();
        let mut input = vec![b'x'; 4097];
        input.extend_from_slice(
            concat!(
            "\n{\"type\":\"web-tls\",\"id\":\"r1\",\"trusted\":null,\"sans\":[\"192.0.2.31\"]}\n",
            "{\"type\":\"clink-restart\",\"request\":\"resize\"}\n",
            "{\"type\":\"clink-trace\",\"action\":\"clear\"}\n",
            "{\"type\":\"clink-pairing\",\"request\":17,\"action\":\"cancel\"}\n",
            "{\"type\":\"clink-capture\",\"action\":\"start\",\"seconds\":1}\n",
            "{\"type\":\"clink-capture\",\"action\":\"stop\"}"
        )
            .as_bytes(),
        );
        let mut replies = Vec::new();
        read_control_with(
            BufReader::with_capacity(7, Cursor::new(input)),
            None,
            |line| relay_control(line, Some(&web), None),
            |v| replies.push(v),
        );
        assert_eq!(replies.len(), 7);
        assert_eq!(replies[0]["message"], "clink control: line_too_long");
        assert_eq!(replies[1]["applied"], true);
        assert_eq!(replies[1]["id"], "r1");
        for (index, kind, request) in [
            (2, "clink-restart", json!("resize")),
            (4, "clink-pairing", json!(17)),
        ] {
            assert_eq!(
                replies[index],
                json!({"type":kind,"request":request,"error":"carlink_off"})
            );
        }
        for index in [3, 5, 6] {
            assert_eq!(
                replies[index],
                json!({"type":"clink-trace","error":"trace_off"})
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn linux_records_feed_summary_and_late_generations_reach_neither_port() {
        use oxp_link_linux::LinkEvent;
        let summaries = Arc::new(Mutex::new(Vec::new()));
        let values = summaries.clone();
        let trace = Arc::new(SessionTrace {
            summary: Arc::new(SummaryTrace::new(Arc::new(move |v| {
                values.lock().unwrap().push(v)
            }))),
            recorder: None,
        });
        trace.record(ClinkTraceEvent::Link {
            at_nanos: 0,
            session: 0,
            stage: "link_start".into(),
            fields: [("generation".into(), "2".into())].into(),
        });
        let retired = Arc::new(Mutex::new(false));
        let active = Mutex::new(true);
        let ports = Mutex::new(Vec::new());
        let advertisement = |name: &str| LinkEvent::Advertisement {
            advertisement: oxp_clink::ble::Advertisement {
                major: 1,
                minor: 2,
                serial: 3,
                car_id: "car".into(),
                model_id: "model".into(),
                vendor_data: "vendor-data".into(),
                name: name.into(),
            },
            serial_same: true,
        };
        for event in [
            advertisement("selected unit"),
            LinkEvent::Trace {
                stage: "ble_exchange_done".into(),
                fields: [
                    ("frequency".into(), "5180".into()),
                    ("replyType".into(), "1002".into()),
                ]
                .into(),
            },
            LinkEvent::Joined {
                address: "address".into(),
                name: "selected unit".into(),
                ssid: "network".into(),
                interface: "wlan0".into(),
                addresses: ClinkAddresses::new([192, 168, 49, 1].into(), [192, 168, 49, 2].into()),
            },
        ] {
            deliver_link(&active, trace.as_ref(), 2, event, |v| {
                ports.lock().unwrap().push(v)
            });
        }
        deliver_link(
            &retired,
            trace.as_ref(),
            1,
            advertisement("stale unit"),
            |_| panic!("retired link event"),
        );
        let stale_trace = GenerationTrace {
            active: retired.clone(),
            trace: trace.clone(),
        };
        stale_trace.record(ClinkTraceEvent::Link {
            at_nanos: 0,
            session: 0,
            stage: "link_start".into(),
            fields: Default::default(),
        });
        let shutdown = Arc::new(RelayShutdown::default());
        shutdown.generation.store(2, Ordering::Release);
        let stale_listener = GenerationListener {
            active: retired,
            listener: RelayListener(shutdown.clone()),
        };
        stale_listener.state(ClinkState::Ended);
        stale_listener.error("retired failure");
        assert!(!shutdown.session_failed.load(Ordering::Acquire));
        trace.record(ClinkTraceEvent::Link {
            at_nanos: 0,
            session: 0,
            stage: "state".into(),
            fields: [("state".into(), "Ended".into())].into(),
        });
        trace.summary.shutdown();
        let ports = ports.lock().unwrap();
        assert_eq!(ports.len(), 3);
        assert_eq!(ports[0]["type"], "ble_advertisement");
        assert_eq!(ports[0]["vendorData"], "vendor-data");
        assert_eq!(ports[0]["name"], "selected unit");
        assert_eq!(ports[1]["type"], "link-trace");
        assert_eq!(
            ports[1]["fields"],
            json!({"frequency":"5180","replyType":"1002"})
        );
        assert!(ports.iter().all(|v| v["generation"] == 2));
        let summaries = summaries.lock().unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0]["generation"], 2);
        assert_eq!(summaries[0]["fields"]["frequency"], "5180");
        assert_eq!(summaries[0]["fields"]["name"], "selected unit");
        assert_eq!(summaries[0]["fields"]["ssid"], "network");
    }

    fn tls(values: &[&str]) -> Result<Tls> {
        let (args, sans) = parse_arguments(values.iter().map(|v| (*v).to_owned()))?;
        tls_from_args(&args, sans)
    }

    #[test]
    fn tls_argument_validation_matrix() {
        assert!(matches!(
            tls(&["--tls-cert", "cert", "--tls-key", "key"]).unwrap(),
            Tls::Pem { .. }
        ));
        let self_signed = tls(&[
            "--tls-self-signed",
            "state",
            "--tls-san",
            "relay.local",
            "--tls-san",
            "192.168.55.1",
        ])
        .unwrap();
        assert!(
            matches!(self_signed, Tls::SelfSigned { sans, .. } if sans == ["relay.local", "192.168.55.1"])
        );
        let sni = tls(&[
            "--tls-cert",
            "cert",
            "--tls-key",
            "key",
            "--tls-self-signed",
            "state",
            "--tls-name",
            "RELAY.test.",
            "--tls-san",
            "relay.local",
        ])
        .unwrap();
        assert!(
            matches!(sni, Tls::Sni { name, sans, .. } if name == "RELAY.test." && sans == ["relay.local"])
        );
        assert!(matches!(tls(&["--no-tls"]).unwrap(), Tls::Disabled));
        let missing_name = [
            "--tls-cert",
            "cert",
            "--tls-key",
            "key",
            "--tls-self-signed",
            "state",
        ];
        assert!(tls(&missing_name)
            .unwrap_err()
            .to_string()
            .contains("requires --tls-name"));
        for values in [
            vec![],
            vec!["--tls-cert", "cert"],
            vec!["--tls-key", "key"],
            vec!["--tls-name", "relay.test"],
            vec!["--tls-self-signed", "state", "--tls-name", "relay.test"],
            vec![
                "--tls-cert",
                "cert",
                "--tls-key",
                "key",
                "--tls-name",
                "relay.test",
            ],
            vec![
                "--tls-cert",
                "cert",
                "--tls-self-signed",
                "state",
                "--tls-name",
                "relay.test",
            ],
            vec!["--tls-cert", "cert", "--tls-key", "key", "--no-tls"],
            vec!["--tls-self-signed", "state", "--no-tls"],
            vec![
                "--tls-cert",
                "cert",
                "--tls-key",
                "key",
                "--tls-self-signed",
                "state",
                "--tls-name",
                "relay.test",
                "--no-tls",
            ],
            vec![
                "--tls-cert",
                "cert",
                "--tls-key",
                "key",
                "--tls-san",
                "relay.local",
            ],
            vec!["--no-tls", "--tls-san", "relay.local"],
            vec!["--tls-self-signed", "state", "--tls-san"],
            vec!["--tls-name"],
            vec!["--tls-name", "relay.test", "--tls-name", "other.test"],
        ] {
            assert!(tls(&values).is_err(), "{values:?}");
        }
    }

    #[test]
    fn tls_name_accepts_only_bounded_dns_ldh_labels() {
        let longest = format!(
            "{}.{}.{}.{}",
            "a".repeat(63),
            "b".repeat(63),
            "c".repeat(63),
            "d".repeat(61)
        );
        for name in [
            "relay.duckdns.org",
            "RELAY.test.",
            "a-b.test",
            "localhost",
            "1relay.test",
            "xn--relay.test",
            &longest,
        ] {
            assert!(
                tls(&[
                    "--tls-cert",
                    "cert",
                    "--tls-key",
                    "key",
                    "--tls-self-signed",
                    "state",
                    "--tls-name",
                    name
                ])
                .is_ok(),
                "{name}"
            );
        }
        let long_label = format!("{}.test", "a".repeat(64));
        let too_long = format!("{longest}d");
        for name in [
            "",
            ".",
            "-relay.test",
            "relay-.test",
            "relay..test",
            "relay.test..",
            "relay_.test",
            "*.test",
            "relay test",
            "https://relay.test",
            "relay.test:443",
            "127.0.0.1",
            "::1",
            "rélais.test",
            &long_label,
            &too_long,
        ] {
            assert!(
                tls(&[
                    "--tls-cert",
                    "cert",
                    "--tls-key",
                    "key",
                    "--tls-self-signed",
                    "state",
                    "--tls-name",
                    name
                ])
                .unwrap_err()
                .to_string()
                .contains("--tls-name"),
                "{name}"
            );
        }
    }

    struct FakeLink(Arc<std::sync::atomic::AtomicUsize>);
    impl ClinkLink for FakeLink {
        fn connect(
            &self,
            _: Arc<dyn Fn(Option<String>) + Send + Sync>,
        ) -> oxp_clink::Result<oxp_clink::ClinkConnection> {
            Err("fake auth failure".into())
        }
        fn disconnect(&self) -> oxp_clink::Result<()> {
            self.0.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }
    }

    #[test]
    fn relay_identity_arguments_are_paired_and_bounded() {
        assert!(parse_arguments(Vec::<String>::new()).is_ok());
        assert!(parse_arguments(
            [
                "--relay-name",
                "OpenXPlay Linux",
                "--relay-version",
                "0.1.3"
            ]
            .map(String::from)
        )
        .is_ok());
        for values in [
            vec!["--relay-name", "Linux"],
            vec!["--relay-version", "0.1.3"],
            vec!["--relay-name", "", "--relay-version", "0.1.3"],
            vec!["--relay-name", "Linux", "--relay-version", "bad\nversion"],
        ] {
            assert!(parse_arguments(values.into_iter().map(String::from)).is_err());
        }
    }

    #[test]
    fn errors_disconnect_before_returning_to_main() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = calls.clone();
        let error = supervise(
            Arc::new(RelayShutdown::default()),
            || false,
            move |state| {
                *state.link.lock().unwrap() = Some(Arc::new(FakeLink(count)));
                Err("fake handshake failure".into())
            },
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "fake handshake failure");
        assert_eq!(calls.load(Ordering::Acquire), 1);
    }

    #[test]
    fn signal_cleanup_is_bounded_even_when_disconnect_stalls() {
        struct SlowLink(Mutex<std::sync::mpsc::Receiver<()>>);
        impl ClinkLink for SlowLink {
            fn connect(
                &self,
                _: Arc<dyn Fn(Option<String>) + Send + Sync>,
            ) -> oxp_clink::Result<oxp_clink::ClinkConnection> {
                unreachable!()
            }
            fn disconnect(&self) -> oxp_clink::Result<()> {
                let _ = self.0.lock().unwrap().recv();
                Ok(())
            }
        }
        let state = Arc::new(RelayShutdown::default());
        let (release, wait) = sync_channel(1);
        *state.link.lock().unwrap() = Some(Arc::new(SlowLink(Mutex::new(wait))));
        let start = Instant::now();
        supervise(state, || true, |_| Ok(())).unwrap();
        assert!(start.elapsed() < Duration::from_millis(2500));
        release.send(()).unwrap();
    }

    #[test]
    fn signal_and_session_failure_interrupt_blocked_wire_and_disconnect() {
        for signal in [true, false] {
            let state = Arc::new(RelayShutdown::default());
            let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            *state.link.lock().unwrap() = Some(Arc::new(FakeLink(calls.clone())));
            let closed = Arc::new(AtomicBool::new(false));
            let wire_closed = closed.clone();
            *state.wire.lock().unwrap() = Some(Arc::new(move || {
                wire_closed.store(true, Ordering::Release);
            }));
            let (ready_tx, ready_rx) = sync_channel(1);
            let (release_tx, release_rx) = sync_channel(1);
            let work = move |state: Arc<RelayShutdown>| {
                if !signal {
                    RelayListener(state).error("fake auth failure");
                }
                ready_tx.send(()).unwrap();
                let _ = release_rx.recv();
                Ok(())
            };
            let mut ready = false;
            let start = Instant::now();
            let result = supervise(
                state.clone(),
                || {
                    if !ready {
                        ready = ready_rx.try_recv().is_ok();
                    }
                    ready && signal
                },
                work,
            );
            release_tx.send(()).unwrap();
            assert_eq!(result.is_ok(), signal);
            assert!(state.stopped.load(Ordering::Acquire));
            assert!(closed.load(Ordering::Acquire));
            assert_eq!(calls.load(Ordering::Acquire), 1);
            assert!(start.elapsed() < Duration::from_secs(1));
        }
    }

    #[test]
    fn plugins_topics_option_is_optional_and_preserves_csv() {
        assert!(args(&[]).get("--plugins-topics").is_none());
        assert_eq!(
            args(&["--plugins-topics", "session,activity,route,audioSource"])
                .get("--plugins-topics"),
            Some("session,activity,route,audioSource")
        );
    }

    fn args(values: &[&str]) -> Args {
        parse_arguments(values.iter().map(|v| (*v).to_owned()))
            .unwrap()
            .0
    }

    #[test]
    fn p2p_reservation_flag_is_strict_ble_only_and_reaches_the_link_config() {
        for (value, expected) in [("on", true), ("off", false)] {
            let values = args(&[
                "--clink-link",
                "ble",
                "--state-dir",
                "/tmp/state",
                "--clink-p2p-reserved",
                value,
            ]);
            assert_eq!(
                ble_config(&values).unwrap().p2p_reserved_by_carplay,
                expected
            );
            assert!(clink_link(&values).unwrap().is_some());
        }
        assert!(!ble_config(&args(&[])).unwrap().p2p_reserved_by_carplay);
        assert!(clink_link(&args(&["--clink-p2p-reserved", "on"])).is_err());
        assert!(clink_link(&args(&[
            "--clink-link",
            "ble",
            "--state-dir",
            "/tmp/state",
            "--clink-p2p-reserved",
            "true"
        ]))
        .is_err());
        assert!(parse_arguments(["--clink-p2p-reserved"].map(str::to_owned)).is_err());
    }

    #[test]
    fn trace_flags_require_carlink_and_control_requires_unix() {
        for extra in [
            vec!["--clink-trace", "/tmp/trace"],
            vec!["--clink-trace", "/tmp/trace", "--control-stdin"],
        ] {
            let mut values = vec!["--unix", "/tmp/wire"];
            values.extend(extra.clone());
            assert!(clink_link(&args(&values)).is_err());
            let mut values = vec![
                "--unix",
                "/tmp/wire",
                "--clink-link",
                "ble",
                "--state-dir",
                "/tmp/state",
            ];
            values.extend(extra);
            assert!(clink_link(&args(&values)).unwrap().is_some());
        }
        for values in [
            vec!["--control-stdin"],
            vec!["--stdio", "--control-stdin"],
            vec!["--unix", "wire", "--control-stdin", "--control-stdin"],
            vec!["--clink-trace"],
            vec!["--clink-trace", ""],
            vec!["--clink-trace", "dir", "--clink-trace", "other"],
        ] {
            assert!(parse_arguments(values.into_iter().map(str::to_owned)).is_err());
        }
        assert!(clink_link(&args(&["--unix", "wire", "--control-stdin"]))
            .unwrap()
            .is_none());
        assert!(USAGE.contains("web-tls"));
        assert!(USAGE.contains("--clink-trace DIR"));
        assert!(USAGE.contains("--control-stdin"));
    }

    #[test]
    fn linux_link_trace_preserves_all_contract_fields() {
        use oxp_link_linux::LinkEvent;
        #[derive(Default)]
        struct Collect(Mutex<Vec<serde_json::Value>>);
        impl ClinkTrace for Collect {
            fn record(&self, event: ClinkTraceEvent) {
                self.0.lock().unwrap().push(event.to_json());
            }
        }
        let trace = Collect::default();
        for event in [
            LinkEvent::Scanning,
            LinkEvent::Connecting {
                address: "address".into(),
                name: "name".into(),
            },
            LinkEvent::Joined {
                address: "address".into(),
                name: "name".into(),
                ssid: "ssid".into(),
                interface: "wlan0".into(),
                addresses: ClinkAddresses::new([192, 168, 49, 1].into(), [192, 168, 49, 2].into()),
            },
            LinkEvent::Lost {
                reason: "lost".into(),
            },
            LinkEvent::Diagnostic {
                reason: "detail".into(),
            },
        ] {
            trace_link(&trace, &event);
        }
        let values = trace.0.lock().unwrap();
        assert_eq!(
            values
                .iter()
                .map(|v| v["stage"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "link_scanning",
                "link_connecting",
                "link_joined",
                "link_lost",
                "link_diagnostic"
            ]
        );
        assert_eq!(values[0]["fields"], json!({}));
        assert_eq!(
            values[1]["fields"],
            json!({"address":"address", "name":"name"})
        );
        assert_eq!(
            values[2]["fields"],
            json!({"address":"address", "name":"name", "ssid":"ssid",
            "interface":"wlan0", "headUnitIp":"192.168.49.1", "localIp":"192.168.49.2"})
        );
        assert_eq!(values[3]["fields"], json!({"reason":"lost"}));
        assert_eq!(values[4]["fields"], json!({"reason":"detail"}));
    }

    #[test]
    fn clink_hevc_argument_accepts_auto_or_off_and_requires_a_clink_link() {
        for value in ["auto", "off", "signal"] {
            let args = args(&[
                "--clink-link",
                "ble",
                "--state-dir",
                "/tmp/state",
                "--clink-hevc",
                value,
            ]);
            assert!(clink_link(&args).unwrap().is_some());
            assert_eq!(args.get("--clink-hevc"), Some(value));
        }
        for value in ["", "on", "AUTO"] {
            let args = args(&[
                "--clink-link",
                "ble",
                "--state-dir",
                "/tmp/state",
                "--clink-hevc",
                value,
            ]);
            assert_eq!(
                clink_link(&args).err().unwrap().to_string(),
                "--clink-hevc must be auto, off or signal"
            );
        }
        assert!(clink_link(&args(&["--clink-hevc", "off"])).is_err());
        for values in [
            vec!["--clink-hevc"],
            vec!["--clink-hevc", "auto", "--clink-hevc", "off"],
        ] {
            assert!(parse_arguments(values.into_iter().map(str::to_owned)).is_err());
        }
        assert!(USAGE.contains("--clink-hevc auto|off|signal"));
    }
    #[test]
    fn clink_modes_and_option_combinations() {
        assert!(clink_link(&args(&[])).unwrap().is_none());
        for values in [
            vec![
                "--clink-head-unit",
                "192.168.43.1",
                "--clink-local",
                "192.168.43.2",
                "--state-dir",
                "/tmp/state",
            ],
            vec![
                "--clink-link",
                "static",
                "--clink-head-unit",
                "192.168.43.1",
                "--clink-local",
                "192.168.43.2",
                "--state-dir",
                "/tmp/state",
                "--pin",
                "001234",
            ],
            vec![
                "--clink-link",
                "ble",
                "--state-dir",
                "/tmp/state",
                "--pin-file",
                "/tmp/pin",
            ],
            vec![
                "--clink-link",
                "ble",
                "--state-dir",
                "/tmp/state",
                "--clink-adapter",
                "hci1",
                "--clink-interface",
                "wlan1",
                "--clink-address",
                "AA:bb:CC:dd:EE:ff",
                "--clink-name",
                "OpenXPlay Linux",
                "--clink-model",
                "OpenXPlay Relay Linux",
                "--clink-id",
                "stable-id",
                "--clink-band",
                "0",
            ],
        ] {
            assert!(clink_link(&args(&values)).unwrap().is_some(), "{values:?}");
        }
        for values in [
            vec!["--clink-link", "unknown"],
            vec!["--clink-link", "static"],
            vec!["--clink-head-unit", "192.168.43.1"],
            vec!["--clink-local", "192.168.43.2"],
            vec!["--pin-file", "/tmp/pin"],
            vec!["--clink-adapter", "hci0"],
            vec!["--clink-band", "0"],
            vec!["--clink-transcode", "off"],
            vec![
                "--clink-head-unit",
                "192.168.43.1",
                "--clink-local",
                "192.168.43.2",
                "--state-dir",
                "/tmp/state",
                "--clink-transcode",
                "always",
            ],
        ] {
            assert!(clink_link(&args(&values)).is_err(), "{values:?}");
        }
        assert!(clink_link(&args(&[
            "--clink-link",
            "ble",
            "--state-dir",
            "/tmp/state",
            "--clink-transcode",
            "off",
        ]))
        .unwrap()
        .is_some());
        for extra in [
            vec!["--clink-head-unit", "192.168.43.1"],
            vec!["--clink-local", "192.168.43.2"],
            vec!["--pin", "12x"],
            vec!["--pin", "1234", "--pin-file", "/tmp/pin"],
            vec!["--pin-file", ""],
            vec!["--clink-address", "AA:BB:CC:DD:EE"],
            vec!["--clink-address", "AA:BB:CC:DD:EE:GG"],
            vec!["--clink-adapter", "bad"],
            vec!["--clink-interface", "bad/interface"],
            vec!["--clink-name", ""],
            vec!["--clink-model", ""],
            vec!["--clink-id", ""],
            vec!["--clink-band", "2"],
        ] {
            let mut values = vec!["--clink-link", "ble", "--state-dir", "/tmp/state"];
            values.extend(extra);
            assert!(clink_link(&args(&values)).is_err(), "{values:?}");
        }
        for values in [
            vec!["--clink-link"],
            vec!["--clink-link", "ble", "--clink-link", "ble"],
            vec!["--pin-file"],
        ] {
            assert!(parse_arguments(values.into_iter().map(str::to_owned)).is_err());
        }
    }

    #[test]
    fn pairing_arguments_default_random_explicit_and_precedence() {
        assert!(matches!(
            pairing_from_args(&args(&[])).unwrap(),
            Pairing::Disabled
        ));
        let Pairing::Required(code) = pairing_from_args(&args(&["--pairing", "--stdio"])).unwrap()
        else {
            panic!("Expected generated code")
        };
        assert_eq!(code.len(), 8);
        assert!(code.bytes().all(|b| b.is_ascii_digit()));
        for values in [
            vec!["--token", "custom"],
            vec!["--pairing", "--token", "custom"],
            vec!["--token", "custom", "--pairing"],
        ] {
            assert!(
                matches!(pairing_from_args(&args(&values)).unwrap(), Pairing::Required(code) if code == "custom")
            );
        }
        for values in [
            vec!["--pairing", "--pairing"],
            vec!["--token"],
            vec!["--pairing", "unexpected"],
        ] {
            assert!(parse_arguments(values.into_iter().map(str::to_owned)).is_err());
        }
    }

    #[test]
    fn pairing_urls_include_a_fragment_only_when_required() {
        assert_eq!(
            pairing_url("relay:8443", true, &Pairing::Disabled),
            "https://relay:8443/"
        );
        assert_eq!(
            pairing_url("127.0.0.1:8080", false, &Pairing::Disabled),
            "http://127.0.0.1:8080/"
        );
        assert_eq!(
            pairing_url("relay", true, &Pairing::Required("00123456".into())),
            "https://relay/#k=00123456"
        );
        assert_eq!(
            pairing_url("relay", false, &Pairing::Required("a /#é-._~".into())),
            "http://relay/#k=a%20%2F%23%C3%A9-._~"
        );
    }

    fn interfaces(list: &[(&str, &str)]) -> Vec<(String, Ipv4Addr)> {
        list.iter()
            .map(|(name, ip)| ((*name).into(), ip.parse().unwrap()))
            .collect()
    }

    #[test]
    fn pairing_host_prefers_the_lowest_metric_default_route() {
        let dongle = interfaces(&[("usb0", "192.168.68.1"), ("wlan0", "192.168.5.173")]);
        let routes = default_routes(
            "Iface Destination Gateway Flags RefCnt Use Metric Mask MTU Window IRTT
usb0 00000000 00000000 0001 0 0 600 00000000 0 0 0
wlan0 00000000 0105A8C0 0003 0 0 100 00000000 0 0 0
usb0 0044A8C0 00000000 0001 0 0 0 00FFFFFF 0 0 0
usb0 00000000 00000000 0000 0 0 0 00000000 0 0 0
malformed",
        );
        assert_eq!(routes.len(), 2);
        assert_eq!(
            lan_choice(&dongle, &routes, None),
            Some([192, 168, 5, 173].into())
        );
        assert_eq!(
            lan_choice(&dongle, &[("usb0".into(), 1), ("wlan0".into(), 2)], None),
            Some([192, 168, 68, 1].into())
        );
    }

    #[test]
    fn pairing_host_without_routes_prefers_physical_interfaces_then_private_ranges() {
        for name in [
            "usb0", "rndis0", "docker0", "veth0", "br-lan", "virbr0", "tun0", "tap0", "wg0",
        ] {
            let candidates = interfaces(&[(name, "192.168.68.1"), ("wlan0", "10.0.0.7")]);
            assert_eq!(
                lan_choice(&candidates, &[], None),
                Some([10, 0, 0, 7].into())
            );
        }
        for (list, expected) in [
            (
                vec![("eth0", "172.20.0.1"), ("wlan0", "10.0.0.7")],
                Some([10, 0, 0, 7].into()),
            ),
            (
                vec![("eth0", "10.0.0.7"), ("wlan0", "192.168.5.173")],
                Some([192, 168, 5, 173].into()),
            ),
            (
                vec![("eth0", "172.31.9.4"), ("wlan0", "172.32.0.1")],
                Some([172, 31, 9, 4].into()),
            ),
            (
                vec![
                    ("tun0", "198.18.0.1"),
                    ("wg0", "100.64.0.2"),
                    ("eth0", "169.254.3.3"),
                    ("lo", "127.0.0.1"),
                ],
                None,
            ),
        ] {
            assert_eq!(lan_choice(&interfaces(&list), &[], None), expected);
        }
        assert_eq!(lan_choice(&[], &[], None), None);
    }

    #[test]
    fn advertised_address_overrides_interfaces_and_routes_and_is_validated() {
        let override_ip =
            advertise_address(&args(&["--advertise-address", "192.168.9.8"])).unwrap();
        let dongle = interfaces(&[("usb0", "192.168.68.1"), ("wlan0", "192.168.5.173")]);
        assert_eq!(
            lan_choice(&dongle, &[("wlan0".into(), 100)], override_ip),
            override_ip
        );
        assert_eq!(lan_choice(&[], &[], override_ip), override_ip);
        assert_eq!(advertise_address(&args(&[])).unwrap(), None);
        for value in [
            "hostname",
            "::1",
            "192.168.1.999",
            "127.0.0.1",
            "0.0.0.0",
            "224.0.0.1",
            "255.255.255.255",
        ] {
            assert!(parse_arguments(["--advertise-address".into(), value.into()]).is_err());
        }
        assert!(parse_arguments(["--advertise-address".into()]).is_err());
    }

    #[test]
    fn interface_inventory_accepts_ip_and_ifconfig_formats() {
        let expected = interfaces(&[("usb0", "192.168.68.1"), ("wlan0", "192.168.5.173")]);
        assert_eq!(
            parse_interfaces(
                "2: usb0: <UP> inet 192.168.68.1/24 scope global usb0\n3: wlan0@if4: <UP> inet 192.168.5.173/24 scope global wlan0"
            ),
            expected
        );
        assert_eq!(
            parse_interfaces(
                "usb0 Link encap:Ethernet\n          inet addr:192.168.68.1 Bcast:192.168.68.255\nwlan0: flags=8863<UP>\n\tinet 192.168.5.173 netmask 0xffffff00"
            ),
            expected
        );
    }
    #[test]
    fn quick_connect_flags_are_validated_and_documented() {
        for (flag, good, bad) in [
            ("--clink-state-dir", "/tmp/state", ""),
            ("--clink-scan", "continuous", "forever"),
            (
                "--clink-phone-mac",
                "AA:BB:CC:DD:EE:FF",
                "02:00:00:00:00:00",
            ),
        ] {
            assert!(USAGE.contains(flag));
            assert!(clink_link(&args(&[
                "--clink-link",
                "ble",
                "--state-dir",
                "/tmp/auth",
                flag,
                good
            ]))
            .unwrap()
            .is_some());
            assert!(clink_link(&args(&[
                "--clink-link",
                "ble",
                "--state-dir",
                "/tmp/auth",
                flag,
                bad
            ]))
            .is_err());
            assert!(clink_link(&args(&[flag, good])).is_err());
            assert!(parse_arguments([flag.to_owned()]).is_err());
        }
    }
    #[test]
    fn host_ui_flag_is_boolean_and_documented() {
        assert!(args(&["--host-ui"]).get("--host-ui").is_some());
        assert!(USAGE.contains("--host-ui"));
    }
    #[test]
    fn link_failure_and_raw_records_reach_both_event_ports() {
        struct Collect(Mutex<Vec<serde_json::Value>>);
        impl ClinkTrace for Collect {
            fn record(&self, event: ClinkTraceEvent) {
                self.0.lock().unwrap().push(event.to_json());
            }
        }
        let trace = Collect(Mutex::default());
        let failure = oxp_link_linux::LinkEvent::Failure(oxp_link_linux::LinkFailure {
            stage: "wifi_join".into(),
            code: "system_tool_failed".into(),
            elapsed_ms: 70,
            total_elapsed_ms: 3000,
            attempt: 2,
            os_code: None,
            operation: Some("nmcli".into()),
            status: Some(7),
        });
        trace_link(&trace, &failure);
        let event = link_event(failure);
        assert_eq!(event["state"], "failed");
        assert_eq!(event["stage"], "wifi_join");
        assert_eq!(event["code"], "system_tool_failed");
        assert_eq!(event["elapsedMs"], 70);
        assert_eq!(event["totalElapsedMs"], 3000);
        assert_eq!(event["attempt"], 2);
        assert_eq!(event["status"], 7);
        let raw = oxp_link_linux::LinkEvent::Trace {
            stage: "ble_raw".into(),
            fields: [("raw".into(), "0100".into()), ("rssi".into(), "-50".into())].into(),
        };
        trace_link(&trace, &raw);
        assert_eq!(link_event(raw)["fields"]["raw"], "0100");
        let events = trace.0.lock().unwrap();
        assert_eq!(events[0]["stage"], "link_failure");
        assert_eq!(events[0]["fields"]["attempt"], "2");
        assert_eq!(events[0]["fields"]["status"], "7");
        assert_eq!(events[1]["stage"], "ble_raw");
        assert_eq!(events[1]["fields"]["raw"], "0100");
    }
    #[test]
    fn advertisement_trace_has_only_decoded_contract_fields() {
        struct Collect(Mutex<Vec<serde_json::Value>>);
        impl ClinkTrace for Collect {
            fn record(&self, event: ClinkTraceEvent) {
                self.0.lock().unwrap().push(event.to_json());
            }
        }
        let trace = Collect(Mutex::default());
        trace_link(
            &trace,
            &oxp_link_linux::LinkEvent::Advertisement {
                advertisement: oxp_clink::ble::Advertisement {
                    major: 1,
                    minor: 2,
                    serial: 3,
                    car_id: "car".into(),
                    model_id: "model".into(),
                    vendor_data: "private".into(),
                    name: "unit".into(),
                },
                serial_same: true,
            },
        );
        let values = trace.0.lock().unwrap();
        assert_eq!(values[0]["stage"], "ble_advertisement");
        assert_eq!(
            values[0]["fields"],
            json!({"major":"1","minor":"2","serial":"3","carId":"car","modelId":"model","vendorData":"private","name":"unit","serialSame":"true"})
        );
    }
    #[test]
    fn carlink_phone_mac_restart_failure_and_pairing_cancel_keep_the_browser_socket_alive() {
        use oxp_core::model::*;
        use std::io::{Read, Write};
        struct Host;
        impl OutputHost for Host {
            fn capabilities(&self, _: OutputCapabilities) {}
            fn control(&self, _: ControlEvent) {}
            fn stats(&self, _: OutputStats) {}
        }
        fn send(stream: &mut std::net::TcpStream, opcode: u8, payload: &[u8]) {
            let mut frame = vec![0x80 | opcode];
            if payload.len() < 126 {
                frame.push(0x80 | payload.len() as u8);
            } else {
                frame.push(0x80 | 126);
                frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
            }
            frame.extend_from_slice(&[0; 4]);
            frame.extend_from_slice(payload);
            stream.write_all(&frame).unwrap();
        }
        fn read(stream: &mut std::net::TcpStream) -> (u8, Vec<u8>) {
            let mut header = [0; 2];
            stream.read_exact(&mut header).unwrap();
            let mut length = (header[1] & 127) as usize;
            if length == 126 {
                let mut n = [0; 2];
                stream.read_exact(&mut n).unwrap();
                length = u16::from_be_bytes(n) as usize;
            }
            assert!(length < 65536);
            let mut payload = vec![0; length];
            stream.read_exact(&mut payload).unwrap();
            (header[0] & 15, payload)
        }
        fn assert_alive(browser: &mut std::net::TcpStream) {
            send(browser, 9, b"alive");
            loop {
                let (opcode, payload) = read(browser);
                assert_ne!(opcode, 8);
                if opcode == 10 {
                    assert_eq!(payload, b"alive");
                    break;
                }
            }
        }
        let root = std::env::temp_dir().join(format!("oxp-w4-browser-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let shutdown = Arc::new(RelayShutdown::default());
        shutdown.keep_running.store(true, Ordering::Release);
        let hub = Hub::new(Arc::new(Host));
        let web = Arc::new(
            WebOutput::new(WebOutputConfig::new(
                "127.0.0.1:0".parse().unwrap(),
                root.clone(),
                Pairing::Disabled,
                Tls::Disabled,
            ))
            .unwrap(),
        );
        hub.add_output(web.clone()).unwrap();
        let address = web.await_listening(Duration::from_secs(3)).unwrap();
        let mut browser = std::net::TcpStream::connect(address).unwrap();
        browser
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        browser.write_all(b"GET /oxp/v0 HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n").unwrap();
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            browser.read_exact(&mut byte).unwrap();
            headers.push(byte[0]);
        }
        assert!(String::from_utf8(headers)
            .unwrap()
            .contains("101 Switching Protocols"));
        send(
            &mut browser,
            1,
            include_bytes!("../../../web-receiver/spec/vectors/web/hello.json"),
        );
        let (_, welcome) = read(&mut browser);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&welcome).unwrap()["type"],
            "welcome"
        );
        let builds = Arc::new(Mutex::new(Vec::new()));
        let observed = builds.clone();
        let fail = Arc::new(AtomicBool::new(false));
        let fail_build = fail.clone();
        let factory = CarlinkFactory::from_args(
            &args(&[
                "--unix",
                "wire",
                "--clink-link",
                "ble",
                "--state-dir",
                root.to_str().unwrap(),
                "--clink-phone-mac",
                "02:11:22:33:44:55",
                "--clink-transcode",
                "off",
                "--control-stdin",
            ]),
            Arc::new(move |_, phone_mac| {
                observed.lock().unwrap().push(phone_mac);
                if fail_build.load(Ordering::Acquire) {
                    return Err("fake_link_build_failure".into());
                }
                Ok(Arc::new(FakeLink(Arc::new(
                    std::sync::atomic::AtomicUsize::new(0),
                ))))
            }),
            Arc::new(SessionTrace {
                summary: Arc::new(SummaryTrace::new(Arc::new(|_| {}))),
                recorder: None,
            }),
        )
        .unwrap();
        let runtime = CarlinkRuntime::new(factory, shutdown.clone(), hub.clone());
        for (mut command, expected) in [
            (
                json!({"picture":{"width":1280,"height":720}}),
                Some("02:11:22:33:44:55"),
            ),
            (
                json!({"phoneMac":"AA:BB:CC:DD:EE:FF","picture":{"width":1920,"height":1080}}),
                Some("aa:bb:cc:dd:ee:ff"),
            ),
            (json!({}), Some("aa:bb:cc:dd:ee:ff")),
            (json!({"phoneMac":null}), None),
            (json!({}), None),
            (
                json!({"phoneMac":"12:34:56:78:9A:BC","hevc":"signal","vehicleData":true}),
                Some("12:34:56:78:9a:bc"),
            ),
        ] {
            command["type"] = json!("clink-restart");
            command["request"] = json!("relay-N");
            let result = runtime.control(&command);
            assert_eq!(result["result"], "started");
            assert_eq!(result["request"], "relay-N");
            assert_eq!(builds.lock().unwrap().last().unwrap().as_deref(), expected);
            assert_alive(&mut browser);
            assert!(!shutdown.session_failed.load(Ordering::Acquire));
        }
        let generation = shutdown.generation.load(Ordering::Acquire);
        assert!(runtime
            .control(&json!({"type":"clink-restart","picture":{"width":321,"height":240}}))
            ["error"]
            .is_string());
        assert_eq!(shutdown.generation.load(Ordering::Acquire), generation);
        let active = runtime
            .current
            .lock()
            .unwrap()
            .generation
            .as_ref()
            .unwrap()
            .active
            .clone();
        let build_count = builds.lock().unwrap().len();
        for invalid in [
            json!(""),
            json!("00:00:00:00:00:00"),
            json!("02:00:00:00:00:00"),
            json!("ff:ff:ff:ff:ff:ff"),
            json!("01:12:34:56:78:90"),
            json!("aa:bb:cc:dd:ee"),
            json!("aa:bb:cc:dd:ee:gg"),
            json!("+2:00:00:00:00:01"),
            json!(" aa:bb:cc:dd:ee:ff"),
            json!("aa:bb:cc:dd:ee:ff\n"),
            json!(false),
            json!(1),
            json!([]),
            json!({}),
        ] {
            let response = runtime.control(&json!({"type":"clink-restart","request":"invalid","phoneMac":invalid,"picture":null,"hevc":"off","vehicleData":false}));
            assert!(response["error"].is_string(), "{invalid}");
            assert_eq!(response["request"], "invalid");
            assert_eq!(response["generation"], generation);
            assert!(*active.lock().unwrap());
            let current = runtime.current.lock().unwrap();
            assert_eq!(current.phone_mac.as_deref(), Some("12:34:56:78:9a:bc"));
            assert_eq!(current.options.picture_size, Some((1920, 1080)));
            assert_eq!(current.options.hevc, oxp_clink::ClinkHevcMode::Signal);
            assert!(current.options.vehicle_data);
            assert_eq!(builds.lock().unwrap().len(), build_count);
        }
        assert_alive(&mut browser);
        fail.store(true, Ordering::Release);
        assert_eq!(
            runtime.control(&json!({"type":"clink-restart","phoneMac":"AA:BB:CC:DD:EE:FF"}))
                ["error"],
            "fake_link_build_failure"
        );
        assert!(runtime.current.lock().unwrap().generation.is_none());
        assert_alive(&mut browser);
        fail.store(false, Ordering::Release);
        assert_eq!(
            runtime.control(&json!({"type":"clink-restart"}))["result"],
            "started"
        );
        assert_eq!(
            builds.lock().unwrap().last().unwrap().as_deref(),
            Some("aa:bb:cc:dd:ee:ff")
        );
        assert_alive(&mut browser);
        let pins = runtime
            .current
            .lock()
            .unwrap()
            .generation
            .as_ref()
            .unwrap()
            .pins
            .clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            pins.ask(
                "car",
                true,
                Duration::from_secs(120),
                &AtomicBool::new(false),
                |v| {
                    let _ = tx.send(v);
                },
            )
        });
        let prompt = rx.recv().unwrap();
        assert_eq!(
            runtime.control(
                &json!({"type":"clink-pairing","request":prompt["request"],"action":"cancel"})
            )["result"],
            "cancelled"
        );
        assert!(waiter.join().unwrap().is_none());
        assert_alive(&mut browser);
        assert!(runtime.current.lock().unwrap().generation.is_none());
        assert_eq!(
            runtime.control(&json!({"type":"clink-restart"}))["result"],
            "started"
        );
        assert_eq!(
            builds.lock().unwrap().last().unwrap().as_deref(),
            Some("aa:bb:cc:dd:ee:ff")
        );
        assert_alive(&mut browser);
        hub.close();
        runtime.factory.trace.summary.shutdown();
        std::fs::remove_dir_all(root).unwrap();
    }
}
