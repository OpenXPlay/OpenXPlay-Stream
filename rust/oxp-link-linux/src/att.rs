// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use oxp_clink::{ble, Result};
use std::time::{Duration, Instant};

pub(crate) trait Socket {
    fn send(&mut self, pdu: &[u8]) -> Result<()>;
    fn receive(&mut self) -> Result<Vec<u8>>;
    fn set_deadline(&mut self, deadline: Instant);
    fn cancel_prepared(&mut self) -> Result<()> {
        self.send(&[0x18, 0])
    }
}

fn word(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]])
}
fn pdu(op: u8, words: &[u16], data: &[u8]) -> Vec<u8> {
    let mut out = vec![op];
    for w in words {
        out.extend(w.to_le_bytes());
    }
    out.extend(data);
    out
}
fn uuid(raw: &[u8]) -> Result<String> {
    if raw.len() == 2 {
        return Ok(format!("{:08x}-0000-1000-8000-00805f9b34fb", word(raw)));
    }
    if raw.len() != 16 {
        return Err("invalid_bluetooth_attribute".into());
    }
    let hex: String = raw.iter().rev().map(|b| format!("{b:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

#[derive(Debug)]
pub(crate) struct Rejected(u8);
impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "bluetooth_attribute_rejected: {:02x}", self.0)
    }
}
impl std::error::Error for Rejected {}

#[derive(Default)]
struct Collector {
    bytes: Vec<u8>,
    stack: Vec<u8>,
    fragments: usize,
    started: bool,
    quoted: bool,
    escaped: bool,
}
impl Collector {
    fn feed(&mut self, fragment: &[u8]) -> Result<Option<&[u8]>> {
        self.fragments += 1;
        if self.fragments > 128 || self.bytes.len() + fragment.len() > 4096 {
            return Err("network_bootstrap_limit".into());
        }
        self.bytes.extend(fragment);
        let mut complete = false;
        for &b in fragment {
            if complete {
                if !b.is_ascii_whitespace() {
                    return Err("invalid_network_bootstrap".into());
                }
                continue;
            }
            if !self.started {
                if b.is_ascii_whitespace() {
                    continue;
                }
                if b != b'{' {
                    return Err("invalid_network_bootstrap".into());
                }
                self.started = true;
                self.stack.push(b);
            } else if self.quoted {
                if self.escaped {
                    self.escaped = false;
                } else if b == b'\\' {
                    self.escaped = true;
                } else if b == b'"' {
                    self.quoted = false;
                }
            } else {
                match b {
                    b'"' => self.quoted = true,
                    b'{' | b'[' => {
                        self.stack.push(b);
                        if self.stack.len() > 16 {
                            return Err("network_bootstrap_limit".into());
                        }
                    }
                    b'}' | b']' => {
                        if self.stack.pop() != Some(if b == b'}' { b'{' } else { b'[' }) {
                            return Err("invalid_network_bootstrap".into());
                        }
                        complete = self.stack.is_empty();
                    }
                    _ => {}
                }
            }
        }
        // Decode only after framing is complete: a fragment may split a UTF-8 codepoint.
        Ok(complete.then_some(self.bytes.as_slice()))
    }
}

