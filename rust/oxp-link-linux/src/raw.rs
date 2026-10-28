// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

use crate::LinkEvent;
use std::{
    collections::{BTreeMap, VecDeque},
    time::Duration,
};

type Fields = BTreeMap<String, String>;
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn uuid(bytes: &[u8]) -> Option<String> {
    match bytes.len() {
        2 => Some(format!(
            "{:08x}-0000-1000-8000-00805f9b34fb",
            u16::from_le_bytes(bytes.try_into().ok()?)
        )),
        4 => Some(format!(
            "{:08x}-0000-1000-8000-00805f9b34fb",
            u32::from_le_bytes(bytes.try_into().ok()?)
        )),
        16 => {
            let text = hex(&bytes.iter().rev().copied().collect::<Vec<_>>());
            Some(format!(
                "{}-{}-{}-{}-{}",
                &text[..8],
                &text[8..12],
                &text[12..16],
                &text[16..20],
                &text[20..]
            ))
        }
        _ => None,
    }
}
fn payload_fields(raw: &[u8], fields: &mut Fields) {
    let mut services = BTreeMap::new();
    let mut manufacturers = BTreeMap::new();
    let mut uuids = vec![];
    let mut rest = raw;
    while let Some((&len, body)) = rest.split_first() {
        if len == 0 || body.len() < len as usize {
            break;
        }
        let (ad, tail) = body.split_at(len as usize);
        rest = tail;
        let kind = ad[0];
        let bytes = &ad[1..];
        match kind {
            0x08 | 0x09 => {
                fields.insert(
                    "localName".into(),
                    String::from_utf8_lossy(bytes).trim_end_matches('\0').into(),
                );
            }
            0x02..=0x07 => {
                let size = match kind {
                    2 | 3 => 2,
                    4 | 5 => 4,
                    _ => 16,
                };
                uuids.extend(bytes.chunks_exact(size).filter_map(uuid));
            }
            0x16 | 0x20 | 0x21 => {
                let size = match kind {
                    0x16 => 2,
                    0x20 => 4,
                    _ => 16,
                };
                if bytes.len() >= size {
                    if let Some(key) = uuid(&bytes[..size]) {
                        services.insert(key, hex(&bytes[size..]));
                    }
                }
            }
            0x0a if bytes.len() == 1 => {
                fields
                    .entry("txPower".into())
                    .or_insert_with(|| (bytes[0] as i8).to_string());
            }
            0xff if bytes.len() >= 2 => {
                manufacturers.insert(
                    u16::from_le_bytes([bytes[0], bytes[1]]).to_string(),
                    hex(&bytes[2..]),
                );
            }
            _ => {}
        }
    }
    fields.entry("localName".into()).or_default();
    fields.insert(
        "serviceUuids".into(),
        serde_json::to_string(&uuids).unwrap(),
    );
    fields.insert(
        "serviceData".into(),
        serde_json::to_string(&services).unwrap(),
    );
    fields.insert(
        "manufacturerData".into(),
        serde_json::to_string(&manufacturers).unwrap(),
    );
}
/// Decode only LE advertising reports; unrelated HCI traffic never crosses the trace port.
fn reports(packet: &[u8]) -> Vec<Fields> {
    if packet.len() < 5
        || packet[0] != 4
        || packet[1] != 0x3e
        || packet[2] as usize + 3 != packet.len()
    {
        return vec![];
    }
    let extended = match packet[3] {
        2 => false,
        13 => true,
        _ => return vec![],
    };
    let mut rest = &packet[5..];
    let mut out = vec![];
    for _ in 0..packet[4] {
        let header = if extended { 24 } else { 9 };
        if rest.len() < header {
            break;
        }
        let (address_type, address, length, raw_start, rssi, event) = if extended {
            (
                rest[2],
                &rest[3..9],
                rest[23] as usize,
                24,
                rest[13] as i8,
                u16::from_le_bytes([rest[0], rest[1]]),
            )
        } else {
            (rest[1], &rest[2..8], rest[8] as usize, 9, 0, rest[0] as u16)
        };
        let total = raw_start + length + usize::from(!extended);
        if rest.len() < total {
            break;
        }
        let raw = &rest[raw_start..raw_start + length];
        let rssi = if extended {
            rssi
        } else {
            rest[raw_start + length] as i8
        };
        let address = address
            .iter()
            .rev()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(":");
        let mut fields: Fields = [
            ("address".into(), address),
            ("addressType".into(), address_type.to_string()),
            ("rssi".into(), rssi.to_string()),
            ("raw".into(), hex(raw)),
            (
                "legacy".into(),
                (!extended || event & 0x10 != 0).to_string(),
            ),
            (
                "connectable".into(),
                (if extended {
                    event & 1 != 0
                } else {
                    event == 0 || event == 1
                })
                .to_string(),
            ),
        ]
        .into();
        if extended {
            fields.insert("txPower".into(), (rest[12] as i8).to_string());
            fields.insert("dataStatus".into(), ((event >> 5) & 3).to_string());
        }
        payload_fields(raw, &mut fields);
        out.push(fields);
        rest = &rest[total..];
    }
    out
}

