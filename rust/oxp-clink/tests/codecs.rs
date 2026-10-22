// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use oxp_clink::{
    ble,
    interfaces::*,
    media::RtpPacketizer,
    model::*,
    placeholder::*,
    protocol::{certificate, control, frame::crc16, *},
    uibc::*,
    video_delivery::*,
    wfd::*,
};
use std::{collections::BTreeMap, sync::Arc};
fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}
fn origin() -> Origin {
    Origin {
        output_id: "clink".into(),
        client_id: "hu".into(),
    }
}
fn response(request: &RtspMessage, body: &str) -> RtspMessage {
    RtspMessage {
        line: "RTSP/1.0 200 OK".into(),
        headers: BTreeMap::from([("cseq".into(), request.headers["cseq"].clone())]),
        body: body.into(),
    }
}
#[test]
fn frame_reference_fragmentation_crc_and_rejection() {
    assert_eq!(0, crc16(&[]));
    assert_eq!(0xbb3d, crc16(b"123456789"));
    let bytes = hex("0000001701020304650000010b03000100006c56080138");
    let frame = Frame {
        sequence: 0x01020304,
        unix_seconds: 0x65000001,
        source: 1,
        format: 1,
        kind: 1,
        category: 3,
        method: 1,
        body: hex("080138"),
    };
    assert_eq!(bytes, frame.encode().unwrap());
    assert_eq!(frame, Frame::decode(&bytes).unwrap());
    for split in 0..=bytes.len() {
        let mut r = FrameReader::default();
        let mut frames = r.feed(&bytes[..split]).unwrap();
        frames.extend(r.feed(&[&bytes[split..], &bytes].concat()).unwrap());
        assert_eq!(2, frames.len());
        assert_eq!(0, r.pending_bytes());
    }
    let mut corrupt = bytes.clone();
    corrupt[4] = 0;
    assert!(Frame::decode(&corrupt).is_err());
    assert!(FrameReader::default().feed(&hex("ffffffff")).is_err());
    for i in [12, 16] {
        let mut bad = bytes.clone();
        bad[i] |= 128;
        let crc = crc16(&bad[..18]);
        bad[18..20].copy_from_slice(&crc.to_be_bytes());
        assert!(Frame::decode(&bad).is_err());
    }
}
#[test]
fn fields_vectors_max_fixed_duplicates_and_overflow() {
    let fields = Fields(BTreeMap::from([
        (3, FieldValue::Bytes(b"abc".to_vec())),
        (1, FieldValue::Varint(150)),
        (2, FieldValue::Varint(0)),
    ]));
    assert_eq!(hex("08960110001a03616263"), fields.encode().unwrap());
    assert_eq!(fields, Fields::decode(&fields.encode().unwrap()).unwrap());
    let max = Fields(BTreeMap::from([(1, FieldValue::Varint(u64::MAX))]));
    assert_eq!(hex("08ffffffffffffffffff01"), max.encode().unwrap());
    assert_eq!(
        u64::MAX,
        Fields::decode(&max.encode().unwrap())
            .unwrap()
            .number(1)
            .unwrap()
    );
    let fixed = hex("0900010203040506071500010203");
    assert_eq!(fixed, Fields::decode(&fixed).unwrap().encode().unwrap());
    for invalid in ["00", "0880", "08010802", "1203ff", "08ffffffffffffffffff02"] {
        assert!(Fields::decode(&hex(invalid)).is_err());
    }
}
#[test]
fn gcm_nist_replay_tamper_fragments_and_variable_iv() {
    let record=hex("0000000c000000000000000000000000000000200388dace60b6a392f328c2b971b2fe78ab6e47d42cec13bdf53a67b21257bddf");
    assert_eq!(
        record,
        SecureRecord::new(&[0; 16])
            .unwrap()
            .seal_with_iv(&[0; 16], &[0; 12])
            .unwrap()
    );
    let mut inbound = SecureRecord::new(&[0; 16]).unwrap();
    assert_eq!(vec![0; 16], inbound.open(&record).unwrap());
    assert!(inbound.open(&record).is_err());
    let mut bad = record.clone();
    *bad.last_mut().unwrap() ^= 1;
    assert!(SecureRecord::new(&[0; 16]).unwrap().open(&bad).is_err());
    for split in 0..=record.len() {
        let mut r = SecureRecord::new(&[0; 16]).unwrap();
        let mut out = r.feed(&record[..split]).unwrap();
        out.extend(r.feed(&record[split..]).unwrap());
        assert_eq!(vec![vec![0; 16]], out);
        assert_eq!(0, r.pending_bytes());
    }
    assert!(SecureRecord::new(&[0; 16])
        .unwrap()
        .open(&[record.clone(), record].concat())
        .is_err());
    assert!(SecureRecord::new(&[0; 16])
        .unwrap()
        .feed(&hex("0000000b"))
        .is_err());
    // Java's accepted non-96-bit nonce path must use GCM's GHASH IV derivation.
    use aes_gcm::{
        aead::{consts::U16, Aead, KeyInit},
        aes::Aes128,
        AesGcm, Nonce,
    };
    let iv = [7; 16];
    let cipher = AesGcm::<Aes128, U16>::new_from_slice(&[0; 16]).unwrap();
    let encrypted = cipher
        .encrypt(Nonce::<U16>::from_slice(&iv), b"control".as_slice())
        .unwrap();
    let mut r = 16u32.to_be_bytes().to_vec();
    r.extend_from_slice(&iv);
    r.extend_from_slice(&(encrypted.len() as u32).to_be_bytes());
    r.extend(encrypted);
    assert_eq!(
        b"control",
        SecureRecord::new(&[0; 16])
            .unwrap()
            .open(&r)
            .unwrap()
            .as_slice()
    );
}
#[test]
fn ble_reference_vectors_and_validation() {
    let ad = ble::advertisement(
        &hex("010200020000000000000100010000"),
        "示例车机".as_bytes(),
    )
    .unwrap();
    assert_eq!("020000000000", ad.car_id);
    assert_eq!("00010001", ad.model_id);
    assert_eq!("0000", ad.vendor_data);
    assert_eq!("示例车机", ad.name);
    assert_eq!(
        "000102030405",
        ble::advertisement(&hex("010900000102030405060708091234ff"), b"Car\0\0")
            .unwrap()
            .car_id
    );
    let request = ble::phone_request(&ble::PhoneRequest {
        id: "client-id".into(),
        name: "Pi source".into(),
        model: "Linux".into(),
        band: 1,
        mac: "aa:bb:cc:dd:ee:ff".into(),
        kind: 1001,
    })
    .unwrap();
    assert_eq!(
        r#"{"id":"client-id","name":"Pi source","model":"Linux","band":1,"mac":"aa:bb:cc:dd:ee:ff","type":1001}"#,
        std::str::from_utf8(&request).unwrap()
    );
    let json = r#"{"name":"car short name","ssid":"DIRECT-vs-XXXXXX","psk":"temporary-wifi-secret","mac":"02:00:00:00:00:00","freq":5200,"port":0,"type":1001}"#;
    let reply = ble::head_unit_reply(json.as_bytes()).unwrap();
    assert_eq!("DIRECT-vs-XXXXXX", reply.ssid);
    assert_eq!(5200, reply.freq);
    assert_eq!(0, reply.port);
    assert!(ble::head_unit_reply(json.replace("5200", "0").as_bytes()).is_ok());
    assert!(ble::head_unit_reply(json.replace("5200", "\"5200\"").as_bytes()).is_err());
    assert!(
        ble::head_unit_reply(json.replace('{', "{\"ssid\":\"duplicate\",").as_bytes()).is_err()
    );
    assert!(ble::head_unit_reply(&[0xc0, 0x80]).is_err());
    assert!(ble::advertisement(&[0; 14], &[]).is_err());
}
#[test]
fn rtsp_roundtrip_fragments_pipeline_feedback_and_duplicates() {
    let fixture = include_str!("../../../kotlin/clink/src/test/resources/head-unit-wfd.txt");
    let request = RtspMessage {
        line: "GET_PARAMETER * RTSP/1.0".into(),
        headers: BTreeMap::from([("cseq".into(), "7".into())]),
        body: fixture.into(),
    };
    let reply = response(&request, "");
    let bytes = [
        request.encode().unwrap(),
        vec![b'$', 0, 0, 2, 1, 2],
        reply.encode().unwrap(),
    ]
    .concat();
    let mut reader = RtspReader::default();
    let mut messages = Vec::new();
    for b in bytes {
        messages.extend(reader.feed(&[b]).unwrap());
    }
    assert_eq!(vec![request, reply], messages);
    assert!(RtspReader::default()
        .feed(b"OPTIONS * RTSP/1.0\r\nCSeq: 1\r\ncseq: 2\r\n\r\n")
        .is_err());
}
#[test]
fn real_wfd_capabilities_setup_play_idr_and_session_validation() {
    let ports = ClinkPorts {
        rtsp: 17236,
        uibc: 14321,
        rtp: 15551,
        ..Default::default()
    };
    let mut s = WfdSource::new("127.0.0.1".into(), ports);
    let d = DisplayInfo {
        width_px: 1072,
        height_px: 2320,
        density_dpi: 320,
        ..Default::default()
    };
    let start = s.start();
    let get = s.handle(&response(&start, ""), &d).unwrap().remove(0);
    let m4 = s
        .handle(
            &response(
                &get,
                include_str!("../../../kotlin/clink/src/test/resources/head-unit-wfd.txt"),
            ),
            &d,
        )
        .unwrap()
        .remove(0);
    assert_eq!("avc1.42E01F", s.decoders[0].codec_string);
    assert_eq!(WfdMode::default(), s.mode);
    for text in [
        "wfd_video_formats: 28 00 01 01 00000020",
        "port=14321;uibc_encrypted=true",
        "display=1280:720;dpi=320;fps=30",
        "rtsp://127.0.0.1:17236/wfd1.0/streamid=0 none",
        "wfd_audio_codecs: AAC 00000001 00",
    ] {
        assert!(m4.body.contains(text));
    }
    let trigger = s.handle(&response(&m4, ""), &d).unwrap().remove(0);
    assert_eq!("wfd_trigger_method: SETUP\r\n", trigger.body);
    s.handle(&response(&trigger, ""), &d).unwrap();
    let setup = s
        .handle(
            &RtspMessage {
                line: "SETUP * RTSP/1.0".into(),
                headers: BTreeMap::from([
                    ("cseq".into(), "20".into()),
                    (
                        "transport".into(),
                        "RTP/AVP/TCP;unicast;client_port=15550".into(),
                    ),
                ]),
                body: String::new(),
            },
            &d,
        )
        .unwrap()
        .remove(0);
    assert_eq!("RTSP/1.0 200 OK", setup.line);
    assert_eq!(15550, s.rtp_port);
    assert!(setup.headers["transport"].contains("server_port=15551-15552"));
    let session = setup.headers["session"].clone();
    let mut play = RtspMessage {
        line: "PLAY * RTSP/1.0".into(),
        headers: BTreeMap::from([("cseq".into(), "21".into()), ("session".into(), session)]),
        body: String::new(),
    };
    s.handle(&play, &d).unwrap();
    assert!(s.playing);
    assert!(s.take_keyframe());
    assert!(!s.take_keyframe());
    s.handle(
        &RtspMessage {
            line: "SET_PARAMETER * RTSP/1.0".into(),
            headers: BTreeMap::from([("cseq".into(), "22".into())]),
            body: "wfd_idr_request\r\n".into(),
        },
        &d,
    )
    .unwrap();
    assert!(s.take_keyframe());
    play.headers.insert("session".into(), "wrong".into());
    assert_eq!(
        "RTSP/1.0 454 Session Not Found",
        s.handle(&play, &d).unwrap()[0].line
    );
}
#[test]
fn rtp_seven_ts_sequence_wrap_and_rfc4571() {
    let ts: Vec<_> = (0..15)
        .map(|n| {
            let mut p = vec![n; 188];
            p[0] = 0x47;
            p
        })
        .collect();
    let p = RtpPacketizer::new(0x434c4144, 65535)
        .packetize(&ts, 1_000_000)
        .unwrap();
    assert_eq!(
        vec![1328, 1328, 200],
        p.iter().map(Vec::len).collect::<Vec<_>>()
    );
    for (i, b) in p.iter().enumerate() {
        assert_eq!(&[128, 33], &b[..2]);
        assert_eq!(
            65535u16.wrapping_add(i as u16),
            u16::from_be_bytes(b[2..4].try_into().unwrap())
        );
        assert_eq!(90000, u32::from_be_bytes(b[4..8].try_into().unwrap()));
        assert_eq!(0x434c4144, u32::from_be_bytes(b[8..12].try_into().unwrap()));
        let framed = RtpPacketizer::frame(b).unwrap();
        assert_eq!(
            b.len(),
            u16::from_be_bytes(framed[..2].try_into().unwrap()) as usize
        );
        assert_eq!(b, &framed[2..]);
    }
    assert_eq!(ts[..7].concat(), p[0][12..]);
}
#[test]
fn touch_source_pixels_slots_pointer_order_up_cancel_and_keys() {
    let mut d = UibcDecoder::new(origin());
    let contacts = |e: Option<ControlEvent>| {
        if let Some(ControlEvent::Touch { contacts, .. }) = e {
            contacts
        } else {
            panic!("not touch")
        }
    };
    let down=d.decode(br#"{"type":1,"action":0,"count":1,"width":1072,"height":2320,"trackID0":100,"x0":640,"y0":360}"#,(1280,720)).unwrap();
    assert_eq!(
        vec![TouchContact::new(0, 0.5, 0.5, true).unwrap()],
        contacts(down)
    );
    let second=br#"{"type":1,"action":261,"count":2,"trackID0":100,"x0":640,"y0":360,"trackID1":200,"x1":1280,"y1":720}"#;
    let c = contacts(d.decode(second, (1280, 720)).unwrap());
    assert_eq!(vec![0, 1], c.iter().map(|c| c.id).collect::<Vec<_>>());
    let reversed=br#"{"type":1,"action":2,"count":2,"trackID0":200,"x0":640,"y0":720,"trackID1":100,"x1":640,"y1":360}"#;
    let c = contacts(d.decode(reversed, (1280, 720)).unwrap());
    assert_eq!(0.5, c[0].y);
    assert_eq!(1.0, c[1].y);
    let up = std::str::from_utf8(second).unwrap().replace("261", "262");
    let c = contacts(d.decode(up.as_bytes(), (1280, 720)).unwrap());
    assert_eq!(
        vec![true, false],
        c.iter().map(|c| c.pressed).collect::<Vec<_>>()
    );
    assert!(contacts(
        d.decode(br#"{"type":1,"action":3,"count":0}"#, (1280, 720))
            .unwrap()
    )
    .is_empty());
    for (code, k) in [
        (3, KeyCode::Home),
        (4, KeyCode::Back),
        (23, KeyCode::Select),
        (66, KeyCode::Select),
        (19, KeyCode::KnobUp),
        (20, KeyCode::KnobDown),
        (21, KeyCode::KnobLeft),
        (22, KeyCode::KnobRight),
        (85, KeyCode::PlayPause),
        (87, KeyCode::Next),
        (88, KeyCode::Previous),
        (231, KeyCode::Voice),
    ] {
        assert_eq!(
            Some(ControlEvent::Key {
                code: k,
                down: true,
                origin: origin()
            }),
            key(code, 0, origin()).unwrap()
        );
        assert_eq!(
            Some(ControlEvent::Key {
                code: k,
                down: false,
                origin: origin()
            }),
            key(code, 1, origin()).unwrap()
        );
    }
    assert!(key(999, 0, origin()).unwrap().is_none());
    assert!(key(3, 2, origin()).unwrap().is_none());
    assert!(d
        .decode(br#"{"type":2,"type":1,"keycode":3}"#, (1280, 720))
        .is_err());
    assert_eq!(
        vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
        KeyCode::all()
            .iter()
            .map(|k| k.wire_code())
            .collect::<Vec<_>>()
    );
}
#[test]
fn encrypted_uibc_fragmentation() {
    let key = [7; 16];
    let record = SecureRecord::new(&key)
        .unwrap()
        .seal(br#"{"type":2,"keycode":4,"action":1}"#)
        .unwrap();
    let mut reader = SecureRecord::new(&key).unwrap();
    let mut d = UibcDecoder::new(origin());
    let mut events = Vec::new();
    for b in record {
        for plain in reader.feed(&[b]).unwrap() {
            events.push(d.decode(&plain, (1280, 720)).unwrap());
        }
    }
    assert_eq!(
        vec![Some(ControlEvent::Key {
            code: KeyCode::Back,
            down: false,
            origin: origin()
        })],
        events
    );
}
#[test]
fn cert_signed_byte_envelope_and_control_projection_mode() {
    use base64::Engine;
    let metadata=br#"{"short_name":"car","product_name":"model","vendor_name":"vendor","vendor_id":"v1","product_id":"p1"}"#;
    let envelope = serde_json::json!({"content":metadata.iter().map(|b|*b as i8).collect::<Vec<_>>(),"signature":vec![0;256],"version":1,"signAlgorithm":"SHA256withRSA","cryptoConfigIndex":"v1"});
    let body = |e: &serde_json::Value| {
        Fields(BTreeMap::from([(
            1,
            FieldValue::Bytes(
                base64::engine::general_purpose::STANDARD
                    .encode(e.to_string())
                    .into_bytes(),
            ),
        )]))
        .encode()
        .unwrap()
    };
    assert_eq!(
        certificate::check(&body(&envelope)).unwrap(),
        BTreeMap::from([
            ("short_name".into(), "car".into()),
            ("product_name".into(), "model".into()),
            ("vendor_name".into(), "vendor".into()),
            ("vendor_id".into(), "v1".into()),
            ("product_id".into(), "p1".into()),
        ])
    );
    let mut bad = envelope.clone();
    bad["version"] = 2.into();
    assert!(certificate::check(&body(&bad)).is_err());
    bad = envelope;
    bad["signature"][0] = 255.into();
    assert!(certificate::check(&body(&bad)).is_err());
    let display = DisplayInfo {
        width_px: 1072,
        height_px: 2320,
        density_dpi: 320,
        ..control::head_unit_fallback_display()
    };
    let body = control::display_body(&display).unwrap();
    let fields = Fields::decode(&body).unwrap();
    for (id, n) in [
        (2, 1072),
        (3, 2320),
        (4, 320),
        (5, 1072),
        (6, 2320),
        (7, 30),
        (10, 0),
        (14, 1),
    ] {
        assert_eq!(n, fields.number(id).unwrap());
    }
    assert_eq!(
        display,
        control::display(&body, &control::head_unit_fallback_display()).unwrap()
    );
    assert!(control::display(
        &Fields(BTreeMap::from([(10, FieldValue::Varint(1))]))
            .encode()
            .unwrap(),
        &display
    )
    .is_err());
    assert_eq!(
        2,
        Fields::decode(&control::server_response(0).unwrap())
            .unwrap()
            .number(1)
            .unwrap()
    );
}
#[test]
fn placeholder_access_units_use_aud_only_as_boundaries() {
    let p = BundledPlaceholderVideo::new().unwrap();
    assert_eq!(p.frames().len(), 30);
    for (i, frame) in p.frames().iter().enumerate() {
        let units = oxp_media::nal::units(frame, oxp_media::nal::Format::Avcc).unwrap();
        assert!(!units.is_empty());
        assert!(units.iter().all(|n| [1, 5, 6].contains(&(n[0] & 31))));
        assert_eq!(
            i == 0,
            oxp_media::nal::is_key_frame(frame, VideoCodec::H264, oxp_media::nal::Format::Avcc)
                .unwrap()
        );
    }
}

#[test]
fn placeholder_gate_and_queue_overflow_retains_shared_keyframe() {
    let p = BundledPlaceholderVideo::new().unwrap();
    assert_eq!(
        (1280, 720, 30, 30),
        (
            p.format().width,
            p.format().height,
            p.fps(),
            p.frames().len()
        )
    );
    let avc = oxp_media::AvcConfig::parse(&p.format().config).unwrap();
    assert_eq!((66, 31), (avc.profile, avc.level));
    let mut gate = VideoSwitch::default();
    assert!(!gate.accept(true));
    gate.play(true);
    assert!(gate.placeholder());
    assert!(!gate.accept(false));
    assert!(gate.accept(true));
    assert!(!gate.placeholder());
    assert!(gate.accept(false));
    gate.reset();
    assert!(gate.placeholder());
    assert!(!gate.accept(false));
    gate.play(false);
    assert!(!gate.placeholder());
    let f = |pts_us, key_frame, n| VideoFrame {
        screen: 0,
        session: 1,
        data: Arc::from(vec![0; n]),
        pts_us,
        key_frame,
        received_at_nanos: 0,
    };
    let mut batched = VideoQueue::default();
    assert!(!batched.offer(f(0, true, 4)));
    assert!(!batched.offer(f(100_001, false, 4)));
    assert!(!batched.offer(f(600_000, false, 4)));
    assert_eq!(batched.dropped, 0);
    assert!(batched.offer(f(600_001, false, 4)));
    assert!(batched.take_gap());
    assert!(batched.poll().is_none());
    assert_eq!(batched.dropped, 4);
    let mut q = VideoQueue::new(12, 100, 120).unwrap();
    q.offer(f(0, false, 4));
    assert!(q.poll().is_none());
    q.offer(f(0, true, 4));
    q.offer(f(20, false, 4));
    q.offer(f(40, false, 4));
    assert!(q.offer(f(60, false, 4)));
    assert!(q.take_gap());
    assert!(q.poll().is_none());
    q.offer(f(80, false, 4));
    assert!(q.poll().is_none());
    let k = f(100, true, 4);
    let data = k.data.clone();
    q.offer(k);
    assert!(Arc::ptr_eq(&data, &q.poll().unwrap().data));
    q.offer(f(110, true, 4));
    assert!(q.offer(f(250, true, 4)));
    assert!(q.poll().unwrap().key_frame);
    assert!(q.offer(f(300, true, 20)));
    assert!(q.poll().is_none());
    assert!(q.dropped >= 7);
}

#[test]
fn aac_negotiation_requires_lc_bit_and_complete_entry() {
    let body = include_str!("../../../kotlin/clink/src/test/resources/head-unit-wfd.txt");
    let lines = body
        .lines()
        .filter(|l| !l.starts_with("wfd_audio_codecs:"))
        .collect::<Vec<_>>()
        .join("\r\n");
    for (entry, enabled) in [
        ("none", false),
        ("AAC 00000002 00", false),
        ("AAC 00000001", false),
        ("AAC 00000001 00 extra", false),
        ("LPCM 00000001 00, AAC 00000001 00", true),
        ("AAC z 00", false),
    ] {
        let mut s = WfdSource::new("127.0.0.1".into(), ClinkPorts::default());
        let d = DisplayInfo::default();
        let start = s.start();
        let get = s.handle(&response(&start, ""), &d).unwrap().remove(0);
        let m4 = s
            .handle(
                &response(&get, &format!("{lines}\r\nwfd_audio_codecs: {entry}\r\n")),
                &d,
            )
            .unwrap()
            .remove(0);
        assert_eq!(s.audio, enabled, "{entry}");
        assert!(m4.body.contains(if enabled {
            "wfd_audio_codecs: AAC 00000001 00"
        } else {
            "wfd_audio_codecs: none"
        }));
    }
}