struct Session<'a, S> {
    socket: &'a mut S,
    mtu: usize,
    received: usize,
    notify: u16,
    collector: Collector,
    reply: Option<ble::HeadUnitReply>,
    prepared: bool,
    descriptor_range: u16,
    subscription_value: u16,
    service_changed: u16,
    changed: bool,
    written: bool,
    diagnostic: Option<&'a mut dyn FnMut(&str)>,
}
impl<S: Socket> Session<'_, S> {
    fn report(&mut self, reason: &str) {
        if let Some(diagnostic) = &mut self.diagnostic {
            diagnostic(reason);
        }
    }
    fn receive(&mut self) -> Result<Vec<u8>> {
        let b = self.socket.receive()?;
        self.received += 1;
        if b.is_empty() || b.len() > self.mtu || self.received > 512 {
            return Err("bluetooth_packet_limit".into());
        }
        Ok(b)
    }
    fn notification(&mut self, b: &[u8]) -> Result<()> {
        if b.len() < 3 || word(&b[1..]) == 0 {
            return Err("invalid_bluetooth_notification".into());
        }
        // Confirm every indication, including unrelated handles, before decoding.
        if b[0] == 0x1d {
            self.socket.send(&[0x1e])?;
        }
        if word(&b[1..]) == self.service_changed && self.service_changed != 0 {
            if b.len() != 7 || word(&b[3..]) == 0 || word(&b[3..]) > word(&b[5..]) {
                return Err("invalid_service_changed".into());
            }
            self.report("service_changed");
            if self.written && self.reply.is_none() {
                self.report("head_unit_rejected_request");
                return Err("head_unit_rejected_request".into());
            }
            self.changed = true;
            return Ok(());
        }
        if word(&b[1..]) != self.notify {
            return Ok(());
        }
        if self.reply.is_some() {
            return Err("duplicate_network_bootstrap".into());
        }
        if let Some(raw) = self.collector.feed(&b[3..])? {
            self.reply = Some(ble::head_unit_reply(raw)?);
        }
        Ok(())
    }
    fn request(&mut self, b: &[u8], op: u8) -> Result<Vec<u8>> {
        if b.len() > self.mtu {
            return Err("bluetooth_request_limit".into());
        }
        self.socket.send(b)?;
        loop {
            let r = self.receive()?;
            match r[0] {
                0x1b | 0x1d => {
                    self.notification(&r)?;
                    continue;
                }
                1 => {
                    if r.len() != 5 || r[1] != b[0] {
                        return Err("invalid_bluetooth_response".into());
                    }
                    return Err(Box::new(Rejected(r[4])));
                }
                code if code == op => return Ok(r),
                _ => return Err("unexpected_bluetooth_response".into()),
            }
        }
    }
    fn entries(
        &mut self,
        op: u8,
        response: u8,
        mut start: u16,
        end: u16,
        kind: u16,
        sizes: &[u8],
    ) -> Result<Vec<Vec<u8>>> {
        let mut out = Vec::new();
        for _ in 0..128 {
            let r = match self.request(&pdu(op, &[start, end, kind], &[]), response) {
                Ok(r) => r,
                Err(e) if e.downcast_ref::<Rejected>().is_some_and(|e| e.0 == 10) => {
                    return Ok(out)
                }
                Err(e) => return Err(e),
            };
            if r.len() <= 2 || !sizes.contains(&r[1]) || (r.len() - 2) % usize::from(r[1]) != 0 {
                return Err("invalid_bluetooth_discovery".into());
            }
            let mut previous = u32::from(start) - 1;
            for entry in r[2..].chunks(usize::from(r[1])) {
                let handle = u32::from(word(entry));
                let last = if kind == 0x2800 {
                    u32::from(word(&entry[2..]))
                } else {
                    handle
                };
                if handle <= previous || last < handle || last > u32::from(end) {
                    return Err("invalid_bluetooth_attribute_range".into());
                }
                previous = last;
                out.push(entry.to_vec());
                if out.len() > 1024 {
                    return Err("bluetooth_discovery_limit".into());
                }
            }
            if previous == u32::from(end) {
                return Ok(out);
            }
            start = (previous + 1) as u16;
        }
        Err("bluetooth_discovery_limit".into())
    }
    fn discover(&mut self) -> Result<u16> {
        let mut ranges = Vec::new();
        let mut gatt = Vec::new();
        for e in self.entries(0x10, 0x11, 1, 65535, 0x2800, &[6, 20])? {
            if uuid(&e[4..])? == "00001801-0000-1000-8000-00805f9b34fb" {
                gatt.push((word(&e), word(&e[2..])));
            }
            if uuid(&e[4..])? == ble::GATT_SERVICE {
                ranges.push((word(&e), word(&e[2..])));
            }
        }
        if ranges.len() != 1 {
            return Err("bluetooth_handoff_service_missing".into());
        }
        for (start, end) in gatt {
            for e in self.entries(8, 9, start, end, 0x2803, &[7, 21])? {
                if uuid(&e[5..])? == "00002a05-0000-1000-8000-00805f9b34fb" {
                    self.service_changed = word(&e[3..]);
                }
            }
        }
        let (start, end) = ranges[0];
        let mut phone = None;
        let mut vehicle = None;
        let characteristics = self.entries(8, 9, start, end, 0x2803, &[7, 21])?;
        for e in &characteristics {
            let handle = word(&e[3..]);
            if handle <= word(e) || handle > end {
                return Err("invalid_bluetooth_attribute_range".into());
            }
            let id = uuid(&e[5..])?;
            let slot = if id == ble::PHONE_CHARACTERISTIC {
                &mut phone
            } else if id == ble::HEAD_UNIT_CHARACTERISTIC {
                &mut vehicle
            } else {
                continue;
            };
            if slot.replace((handle, e[2])).is_some() {
                return Err("ambiguous_bluetooth_characteristic".into());
            }
        }
        let (phone, properties) = phone.ok_or("bluetooth_phone_characteristic_missing")?;
        if properties & 8 == 0 {
            return Err("bluetooth_phone_characteristic_missing".into());
        }
        let (vehicle, properties) = vehicle.ok_or("bluetooth_vehicle_characteristic_missing")?;
        if properties & 0x30 == 0 || phone == vehicle {
            return Err("bluetooth_vehicle_characteristic_missing".into());
        }
        self.notify = vehicle;
        self.descriptor_range = characteristics
            .iter()
            .map(|e| word(e))
            .filter(|h| *h > vehicle)
            .min()
            .map_or(end, |h| h - 1);
        self.subscription_value = if properties & 0x20 != 0 { 2 } else { 1 };
        Ok(phone)
    }
    fn subscribe(&mut self, end: Instant) -> Result<()> {
        let mut start = self.notify.checked_add(1).ok_or("bluetooth_cccd_missing")?;
        let mut cccd = None;
        while start <= self.descriptor_range {
            let r = match self.request(&pdu(4, &[start, self.descriptor_range], &[]), 5) {
                Ok(r) => r,
                Err(e) if e.downcast_ref::<Rejected>().is_some_and(|e| e.0 == 10) => break,
                Err(e) => return Err(e),
            };
            let size = match r.get(1) {
                Some(1) => 4,
                Some(2) => 18,
                _ => return Err("invalid_bluetooth_discovery".into()),
            };
            if r.len() <= 2 || (r.len() - 2) % size != 0 {
                return Err("invalid_bluetooth_discovery".into());
            }
            let mut last = start - 1;
            for entry in r[2..].chunks(size) {
                let handle = word(entry);
                if handle <= last || handle > self.descriptor_range {
                    return Err("invalid_bluetooth_attribute_range".into());
                }
                last = handle;
                if uuid(&entry[2..])? == ble::CCCD && cccd.replace(handle).is_some() {
                    return Err("ambiguous_bluetooth_cccd".into());
                }
            }
            if last == self.descriptor_range {
                break;
            }
            start = last + 1;
        }
        let handle = cccd.ok_or("bluetooth_cccd_missing")?;
        self.socket
            .set_deadline(end.min(Instant::now() + Duration::from_secs(2)));
        let result = self.write(handle, &self.subscription_value.to_le_bytes());
        self.socket.set_deadline(end);
        match result {
            Err(e) if e.to_string() == "bluetooth_handoff_timeout" => {
                Err("bluetooth_cccd_timeout".into())
            }
            other => other,
        }
    }
    fn write(&mut self, handle: u16, payload: &[u8]) -> Result<()> {
        if payload.len() <= self.mtu - 3 {
            let r = self.request(&pdu(0x12, &[handle], payload), 0x13)?;
            if r.len() != 1 {
                return Err("invalid_bluetooth_write_response".into());
            }
            return Ok(());
        }
        self.prepared = true;
        for (i, chunk) in payload.chunks(self.mtu - 5).enumerate() {
            let req = pdu(0x16, &[handle, (i * (self.mtu - 5)) as u16], chunk);
            let r = self.request(&req, 0x17)?;
            if r[1..] != req[1..] {
                return Err("bluetooth_write_echo_mismatch".into());
            }
        }
        let r = self.request(&[0x18, 1], 0x19)?;
        self.prepared = false;
        if r.len() != 1 {
            return Err("invalid_bluetooth_write_response".into());
        }
        Ok(())
    }
}