/// Android's 50 events/s and two-second address/payload duplicate window.
#[derive(Default)]
pub(crate) struct Capture {
    recent: BTreeMap<(String, String), Duration>,
    emitted: VecDeque<Duration>,
    dropped: u64,
}
impl Capture {
    pub(crate) fn record(
        &mut self,
        fields: Fields,
        now: Duration,
        emit: &mut dyn FnMut(LinkEvent),
    ) {
        while self
            .emitted
            .front()
            .is_some_and(|at| now.saturating_sub(*at) >= Duration::from_secs(1))
        {
            self.emitted.pop_front();
        }
        self.recent
            .retain(|_, at| now.saturating_sub(*at) < Duration::from_secs(2));
        let key = (fields["address"].clone(), fields["raw"].clone());
        if self.recent.contains_key(&key) || self.emitted.len() >= 50 {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        self.recent.insert(key, now);
        self.emitted.push_back(now);
        emit(LinkEvent::Trace {
            stage: "ble_raw".into(),
            fields,
        });
    }
    pub(crate) fn finish(&self, emit: &mut dyn FnMut(LinkEvent)) {
        emit(LinkEvent::Trace {
            stage: "ble_raw_dropped".into(),
            fields: [("count".into(), self.dropped.to_string())].into(),
        });
    }
}

#[cfg(target_os = "linux")]
mod native {
    use super::*;
    use oxp_clink::Result;
    use std::{
        io,
        mem::size_of,
        os::fd::{AsRawFd, FromRawFd, OwnedFd},
    };
    #[repr(C)]
    struct Address {
        family: libc::sa_family_t,
        device: u16,
        channel: u16,
    }
    #[repr(C)]
    struct Filter {
        type_mask: u32,
        event_mask: [u32; 2],
        opcode: u16,
        padding: u16,
    }
    pub(crate) struct RawSocket(OwnedFd);
    impl RawSocket {
        pub(crate) fn open(adapter: &str) -> Result<Self> {
            let device: u16 = adapter
                .strip_prefix("hci")
                .ok_or("invalid_bluetooth_configuration")?
                .parse()?;
            // SAFETY: Linux HCI raw socket constants; no borrowed pointers.
            let fd = unsafe {
                libc::socket(
                    libc::AF_BLUETOOTH,
                    libc::SOCK_RAW | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                    1,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error().into());
            }
            // SAFETY: fd was just created and ownership transfers to this object.
            let socket = Self(unsafe { OwnedFd::from_raw_fd(fd) });
            let address = Address {
                family: libc::AF_BLUETOOTH as _,
                device,
                channel: 0,
            };
            // SAFETY: address is the initialized Linux sockaddr_hci and fd is live.
            if unsafe {
                libc::bind(
                    fd,
                    (&address as *const Address).cast(),
                    size_of::<Address>() as _,
                )
            } < 0
            {
                return Err(io::Error::last_os_error().into());
            }
            let filter = Filter {
                type_mask: 1 << 4,
                event_mask: [0, 1 << (0x3e - 32)],
                opcode: 0,
                padding: 0,
            };
            // SAFETY: filter has Linux hci_filter layout and remains live for this call.
            if unsafe {
                libc::setsockopt(
                    fd,
                    0,
                    2,
                    (&filter as *const Filter).cast(),
                    size_of::<Filter>() as _,
                )
            } < 0
            {
                return Err(io::Error::last_os_error().into());
            }
            Ok(socket)
        }
        pub(crate) fn drain(&self, emit: &mut dyn FnMut(Fields)) -> Result<()> {
            let mut packet = [0; 260];
            // Bound work per scan tick; the kernel receive queue also has a fixed limit.
            for _ in 0..256 {
                // SAFETY: packet is writable for its full length and the socket is live.
                let len = unsafe {
                    libc::recv(
                        self.0.as_raw_fd(),
                        packet.as_mut_ptr().cast(),
                        packet.len(),
                        libc::MSG_TRUNC,
                    )
                };
                if len < 0 {
                    let error = io::Error::last_os_error();
                    match error.kind() {
                        io::ErrorKind::WouldBlock => return Ok(()),
                        io::ErrorKind::Interrupted => continue,
                        _ => return Err(error.into()),
                    }
                }
                if len == 0 {
                    return Err("bluetooth_monitor_closed".into());
                }
                if len as usize <= packet.len() {
                    for fields in reports(&packet[..len as usize]) {
                        emit(fields);
                    }
                }
            }
            Ok(())
        }
    }
}
#[cfg(target_os = "linux")]
pub(crate) use native::RawSocket;

#[cfg(test)]
mod tests {
    use super::*;
    fn legacy(raw: &[u8]) -> Vec<u8> {
        let mut p = vec![4, 0x3e, 0, 2, 1, 0, 1, 6, 5, 4, 3, 2, 1, raw.len() as u8];
        p.extend(raw);
        p.push(206);
        p[2] = (p.len() - 3) as u8;
        p
    }
    #[test]
    fn unrelated_advertisements_keep_exact_payload_name_vendor_and_rssi() {
        let payload = [
            4, 9, b'C', b'a', b'r', 5, 255, 0x25, 0, 0xab, 0xcd, 5, 0x16, 0xfb, 0xfc, 1, 2,
        ];
        let fields = reports(&legacy(&payload)).pop().unwrap();
        assert_eq!(fields["address"], "01:02:03:04:05:06");
        assert_eq!(fields["rssi"], "-50");
        assert_eq!(fields["localName"], "Car");
        assert_eq!(fields["manufacturerData"], r#"{"37":"abcd"}"#);
        assert_eq!(fields["raw"], hex(&payload));
        assert!(fields["serviceData"].contains("0102"));
        assert!(reports(&legacy(&[8, 9, 1])).len() == 1); // Malformed AD is still captured raw.
        let packet = legacy(&payload);
        for end in 0..packet.len() {
            assert!(reports(&packet[..end]).is_empty());
        }
    }
    #[test]
    fn extended_reports_and_multiple_reports_are_bounded_and_preserved() {
        let mut p = vec![4, 0x3e, 26, 13, 1];
        let mut body = vec![0; 24];
        body[2] = 1;
        body[3..9].copy_from_slice(&[6, 5, 4, 3, 2, 1]);
        body[13] = 196;
        body[23] = 2;
        p.extend(body);
        p.extend([1, 0]);
        p[2] = (p.len() - 3) as u8;
        let fields = reports(&p).pop().unwrap();
        assert_eq!(fields["rssi"], "-60");
        assert_eq!(fields["legacy"], "false");
        assert_eq!(fields["raw"], "0100");
        let first = legacy(&[1, 0]);
        let mut multiple = first.clone();
        multiple[4] = 2;
        multiple.extend(&first[5..]);
        multiple[2] = (multiple.len() - 3) as u8;
        assert_eq!(reports(&multiple).len(), 2);
    }
    #[test]
    fn throttle_matches_android_and_duplicate_memory_stays_bounded() {
        let mut capture = Capture::default();
        let mut count = 0;
        for n in 0..1000 {
            let fields = [
                ("address".into(), n.to_string()),
                ("raw".into(), "00".into()),
            ]
            .into();
            capture.record(fields, Duration::ZERO, &mut |_| count += 1);
        }
        assert_eq!(count, 50);
        assert_eq!(capture.recent.len(), 50);
        assert_eq!(capture.dropped, 950);
        let fields = [("address".into(), "0".into()), ("raw".into(), "00".into())].into();
        capture.record(fields, Duration::from_secs(1), &mut |_| count += 1);
        assert_eq!(count, 50);
        let fields = [("address".into(), "0".into()), ("raw".into(), "00".into())].into();
        capture.record(fields, Duration::from_secs(2), &mut |_| count += 1);
        assert_eq!(count, 51);
        assert_eq!(capture.recent.len(), 1);
    }
}
