// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use crate::{
    command::{run, Process},
    Config, Device, LinkEvent, ScanMode,
};
use oxp_clink::{ble, Result};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet},
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::{Duration, Instant},
};

const DEVICE: &str = "org.bluez.Device1";
// busctl wraps variants in {type,data}; containers remain JSON objects/arrays.
fn plain(v: &Value) -> Value {
    if let Some(o) = v.as_object() {
        if o.len() == 2 && o.contains_key("type") && o.contains_key("data") {
            return plain(&o["data"]);
        }
        return Value::Object(o.iter().map(|(k, v)| (k.clone(), plain(v))).collect());
    }
    if let Some(a) = v.as_array() {
        return Value::Array(a.iter().map(plain).collect());
    }
    v.clone()
}
fn data(s: &str) -> Result<Value> {
    let v: Value = serde_json::from_str(s).map_err(|_| "bluetooth_bus_invalid")?;
    Ok(plain(v.get("data").ok_or("bluetooth_bus_invalid")?))
}
fn bytes(v: &Value) -> Option<Vec<u8>> {
    v.as_array()?
        .iter()
        .map(|b| b.as_u64().and_then(|b| u8::try_from(b).ok()))
        .collect()
}
fn carlink_properties(properties: &serde_json::Map<String, Value>) -> bool {
    properties
        .get("UUIDs")
        .and_then(Value::as_array)
        .is_some_and(|uuids| {
            uuids.iter().any(|uuid| {
                uuid.as_str()
                    .is_some_and(|uuid| uuid.eq_ignore_ascii_case(ble::ADVERTISEMENT_SERVICE))
            })
        })
        || properties
            .get("ServiceData")
            .and_then(Value::as_object)
            .is_some_and(|data| {
                data.keys()
                    .any(|uuid| uuid.eq_ignore_ascii_case(ble::IDENTITY_SERVICE))
            })
}
#[derive(Default)]
struct Entry {
    properties: serde_json::Map<String, Value>,
    fresh: bool,
    observed: u64,
}
struct Window {
    prefix: String,
    devices: BTreeMap<String, Entry>,
    sequence: u64,
}
impl Window {
    fn new(adapter: &str, snapshot: Value) -> Self {
        let prefix = format!("/org/bluez/{adapter}/");
        let mut devices = BTreeMap::new();
        let objects = snapshot
            .as_array()
            .and_then(|a| a.first())
            .unwrap_or(&snapshot);
        if let Some(objects) = objects.as_object() {
            for (path, interfaces) in objects {
                if path.starts_with(&prefix) {
                    if let Some(p) = interfaces
                        .get(DEVICE)
                        .and_then(Value::as_object)
                        .filter(|p| carlink_properties(p))
                    {
                        devices.insert(
                            path.clone(),
                            Entry {
                                properties: p.clone(),
                                fresh: false,
                                observed: 0,
                            },
                        );
                    }
                }
            }
        }
        while devices.len() > 128 {
            devices.pop_first();
        }
        Self {
            prefix,
            devices,
            sequence: 0,
        }
    }
    fn update(&mut self, message: &Value) -> Result<()> {
        if message.get("type").and_then(Value::as_str) != Some("signal") {
            return Ok(());
        }
        let Some(payload) = message.get("payload") else {
            return Err("bluetooth_scan_invalid_signal".into());
        };
        let body = plain(payload);
        let body = body.as_array().ok_or("bluetooth_scan_invalid_signal")?;
        let member = message["member"].as_str().unwrap_or("");
        let (path, properties, invalidated) = match member {
            "InterfacesAdded" => {
                let path = body
                    .first()
                    .and_then(Value::as_str)
                    .ok_or("bluetooth_scan_invalid_signal")?;
                if !path.starts_with(&self.prefix) {
                    return Ok(());
                }
                let Some(p) = body.get(1).and_then(|v| v.get(DEVICE)) else {
                    return Ok(());
                };
                self.devices.remove(path);
                (path, p, None)
            }
            "InterfacesRemoved" => {
                let path = body
                    .first()
                    .and_then(Value::as_str)
                    .ok_or("bluetooth_scan_invalid_signal")?;
                if body
                    .get(1)
                    .and_then(Value::as_array)
                    .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(DEVICE)))
                {
                    self.devices.remove(path);
                }
                return Ok(());
            }
            "PropertiesChanged" => {
                if body.first().and_then(Value::as_str) != Some(DEVICE) {
                    return Ok(());
                }
                let path = message["path"]
                    .as_str()
                    .ok_or("bluetooth_scan_invalid_signal")?;
                (
                    path,
                    body.get(1).ok_or("bluetooth_scan_invalid_signal")?,
                    body.get(2),
                )
            }
            _ => return Ok(()),
        };
        if !path.starts_with(&self.prefix) {
            return Ok(());
        }
        let p = properties
            .as_object()
            .ok_or("bluetooth_scan_invalid_signal")?;
        // Raw records use the HCI observer; discovery retains only Carlink candidates.
        if !self.devices.contains_key(path) && !carlink_properties(p) {
            return Ok(());
        }
        if !self.devices.contains_key(path) && self.devices.len() >= 128 {
            if let Some(oldest) = self
                .devices
                .iter()
                .min_by_key(|(_, entry)| entry.observed)
                .map(|(path, _)| path.clone())
            {
                self.devices.remove(&oldest);
            }
        }
        self.sequence = self.sequence.saturating_add(1);
        let entry = self.devices.entry(path.into()).or_default();
        if p.contains_key("ServiceData")
            || p.contains_key("ManufacturerData")
            || p.contains_key("RSSI")
        {
            entry.observed = self.sequence;
        }
        entry.fresh |= p.contains_key("ServiceData") || p.contains_key("ManufacturerData");
        entry.properties.extend(p.clone());
        if let Some(a) = invalidated.and_then(Value::as_array) {
            for k in a.iter().filter_map(Value::as_str) {
                entry.properties.remove(k);
            }
        }
        Ok(())
    }
    fn result(&self) -> Vec<Device> {
        let mut newest = BTreeMap::new();
        for entry in self.devices.values().filter(|e| e.fresh) {
            let p = &entry.properties;
            let Some(address) = p.get("Address").and_then(Value::as_str) else {
                continue;
            };
            if crate::mac_bytes(address).is_err() {
                continue;
            }
            let Some(uuids) = p.get("UUIDs").and_then(Value::as_array) else {
                continue;
            };
            if !uuids.iter().any(|v| {
                v.as_str()
                    .is_some_and(|s| s.eq_ignore_ascii_case(ble::ADVERTISEMENT_SERVICE))
            }) {
                continue;
            }
            let Some(sd) = p.get("ServiceData").and_then(Value::as_object) else {
                continue;
            };
            let lookup = |key: &str| {
                sd.iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(key))
                    .and_then(|(_, v)| bytes(v))
            };
            let Some(identity) = lookup(ble::IDENTITY_SERVICE) else {
                continue;
            };
            let name = lookup(ble::NAME_SERVICE).unwrap_or_default();
            if let Ok(advertisement) = ble::advertisement(&identity, &name) {
                let device = Device {
                    address: address.to_lowercase(),
                    advertisement,
                };
                let key = if device.advertisement.car_id.is_empty() {
                    device.address.clone()
                } else {
                    device.advertisement.car_id.clone()
                };
                if newest
                    .get(&key)
                    .is_none_or(|(sequence, _)| *sequence < entry.observed)
                {
                    newest.insert(key, (entry.observed, device));
                }
            }
        }
        let mut out: Vec<Device> = newest.into_values().map(|(_, device)| device).collect();
        out.sort_by(|a, b| {
            (&a.advertisement.car_id, &a.address).cmp(&(&b.advertisement.car_id, &b.address))
        });
        out
    }
}
fn next(rx: &Receiver<Result<Value>>, wait: Duration) -> Result<Option<Value>> {
    match rx.recv_timeout(wait) {
        Ok(v) => v.map(Some),
        Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
        Err(_) => Err("bluetooth_monitor_closed".into()),
    }
}
pub(crate) fn scan(
    c: &Config,
    address: Option<&str>,
    car_id: Option<&str>,
    advertisement: &mut dyn FnMut(&Device),
    trace: &mut dyn FnMut(LinkEvent),
) -> Result<Vec<Device>> {
    let setup = Instant::now() + c.command_timeout;
    let mut monitor = Process(
        Command::new("busctl")
            .args(["--system", "--json=short", "monitor", "org.bluez"])
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "bluetooth_monitor_unavailable")?,
    );
    let output = monitor
        .0
        .stdout
        .take()
        .ok_or("bluetooth_monitor_unavailable")?;
    let (tx, rx) = mpsc::sync_channel(256);
    let overflow = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let lost = overflow.clone();
    let ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = ready.clone();
    thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            let message = line
                .map_err(|_| "bluetooth_monitor_failed".into())
                .and_then(|s| {
                    serde_json::from_str(&s).map_err(|_| "bluetooth_scan_invalid_signal".into())
                });
            if message.is_ok() {
                observed.store(true, std::sync::atomic::Ordering::Release);
            }
            match tx.try_send(message) {
                Ok(()) => {}
                Err(mpsc::TrySendError::Full(_)) => {
                    lost.store(true, std::sync::atomic::Ordering::Release);
                    break;
                }
                Err(_) => break,
            }
        }
    });
    // Probe traffic establishes subscription readiness without relying on busctl's
    // human-readable startup banner, which varies with JSON mode and systemd versions.
    let snapshot = loop {
        let snapshot = data(&run(
            "busctl",
            &[
                "--system",
                "--json=short",
                "call",
                "org.bluez",
                "/",
                "org.freedesktop.DBus.ObjectManager",
                "GetManagedObjects",
            ],
            setup,
        )?)?;
        if ready.load(std::sync::atomic::Ordering::Acquire) {
            break snapshot;
        }
        if Instant::now() >= setup {
            return Err("bluetooth_monitor_not_ready".into());
        }
        thread::sleep(Duration::from_millis(50));
    };
    let window = Window::new(&c.bluetooth_adapter, snapshot);
    // Discard events preceding our snapshot/start boundary; cached RSSI cannot qualify.
    while rx.try_recv().is_ok() {}
    // BlueZ discovery/filter ownership follows the D-Bus client. Keep bluetoothctl
    // alive; separate busctl calls would lose the filter and stop discovery on exit.
    let adapter = adapter_address(c)?;
    let mut discovery = Process(
        Command::new("bluetoothctl")
            .env("LC_ALL", "C")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "bluetooth_control_unavailable")?,
    );
    let output = discovery
        .0
        .stdout
        .take()
        .ok_or("bluetooth_control_unavailable")?;
    let (started_tx, started_rx) = mpsc::sync_channel(16);
    thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            let Ok(line) = line else { break };
            if line.contains("Discovery started") {
                let _ = started_tx.try_send(Ok(()));
            } else if line.contains("Failed")
                || line.contains("Invalid command")
                || line.contains("not available")
            {
                let _ = started_tx.try_send(Err("bluetooth_scan_failed"));
            }
        }
    });
    let service = if c.trace_advertisements {
        ""
    } else {
        ble::ADVERTISEMENT_SERVICE
    };
    let script = format!(
        "select {adapter}\nmenu scan\nuuids {}\ntransport le\nduplicate-data on\nback\nscan on\n",
        service
    );
    #[cfg(target_os = "linux")]
    let raw = if c.trace_advertisements {
        match crate::raw::RawSocket::open(&c.bluetooth_adapter) {
            Ok(socket) => Some(socket),
            Err(error) => {
                raw_failure(error.as_ref(), trace);
                None
            }
        }
    } else {
        None
    };
    let input = discovery
        .0
        .stdin
        .as_mut()
        .ok_or("bluetooth_control_unavailable")?;
    input.write_all(script.as_bytes())?;
    input.flush()?;
    wait_started(&started_rx, c.command_timeout)?;
    let mut discovery = NativeDiscovery {
        process: discovery,
        rx,
        started_rx,
        overflow,
        start: Instant::now(),
        command_timeout: c.command_timeout,
        #[cfg(target_os = "linux")]
        raw,
    };
    scan_discovery(
        c,
        address,
        car_id,
        window,
        &mut discovery,
        advertisement,
        trace,
    )
}
fn wait_started(
    rx: &Receiver<std::result::Result<(), &'static str>>,
    timeout: Duration,
) -> Result<()> {
    let end = Instant::now() + timeout;
    loop {
        #[cfg(target_os = "linux")]
        crate::signals::check()?;
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(result) => return result.map_err(Into::into),
            Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() < end => {}
            Err(_) => return Err("bluetooth_scan_start_timeout".into()),
        }
    }
}
fn raw_failure(error: &(dyn std::error::Error + 'static), trace: &mut dyn FnMut(LinkEvent)) {
    let mut fields = BTreeMap::from([("code".into(), crate::diagnostics::code(error))]);
    if let Some(code) = error
        .downcast_ref::<std::io::Error>()
        .and_then(|e| e.raw_os_error())
    {
        fields.insert("osCode".into(), code.to_string());
    }
    trace(LinkEvent::Trace {
        stage: "ble_raw_failed".into(),
        fields,
    });
}
trait Discovery {
    fn raw(&mut self, _: &mut dyn FnMut(BTreeMap<String, String>)) -> Result<()> {
        Ok(())
    }
    fn elapsed(&self) -> Duration;
    fn receive(&mut self, wait: Duration) -> Result<Option<Value>>;
    fn restart(&mut self) -> Result<()>;
    fn check(&mut self) -> Result<()>;
}
struct NativeDiscovery {
    process: Process,
    rx: Receiver<Result<Value>>,
    started_rx: Receiver<std::result::Result<(), &'static str>>,
    overflow: std::sync::Arc<std::sync::atomic::AtomicBool>,
    start: Instant,
    command_timeout: Duration,
    #[cfg(target_os = "linux")]
    raw: Option<crate::raw::RawSocket>,
}
impl Discovery for NativeDiscovery {
    fn raw(&mut self, emit: &mut dyn FnMut(BTreeMap<String, String>)) -> Result<()> {
        #[cfg(target_os = "linux")]
        if let Some(raw) = &self.raw {
            if let Err(error) = raw.drain(emit) {
                self.raw = None;
                return Err(error);
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = emit;
        Ok(())
    }
    fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }
    fn receive(&mut self, wait: Duration) -> Result<Option<Value>> {
        next(&self.rx, wait)
    }
    fn restart(&mut self) -> Result<()> {
        while self.started_rx.try_recv().is_ok() {}
        let input = self
            .process
            .0
            .stdin
            .as_mut()
            .ok_or("bluetooth_control_unavailable")?;
        input.write_all(b"scan off\nscan on\n")?;
        input.flush()?;
        wait_started(&self.started_rx, self.command_timeout)
    }
    fn check(&mut self) -> Result<()> {
        #[cfg(target_os = "linux")]
        crate::signals::check()?;
        if self.overflow.load(std::sync::atomic::Ordering::Acquire) {
            return Err("bluetooth_scan_overflow".into());
        }
        if self.process.0.try_wait()?.is_some() {
            return Err("bluetooth_scan_failed".into());
        }
        match self.started_rx.try_recv() {
            Ok(Err(reason)) => Err(reason.into()),
            Err(TryRecvError::Disconnected) => Err("bluetooth_scan_failed".into()),
            _ => Ok(()),
        }
    }
}
fn scan_discovery(
    c: &Config,
    address: Option<&str>,
    car_id: Option<&str>,
    mut window: Window,
    discovery: &mut dyn Discovery,
    advertisement: &mut dyn FnMut(&Device),
    trace: &mut dyn FnMut(LinkEvent),
) -> Result<Vec<Device>> {
    let mut capture = crate::raw::Capture::default();
    let mut drain = |discovery: &mut dyn Discovery| {
        if !c.trace_advertisements {
            return;
        }
        let now = discovery.elapsed();
        let mut records = vec![];
        if let Err(error) = discovery.raw(&mut |fields| records.push(fields)) {
            raw_failure(error.as_ref(), trace);
        }
        for fields in records {
            capture.record(fields, now, trace);
        }
    };
    let result = scan_loop(
        c,
        address,
        car_id,
        &mut window,
        discovery,
        advertisement,
        &mut drain,
    );
    drain(discovery);
    if c.trace_advertisements {
        capture.finish(trace);
    }
    result
}
fn scan_loop(
    c: &Config,
    address: Option<&str>,
    car_id: Option<&str>,
    window: &mut Window,
    discovery: &mut dyn Discovery,
    advertisement: &mut dyn FnMut(&Device),
    drain: &mut dyn FnMut(&mut dyn Discovery),
) -> Result<Vec<Device>> {
    let mut renewal = Duration::from_secs(600);
    let mut seen = HashSet::new();
    loop {
        discovery.check()?;
        drain(discovery);
        let now = discovery.elapsed();
        if c.scan_mode == ScanMode::Window && now >= c.scan_timeout {
            break;
        }
        if c.scan_mode == ScanMode::Continuous && now >= renewal {
            discovery.restart()?;
            renewal = discovery.elapsed() + Duration::from_secs(600);
        }
        let wait = if c.scan_mode == ScanMode::Window {
            c.scan_timeout.saturating_sub(now)
        } else {
            renewal.saturating_sub(discovery.elapsed())
        }
        .min(Duration::from_millis(200));
        if let Some(value) = discovery.receive(wait)? {
            drain(discovery);
            window.update(&value)?;
            let devices = window.result();
            if seen.len() >= 128 {
                seen.clear();
            }
            for device in &devices {
                if seen.insert((
                    device.advertisement.car_id.clone(),
                    device.advertisement.serial,
                )) {
                    advertisement(device);
                }
            }
            if c.scan_mode == ScanMode::Continuous {
                if let Some(device) = devices.into_iter().find(|d| {
                    car_id.map_or_else(
                        || address.is_none_or(|a| a.eq_ignore_ascii_case(&d.address)),
                        |id| d.advertisement.car_id == id,
                    )
                }) {
                    return Ok(vec![device]);
                }
            }
        }
    }
    // Include queued removals when the window deadline wins.
    while let Some(value) = discovery.receive(Duration::ZERO)? {
        window.update(&value)?;
        for device in window.result() {
            if seen.insert((
                device.advertisement.car_id.clone(),
                device.advertisement.serial,
            )) {
                advertisement(&device);
            }
        }
    }
    discovery.check()?;
    Ok(window.result())
}
pub(crate) fn adapter_address(c: &Config) -> Result<String> {
    let path = format!("/org/bluez/{}", c.bluetooth_adapter);
    let v = data(&run(
        "busctl",
        &[
            "--system",
            "--json=short",
            "get-property",
            "org.bluez",
            &path,
            "org.bluez.Adapter1",
            "Address",
        ],
        Instant::now() + c.command_timeout,
    )?)?;
    let address = v
        .as_str()
        .ok_or("bluetooth_adapter_unavailable")?
        .to_owned();
    crate::mac_bytes(&address)?;
    Ok(address)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn current_advertisement_required() {
        let path = "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_01";
        let p = json!({"Address":"aa:bb:cc:dd:ee:01", "UUIDs":[ble::ADVERTISEMENT_SERVICE], "ServiceData":{ble::IDENTITY_SERVICE:[1,9,0,0,1,2,3,4,5,6,7,8,9,18,52,255],ble::NAME_SERVICE:[67,97,114,0,0]}});
        let mut w = Window::new("hci0", json!([{path:{DEVICE:p}}]));
        assert!(w.result().is_empty());
        let changed = |p: Value, invalid: Value| json!({"type":"signal","path":path,"member":"PropertiesChanged","payload":{"type":"sa{sv}as","data":[DEVICE,p,invalid]}});
        w.update(&changed(json!({"RSSI":-50,"Connected":false}), json!([])))
            .unwrap();
        assert!(w.result().is_empty());
        w.update(&changed(json!({"ServiceData":p["ServiceData"]}), json!([])))
            .unwrap();
        assert_eq!(w.result()[0].advertisement.car_id, "000102030405");
        assert_eq!(w.result()[0].advertisement.model_id, "06070809");
        w.update(&changed(json!({}), json!(["ServiceData"])))
            .unwrap();
        assert!(w.result().is_empty());
        w.update(&json!({"type":"signal","member":"InterfacesAdded","payload":{"type":"oa{sa{sv}}","data":[path,{DEVICE:p}]}})).unwrap();
        assert_eq!(w.result().len(), 1);
        w.update(&json!({"type":"signal","member":"InterfacesRemoved","payload":{"type":"oas","data":[path,[DEVICE]]}})).unwrap();
        assert!(w.result().is_empty());
    }
    struct FakeDiscovery {
        now: Duration,
        events: std::collections::VecDeque<(Duration, Option<Value>)>,
        restarts: usize,
        error: Option<&'static str>,
    }
    impl Discovery for FakeDiscovery {
        fn elapsed(&self) -> Duration {
            self.now
        }
        fn receive(&mut self, _: Duration) -> Result<Option<Value>> {
            if let Some((after, value)) = self.events.pop_front() {
                self.now += after;
                Ok(value)
            } else {
                Ok(None)
            }
        }
        fn restart(&mut self) -> Result<()> {
            self.restarts += 1;
            Ok(())
        }
        fn check(&mut self) -> Result<()> {
            if let Some(reason) = self.error {
                Err(reason.into())
            } else {
                Ok(())
            }
        }
    }
    fn signal(serial: u8) -> Value {
        let identity = [1, 9, serial, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 18, 52, 255];
        json!({"type":"signal","member":"InterfacesAdded","payload":{"type":"oa{sa{sv}}","data":["/org/bluez/hci0/dev_unit",{DEVICE:{"Address":"aa:bb:cc:dd:ee:01","UUIDs":[ble::ADVERTISEMENT_SERVICE],"ServiceData":{ble::IDENTITY_SERVICE:identity}}}]}})
    }
    #[test]
    fn continuous_waits_renews_and_first_match_wins() {
        let mut discovery = FakeDiscovery {
            now: Duration::ZERO,
            events: [
                (Duration::from_secs(600), None),
                (Duration::from_secs(600), None),
                (Duration::from_secs(1), Some(signal(1))),
                (Duration::ZERO, Some(signal(2))),
            ]
            .into(),
            restarts: 0,
            error: None,
        };
        let c = Config {
            scan_mode: ScanMode::Continuous,
            trace_advertisements: true,
            ..Config::default()
        };
        let mut traces = vec![];
        let devices = scan_discovery(
            &c,
            None,
            None,
            Window::new("hci0", json!({})),
            &mut discovery,
            &mut |d| traces.push(d.advertisement.serial),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(discovery.restarts, 2);
        assert_eq!(devices[0].advertisement.serial, 1);
        assert_eq!(traces, [1]);
        assert_eq!(discovery.events.len(), 1);
    }
    #[test]
    fn scan_trace_deduplicates_per_car_serial_and_window_still_expires() {
        let mut discovery = FakeDiscovery {
            now: Duration::ZERO,
            events: [
                (Duration::ZERO, Some(signal(1))),
                (Duration::ZERO, Some(signal(1))),
                (Duration::ZERO, Some(signal(2))),
                (Duration::from_secs(8), None),
            ]
            .into(),
            restarts: 0,
            error: None,
        };
        let mut traces = vec![];
        let c = Config {
            trace_advertisements: true,
            ..Config::default()
        };
        scan_discovery(
            &c,
            None,
            None,
            Window::new("hci0", json!({})),
            &mut discovery,
            &mut |d| traces.push(d.advertisement.serial),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(traces, [1, 2]);
        assert_eq!(discovery.restarts, 0);
        for error in ["shutdown_requested", "bluetooth_scan_failed"] {
            discovery.error = Some(error);
            let c = Config {
                scan_mode: ScanMode::Continuous,
                ..Config::default()
            };
            assert_eq!(
                scan_discovery(
                    &c,
                    None,
                    None,
                    Window::new("hci0", json!({})),
                    &mut discovery,
                    &mut |_| {},
                    &mut |_| {}
                )
                .unwrap_err()
                .to_string(),
                error
            );
        }
    }
    #[test]
    fn latest_address_for_the_same_car_wins() {
        let mut window = Window::new("hci0", json!({}));
        window.update(&signal(1)).unwrap();
        let mut moved = signal(2);
        moved["payload"]["data"][0] = json!("/org/bluez/hci0/dev_rotated");
        moved["payload"]["data"][1][DEVICE]["Address"] = json!("aa:bb:cc:dd:ee:02");
        window.update(&moved).unwrap();
        let devices = window.result();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].address, "aa:bb:cc:dd:ee:02");
    }
    #[test]
    fn continuous_retry_ignores_other_cars_and_accepts_rotated_address() {
        let mut other = signal(1);
        other["payload"]["data"][1][DEVICE]["ServiceData"][ble::IDENTITY_SERVICE][3] = json!(99);
        let mut target = signal(2);
        target["payload"]["data"][0] = json!("/org/bluez/hci0/dev_rotated");
        target["payload"]["data"][1][DEVICE]["Address"] = json!("aa:bb:cc:dd:ee:02");
        let mut discovery = FakeDiscovery {
            now: Duration::ZERO,
            events: [
                (Duration::ZERO, Some(other)),
                (Duration::ZERO, Some(target)),
            ]
            .into(),
            restarts: 0,
            error: None,
        };
        let c = Config {
            scan_mode: ScanMode::Continuous,
            ..Config::default()
        };
        let devices = scan_discovery(
            &c,
            Some("aa:bb:cc:dd:ee:01"),
            Some("000102030405"),
            Window::new("hci0", json!({})),
            &mut discovery,
            &mut |_| {},
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(devices[0].address, "aa:bb:cc:dd:ee:02");
        assert!(discovery.events.is_empty());
    }
    #[test]
    fn raw_capture_only_runs_with_trace_and_always_reports_drops() {
        struct RawDiscovery {
            calls: usize,
            now: Duration,
        }
        impl Discovery for RawDiscovery {
            fn elapsed(&self) -> Duration {
                self.now
            }
            fn receive(&mut self, _: Duration) -> Result<Option<Value>> {
                self.now += Duration::from_secs(8);
                Ok(None)
            }
            fn restart(&mut self) -> Result<()> {
                Ok(())
            }
            fn check(&mut self) -> Result<()> {
                Ok(())
            }
            fn raw(&mut self, emit: &mut dyn FnMut(BTreeMap<String, String>)) -> Result<()> {
                self.calls += 1;
                emit(
                    [
                        ("address".into(), "aa:bb:cc:dd:ee:ff".into()),
                        ("raw".into(), "0100".into()),
                    ]
                    .into(),
                );
                Ok(())
            }
        }
        for trace in [false, true] {
            let mut discovery = RawDiscovery {
                calls: 0,
                now: Duration::ZERO,
            };
            let mut events = vec![];
            let c = Config {
                trace_advertisements: trace,
                ..Config::default()
            };
            scan_discovery(
                &c,
                None,
                None,
                Window::new("hci0", json!({})),
                &mut discovery,
                &mut |_| {},
                &mut |e| events.push(e),
            )
            .unwrap();
            assert_eq!(discovery.calls > 0, trace);
            assert_eq!(events.is_empty(), !trace);
            if trace {
                assert!(
                    matches!(events.last(),Some(LinkEvent::Trace{stage,..}) if stage=="ble_raw_dropped")
                );
            }
        }
    }
    #[test]
    fn busctl_variants() {
        assert_eq!(
            data(r#"{"type":"v","data":{"type":"s","data":"aa:bb:cc:dd:ee:ff"}}"#).unwrap(),
            json!("aa:bb:cc:dd:ee:ff")
        );
    }
}