pub(crate) fn handoff<S: Socket>(
    socket: &mut S,
    payload: &[u8],
    legacy: bool,
    end: Instant,
    diagnostic: &mut dyn FnMut(&str),
) -> Result<ble::HeadUnitReply> {
    if payload.is_empty() || payload.len() > 512 {
        return Err("phone_bootstrap_limit".into());
    }
    let mut s = Session {
        socket,
        mtu: 23,
        received: 0,
        notify: 0,
        collector: Collector::default(),
        reply: None,
        prepared: false,
        descriptor_range: 0,
        subscription_value: 0,
        service_changed: 0,
        changed: false,
        written: false,
        diagnostic: Some(diagnostic),
    };
    let result = (|| {
        let r = s.request(&[2, 0, 2], 3)?;
        if r.len() != 3 || !(23..=517).contains(&word(&r[1..])) {
            return Err("invalid_bluetooth_mtu".into());
        }
        s.mtu = usize::from(word(&r[1..])).min(512);
        let phone = loop {
            s.changed = false;
            let phone = s.discover()?;
            if s.changed {
                continue;
            }
            s.report(if legacy {
                "subscription_legacy"
            } else {
                "subscription_cccd"
            });
            if !legacy {
                s.subscribe(end)?;
            }
            if !s.changed {
                break phone;
            }
        };
        s.written = true;
        if let Err(e) = s.write(phone, payload) {
            // Some devices close ATT immediately after delivering valid credentials.
            if s.reply.is_none() {
                return Err(e);
            }
        }
        while s.reply.is_none() {
            let b = s.receive()?;
            if b[0] != 0x1b && b[0] != 0x1d {
                return Err("unexpected_bluetooth_notification".into());
            }
            s.notification(&b)?;
        }
        s.reply
            .take()
            .ok_or_else(|| "missing_network_bootstrap".into())
    })();
    if s.prepared
        && result
            .as_ref()
            .err()
            .is_none_or(|e| e.to_string() != "head_unit_rejected_request")
    {
        let _ = s.socket.cancel_prepared();
    }
    result
}

#[cfg(test)]
pub(crate) mod tests;
