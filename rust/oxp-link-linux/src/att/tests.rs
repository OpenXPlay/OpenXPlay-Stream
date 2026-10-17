// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use super::*;
use std::collections::VecDeque;
pub(crate) const JSON: &[u8] = br#"{"name":"vehicle","ssid":"WiFi","psk":"12345678","mac":"aa:bb:cc:dd:ee:ff","freq":5180,"port":57209,"type":1001}"#;
pub(crate) struct Fake {
    pub(crate) replies: VecDeque<Vec<u8>>,
    pub(crate) sent: Vec<Vec<u8>>,
    pub(crate) echo: bool,
}
impl Socket for Fake {
    fn set_deadline(&mut self, _: Instant) {}
    fn send(&mut self, b: &[u8]) -> Result<()> {
        self.sent.push(b.to_vec());
        if self.echo && b[0] == 0x16 {
            let mut r = b.to_vec();
            r[0] = 0x17;
            self.replies.push_front(r);
        }
        Ok(())
    }
    fn receive(&mut self) -> Result<Vec<u8>> {
        let reply = self.replies.pop_front().ok_or("fake_closed")?;
        if reply.is_empty() {
            return Err("bluetooth_handoff_timeout".into());
        }
        Ok(reply)
    }
}
fn raw_uuid(s: &str) -> Vec<u8> {
    let s = s.replace('-', "");
    (0..32)
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .rev()
        .collect()
}
pub(crate) fn discovery(mtu: u16) -> VecDeque<Vec<u8>> {
    let service = [
        vec![0x11, 20],
        1u16.to_le_bytes().to_vec(),
        10u16.to_le_bytes().to_vec(),
        raw_uuid(ble::GATT_SERVICE),
    ]
    .concat();
    let chars = [
        vec![9, 21],
        2u16.to_le_bytes().to_vec(),
        vec![8],
        3u16.to_le_bytes().to_vec(),
        raw_uuid(ble::PHONE_CHARACTERISTIC),
        4u16.to_le_bytes().to_vec(),
        vec![0x20],
        5u16.to_le_bytes().to_vec(),
        raw_uuid(ble::HEAD_UNIT_CHARACTERISTIC),
    ]
    .concat();
    VecDeque::from(vec![
        pdu(3, &[mtu], &[]),
        service,
        vec![1, 0x10, 11, 0, 10],
        chars,
        vec![1, 8, 5, 0, 10],
    ])
}
fn fake_session(fake: &mut Fake, mtu: usize) -> Session<'_, Fake> {
    Session {
        socket: fake,
        mtu,
        received: 0,
        notify: 5,
        collector: Collector::default(),
        reply: None,
        prepared: false,
        descriptor_range: 10,
        subscription_value: 2,
        service_changed: 0,
        changed: false,
        written: false,
        diagnostic: None,
    }
}
#[test]
fn att_vectors() {
    assert_eq!(pdu(0x16, &[3, 18], b"phone"), b"\x16\x03\x00\x12\x00phone");
    assert_eq!(
        uuid(&raw_uuid(ble::GATT_SERVICE)).unwrap(),
        ble::GATT_SERVICE
    );
    assert_eq!(
        uuid(&[0, 0x28]).unwrap(),
        "00002800-0000-1000-8000-00805f9b34fb"
    );
    assert!(uuid(&[1]).is_err());
}
#[test]
fn complete_handoff_indication_before_write_response() {
    let mut replies = discovery(512);
    replies.extend([pdu(0x1d, &[5], JSON), vec![0x13]]);
    let mut f = Fake {
        replies,
        sent: Vec::new(),
        echo: false,
    };
    let reply = handoff(
        &mut f,
        b"phone",
        true,
        Instant::now() + Duration::from_secs(15),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(reply.ssid, "WiFi");
    assert_eq!(f.sent[0], vec![2, 0, 2]);
    assert!(f.sent.contains(&vec![0x1e]));
    assert_eq!(f.sent.iter().filter(|b| b[0] == 0x12).count(), 1);
    assert_eq!(
        f.sent.iter().find(|b| b[0] == 0x12).unwrap(),
        &pdu(0x12, &[3], b"phone")
    );
}
#[test]
fn prepared_write_and_fragmented_indications() {
    let mut f = Fake {
        replies: VecDeque::from([vec![0x19]]),
        sent: Vec::new(),
        echo: true,
    };
    let mut s = fake_session(&mut f, 23);
    let payload = vec![42; 50];
    s.write(3, &payload).unwrap();
    assert!(!s.prepared);
    for chunk in JSON.chunks(20) {
        s.notification(&pdu(0x1d, &[5], chunk)).unwrap();
    }
    assert_eq!(s.reply.unwrap().ssid, "WiFi");
    assert_eq!(f.sent[0], pdu(0x16, &[3, 0], &payload[..18]));
    assert_eq!(f.sent[1], pdu(0x16, &[3, 18], &payload[18..36]));
    assert_eq!(f.sent[2], pdu(0x16, &[3, 36], &payload[36..]));
    assert_eq!(f.sent[3], vec![0x18, 1]);
    assert_eq!(
        f.sent.iter().filter(|p| p.as_slice() == [0x1e]).count(),
        JSON.len().div_ceil(20)
    );
}
#[test]
fn indications_confirmed_even_for_other_handles() {
    let mut f = Fake {
        replies: VecDeque::new(),
        sent: Vec::new(),
        echo: false,
    };
    let mut s = fake_session(&mut f, 512);
    s.notification(&pdu(0x1d, &[8], b"unrelated")).unwrap();
    assert!(s.reply.is_none());
    assert_eq!(f.sent, vec![vec![0x1e]]);
}
#[test]
fn reject_bad_echo_mtu_packet_and_duplicate_json() {
    let mut f = Fake {
        replies: VecDeque::from([vec![0x17, 3, 0, 0, 0, 0]]),
        sent: Vec::new(),
        echo: false,
    };
    let mut s = fake_session(&mut f, 23);
    assert!(s.write(3, &[42; 50]).is_err());
    assert!(s.prepared);
    let mut f = Fake {
        replies: VecDeque::from([vec![3, 22, 0]]),
        sent: Vec::new(),
        echo: false,
    };
    assert!(handoff(
        &mut f,
        b"phone",
        true,
        Instant::now() + Duration::from_secs(15),
        &mut |_| {}
    )
    .is_err());
    let mut f = Fake {
        replies: VecDeque::from([vec![0; 24]]),
        sent: Vec::new(),
        echo: false,
    };
    assert!(fake_session(&mut f, 23).receive().is_err());
    let mut f = Fake {
        replies: VecDeque::new(),
        sent: Vec::new(),
        echo: false,
    };
    let mut s = fake_session(&mut f, 512);
    s.notification(&pdu(0x1d, &[5], JSON)).unwrap();
    assert!(s.notification(&pdu(0x1d, &[5], JSON)).is_err());
}
#[test]
fn reject_duplicate_credentials_and_cancel_failed_prepare() {
    let mut f = Fake {
        replies: VecDeque::new(),
        sent: Vec::new(),
        echo: false,
    };
    let mut s = fake_session(&mut f, 512);
    let duplicate = String::from_utf8(JSON.to_vec())
        .unwrap()
        .replace("\"ssid\":\"WiFi\"", "\"ssid\":\"WiFi\",\"ssid\":\"two\"");
    assert!(s
        .notification(&pdu(0x1d, &[5], duplicate.as_bytes()))
        .is_err());
    let mut replies = discovery(64);
    replies.push_back(vec![1, 0x16, 3, 0, 14]);
    let mut f = Fake {
        replies,
        sent: Vec::new(),
        echo: false,
    };
    assert!(handoff(
        &mut f,
        &[42; 128],
        true,
        Instant::now() + Duration::from_secs(15),
        &mut |_| {}
    )
    .is_err());
    assert_eq!(f.sent.last().unwrap(), &vec![0x18, 0]);
}

#[test]
fn json_fragments_split_utf8_and_escaped_strings() {
    let json = String::from_utf8(JSON.to_vec())
        .unwrap()
        .replace("vehicle", "车\\\"{unit}");
    let split = json.as_bytes().iter().position(|b| *b == 0xe8).unwrap() + 1;
    let mut f = Fake {
        replies: VecDeque::new(),
        sent: Vec::new(),
        echo: false,
    };
    let mut s = fake_session(&mut f, 512);
    s.notification(&pdu(0x1d, &[5], &json.as_bytes()[..split]))
        .unwrap();
    assert!(s.reply.is_none());
    s.notification(&pdu(0x1d, &[5], &json.as_bytes()[split..]))
        .unwrap();
    assert!(s.reply.unwrap().name.starts_with('车'));
    let mut c = Collector::default();
    assert!(c.feed(b"{}{}").is_err());
    let mut c = Collector::default();
    assert!(c.feed(&[b'{'; 17]).is_err());
    let mut c = Collector::default();
    assert!(c.feed(&[b' '; 4097]).is_err());
}

// Include the standard GATT service and its Service Changed value at handle 22.
pub(crate) fn discovery_with_service_changed() -> VecDeque<Vec<u8>> {
    let mut replies = discovery(512);
    replies.insert(2, vec![0x11, 6, 20, 0, 25, 0, 1, 0x18]);
    replies.insert(4, vec![9, 7, 21, 0, 0x20, 22, 0, 5, 0x2a]);
    replies.insert(5, vec![1, 8, 22, 0, 10]);
    replies
}
pub(crate) fn cccd_discovery() -> Vec<Vec<u8>> {
    vec![vec![5, 1, 6, 0, 2, 0x29], vec![1, 4, 7, 0, 10]]
}
#[test]
fn cccd_write_precedes_phone_request_for_indicate_and_notify() {
    for (properties, value) in [(0x20, 2), (0x10, 1)] {
        let mut replies = discovery(512);
        replies[3][25] = properties;
        replies.extend(cccd_discovery());
        replies.extend([vec![0x13], vec![0x13], pdu(0x1d, &[5], JSON)]);
        let mut f = Fake {
            replies,
            sent: Vec::new(),
            echo: false,
        };
        handoff(
            &mut f,
            b"phone",
            false,
            Instant::now() + Duration::from_secs(15),
            &mut |_| {},
        )
        .unwrap();
        let writes: Vec<_> = f.sent.iter().filter(|p| p[0] == 0x12).collect();
        assert_eq!(
            writes,
            vec![&pdu(0x12, &[6], &[value, 0]), &pdu(0x12, &[3], b"phone")]
        );
        assert!(f.sent.contains(&pdu(4, &[6, 10], &[])));
    }
}
#[test]
fn service_changed_after_write_rejects_without_waiting_for_write_response() {
    let mut replies = discovery_with_service_changed();
    replies.push_back(pdu(0x1d, &[22, 1, 65535], &[]));
    let mut f = Fake {
        replies,
        sent: Vec::new(),
        echo: false,
    };
    let mut events = Vec::new();
    let error = handoff(
        &mut f,
        b"phone",
        true,
        Instant::now() + Duration::from_secs(15),
        &mut |s| events.push(s.to_owned()),
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "head_unit_rejected_request");
    assert_eq!(f.sent.last().unwrap(), &[0x1e]);
    assert!(events.iter().any(|e| e == "head_unit_rejected_request"));
}
#[test]
fn service_changed_before_write_rediscovers_after_outstanding_response() {
    let mut replies = discovery_with_service_changed();
    replies.extend(cccd_discovery());
    replies.extend([pdu(0x1d, &[22, 1, 65535], &[]), vec![0x13]]);
    replies.extend(discovery_with_service_changed().into_iter().skip(1));
    replies.extend(cccd_discovery());
    replies.extend([vec![0x13], vec![0x13], pdu(0x1d, &[5], JSON)]);
    let mut f = Fake {
        replies,
        sent: Vec::new(),
        echo: false,
    };
    handoff(
        &mut f,
        b"phone",
        false,
        Instant::now() + Duration::from_secs(15),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(
        f.sent.iter().filter(|p| p[0] == 0x10 && p[1] == 1).count(),
        2
    );
    assert_eq!(
        f.sent.iter().filter(|p| p[0] == 0x12 && p[1] == 3).count(),
        1
    );
}
