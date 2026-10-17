// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use crate::{
    command::{run, Process},
    Config, Device,
};
use oxp_clink::{ble, Result};
use serde_json::Value;
use std::{
    collections::BTreeMap,
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
#[derive(Default)]
struct Entry {
    properties: serde_json::Map<String, Value>,
    fresh: bool,
}
struct Window {
    prefix: String,
    devices: BTreeMap<String, Entry>,
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
                    if let Some(p) = interfaces.get(DEVICE).and_then(Value::as_object) {
                        devices.insert(
                            path.clone(),
                            Entry {
                                properties: p.clone(),
                                fresh: false,
                            },
                        );
                    }
                }
            }
        }
        Self { prefix, devices }
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
        let entry = self.devices.entry(path.into()).or_default();
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
        let mut out = Vec::new();
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
                out.push(Device {
                    address: address.to_lowercase(),
                    advertisement,
                });
            }
        }
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
pub(crate) fn scan(c: &Config) -> Result<Vec<Device>> {
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
    let mut window = Window::new(&c.bluetooth_adapter, snapshot);
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
    let script = format!(
        "select {adapter}\nmenu scan\nuuids {}\ntransport le\nduplicate-data on\nback\nscan on\n",
        ble::ADVERTISEMENT_SERVICE
    );
    let input = discovery
        .0
        .stdin
        .as_mut()
        .ok_or("bluetooth_control_unavailable")?;
    input.write_all(script.as_bytes())?;
    input.flush()?;
    let start_deadline = Instant::now() + c.command_timeout;
    loop {
        #[cfg(target_os = "linux")]
        crate::signals::check()?;
        match started_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(result) => {
                result?;
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() < start_deadline => {}
            Err(_) => return Err("bluetooth_scan_start_timeout".into()),
        }
    }
    let end = Instant::now() + c.scan_timeout;
    while Instant::now() < end {
        #[cfg(target_os = "linux")]
        crate::signals::check()?;
        if overflow.load(std::sync::atomic::Ordering::Acquire) {
            return Err("bluetooth_scan_overflow".into());
        }
        if let Some(v) = next(
            &rx,
            end.saturating_duration_since(Instant::now())
                .min(Duration::from_millis(200)),
        )? {
            window.update(&v)?;
        }
    }
    // Include queued removals when the deadline wins.
    loop {
        match rx.try_recv() {
            Ok(v) => window.update(&v?)?,
            Err(TryRecvError::Empty) => break,
            Err(_) => return Err("bluetooth_monitor_closed".into()),
        }
    }
    if overflow.load(std::sync::atomic::Ordering::Acquire) {
        return Err("bluetooth_scan_overflow".into());
    }
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
    #[test]
    fn busctl_variants() {
        assert_eq!(
            data(r#"{"type":"v","data":{"type":"s","data":"aa:bb:cc:dd:ee:ff"}}"#).unwrap(),
            json!("aa:bb:cc:dd:ee:ff")
        );
    }
}
