// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-22

use crate::protocol::{FieldValue, Fields};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        OnceLock,
    },
    time::Instant,
};

/// A sink must return immediately, never block and never fail.
pub trait ClinkTrace: Send + Sync {
    fn record(&self, event: ClinkTraceEvent);
}
/// Receives the RTP header and TS payload, without the TCP length prefix.
/// Implementations must return immediately.
pub trait ClinkMediaTap: Send + Sync {
    fn rtp(&self, packet: &[u8]);
}
pub fn now_nanos() -> u64 {
    static ANCHOR: OnceLock<Instant> = OnceLock::new();
    ANCHOR
        .get_or_init(Instant::now)
        .elapsed()
        .as_nanos()
        .min(u64::MAX as u128) as u64
}
pub fn next_session() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed).wrapping_add(1)
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClinkTraceField {
    Number(u64),
    Bytes { length: usize, preview: String },
}
pub fn decode_fields(
    service: &str,
    body: &[u8],
    decrypted: bool,
) -> BTreeMap<u32, ClinkTraceField> {
    if !decrypted || matches!(service, "auth" | "cert") {
        return BTreeMap::new();
    }
    let Ok(fields) = Fields::decode(body) else {
        return BTreeMap::new();
    };
    fields
        .0
        .into_iter()
        .map(|(id, value)| {
            let value = match value {
                FieldValue::Varint(n) => ClinkTraceField::Number(n),
                value => {
                    let bytes: Vec<u8> = match value {
                        FieldValue::Bytes(b) => b,
                        FieldValue::Fixed32(b) => b.to_vec(),
                        FieldValue::Fixed64(b) => b.to_vec(),
                        FieldValue::Varint(_) => unreachable!(),
                    };
                    let cut = &bytes[..bytes.len().min(64)];
                    let preview = if cut.iter().all(|b| (32..=126).contains(b)) {
                        cut.iter().map(|b| *b as char).collect()
                    } else {
                        cut.iter().map(|b| format!("{b:02x}")).collect()
                    };
                    ClinkTraceField::Bytes {
                        length: bytes.len(),
                        preview,
                    }
                }
            };
            (id, value)
        })
        .collect()
}
#[derive(Clone, Debug, PartialEq)]
pub enum ClinkTraceEvent {
    Rtsp {
        at_nanos: u64,
        session: u64,
        direction: String,
        start_line: String,
        headers: BTreeMap<String, String>,
        body: String,
    },
    Frame {
        at_nanos: u64,
        session: u64,
        direction: String,
        port: u16,
        service: String,
        category: u8,
        method: u16,
        kind: u8,
        sequence: u64,
        body_size: usize,
        fields: BTreeMap<u32, ClinkTraceField>,
    },
    Socket {
        at_nanos: u64,
        session: u64,
        service: String,
        event: String,
        remote_address: Option<String>,
        port: u16,
        error_class: Option<String>,
    },
    Negotiation {
        at_nanos: u64,
        session: u64,
        mode: String,
        decoders: Vec<String>,
        audio: bool,
        rtp_port: u16,
        display: String,
    },
    Media {
        at_nanos: u64,
        session: u64,
        live_frames: u64,
        placeholder_frames: u64,
        dropped_frames: u64,
        keyframes: u64,
        video_bytes: u64,
        rtp_packets: u64,
        rtp_bytes: u64,
        send_stalls: u64,
        audio_frames: u64,
        queue_overflows: u64,
    },
    Video {
        at_nanos: u64,
        session: u64,
        event: String,
        source: String,
        codec: Option<String>,
        width: Option<u32>,
        height: Option<u32>,
        sps: Vec<String>,
        pps: Vec<String>,
    },
    Input {
        at_nanos: u64,
        session: u64,
        kind: String,
        coordinates: Vec<(f64, f64)>,
        raw_coordinates: Vec<(i64, i64)>,
        stream_size: Option<(u32, u32)>,
        view_size: Option<(u32, u32)>,
    },
    Error {
        at_nanos: u64,
        session: u64,
        code: String,
        exception_class: String,
        location: String,
    },
    Link {
        at_nanos: u64,
        session: u64,
        stage: String,
        fields: BTreeMap<String, String>,
    },
}
impl ClinkTraceEvent {
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Rtsp { .. } => "Rtsp",
            Self::Frame { .. } => "Frame",
            Self::Socket { .. } => "Socket",
            Self::Negotiation { .. } => "Negotiation",
            Self::Media { .. } => "Media",
            Self::Video { .. } => "Video",
            Self::Input { .. } => "Input",
            Self::Error { .. } => "Error",
            Self::Link { .. } => "Link",
        }
    }
    pub fn to_json(&self) -> Value {
        match self {
            Self::Rtsp {
                at_nanos,
                session,
                direction,
                start_line,
                headers,
                body,
            } => {
                json!({
                    "type":self.type_name(),
                    "atNanos":at_nanos,
                    "session":session,
                    "direction":direction,
                    "startLine":start_line,
                    "headers":headers,
                    "body":body
                })
            }
            Self::Frame {
                at_nanos,
                session,
                direction,
                port,
                service,
                category,
                method,
                kind,
                sequence,
                body_size,
                fields,
            } => {
                let fields: BTreeMap<String, Value> = if matches!(service.as_str(), "auth" | "cert")
                {
                    BTreeMap::new()
                } else {
                    fields
                        .iter()
                        .map(|(id, f)| {
                            (
                                id.to_string(),
                                match f {
                                    ClinkTraceField::Number(n) => json!(n),
                                    ClinkTraceField::Bytes { length, preview } => {
                                        json!({"length":length, "preview":preview})
                                    }
                                },
                            )
                        })
                        .collect()
                };
                json!({
                    "type":self.type_name(),
                    "atNanos":at_nanos,
                    "session":session,
                    "direction":direction,
                    "port":port,
                    "service":service,
                    "category":category,
                    "method":method,
                    "kind":kind,
                    "sequence":sequence,
                    "bodySize":body_size,
                    "fields":fields
                })
            }
            Self::Socket {
                at_nanos,
                session,
                service,
                event,
                remote_address,
                port,
                error_class,
            } => {
                json!({
                    "type":self.type_name(),
                    "atNanos":at_nanos,
                    "session":session,
                    "service":service,
                    "event":event,
                    "remoteAddress":remote_address,
                    "port":port,
                    "errorClass":error_class
                })
            }
            Self::Negotiation {
                at_nanos,
                session,
                mode,
                decoders,
                audio,
                rtp_port,
                display,
            } => {
                json!({
                    "type":self.type_name(),
                    "atNanos":at_nanos,
                    "session":session,
                    "mode":mode,
                    "decoders":decoders,
                    "audio":audio,
                    "rtpPort":rtp_port,
                    "display":display
                })
            }
            Self::Media {
                at_nanos,
                session,
                live_frames,
                placeholder_frames,
                dropped_frames,
                keyframes,
                video_bytes,
                rtp_packets,
                rtp_bytes,
                send_stalls,
                audio_frames,
                queue_overflows,
            } => {
                json!({
                    "type":self.type_name(),
                    "atNanos":at_nanos,
                    "session":session,
                    "liveFrames":live_frames,
                    "placeholderFrames":placeholder_frames,
                    "droppedFrames":dropped_frames,
                    "keyframes":keyframes,
                    "videoBytes":video_bytes,
                    "rtpPackets":rtp_packets,
                    "rtpBytes":rtp_bytes,
                    "sendStalls":send_stalls,
                    "audioFrames":audio_frames,
                    "queueOverflows":queue_overflows
                })
            }
            Self::Video {
                at_nanos,
                session,
                event,
                source,
                codec,
                width,
                height,
                sps,
                pps,
            } => {
                json!({
                    "type":self.type_name(),
                    "atNanos":at_nanos,
                    "session":session,
                    "event":event,
                    "source":source,
                    "codec":codec,
                    "width":width,
                    "height":height,
                    "sps":sps,
                    "pps":pps
                })
            }
            Self::Input {
                at_nanos,
                session,
                kind,
                coordinates,
                raw_coordinates,
                stream_size,
                view_size,
            } => {
                json!({
                    "type":self.type_name(),
                    "atNanos":at_nanos,
                    "session":session,
                    "kind":kind,
                    "coordinates":coordinates,
                    "rawCoordinates":raw_coordinates,
                    "streamSize":stream_size,
                    "viewSize":view_size
                })
            }
            Self::Error {
                at_nanos,
                session,
                code,
                exception_class,
                location,
            } => {
                json!({
                    "type":self.type_name(),
                    "atNanos":at_nanos,
                    "session":session,
                    "code":code,
                    "exceptionClass":exception_class,
                    "location":location
                })
            }
            Self::Link {
                at_nanos,
                session,
                stage,
                fields,
            } => {
                json!({
                    "type":self.type_name(),
                    "atNanos":at_nanos,
                    "session":session,
                    "stage":stage,
                    "fields":fields
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn golden_android_events() {
        let cases = [
            (
                ClinkTraceEvent::Rtsp {
                    at_nanos: 1,
                    session: 2,
                    direction: "RX".into(),
                    start_line: "OPTIONS * RTSP/1.0".into(),
                    headers: BTreeMap::new(),
                    body: "".into(),
                },
                json!({
                    "type":"Rtsp",
                    "atNanos":1,
                    "session":2,
                    "direction":"RX",
                    "startLine":"OPTIONS * RTSP/1.0",
                    "headers":{},
                    "body":""
                }),
            ),
            (
                ClinkTraceEvent::Frame {
                    at_nanos: 12,
                    session: 3,
                    direction: "RX".into(),
                    port: 123,
                    service: "control".into(),
                    category: 1,
                    method: 2,
                    kind: 0,
                    sequence: 9,
                    body_size: 8,
                    fields: BTreeMap::from([
                        (1, ClinkTraceField::Number(42)),
                        (
                            2,
                            ClinkTraceField::Bytes {
                                length: 100,
                                preview: "preview".into(),
                            },
                        ),
                    ]),
                },
                json!({
                    "type":"Frame",
                    "atNanos":12,
                    "session":3,
                    "direction":"RX",
                    "port":123,
                    "service":"control",
                    "category":1,
                    "method":2,
                    "kind":0,
                    "sequence":9,
                    "bodySize":8,
                    "fields":{"1":42,"2":{"length":100,"preview":"preview"}}
                }),
            ),
            (
                ClinkTraceEvent::Socket {
                    at_nanos: 1,
                    session: 2,
                    service: "rtsp".into(),
                    event: "LISTEN".into(),
                    remote_address: None,
                    port: 7236,
                    error_class: None,
                },
                json!({
                    "type":"Socket",
                    "atNanos":1,
                    "session":2,
                    "service":"rtsp",
                    "event":"LISTEN",
                    "remoteAddress":null,
                    "port":7236,
                    "errorClass":null
                }),
            ),
            (
                ClinkTraceEvent::Negotiation {
                    at_nanos: 1,
                    session: 2,
                    mode: "mode".into(),
                    decoders: vec!["avc1.42E01F".into()],
                    audio: true,
                    rtp_port: 15550,
                    display: "display".into(),
                },
                json!({
                    "type":"Negotiation",
                    "atNanos":1,
                    "session":2,
                    "mode":"mode",
                    "decoders":["avc1.42E01F"],
                    "audio":true,
                    "rtpPort":15550,
                    "display":"display"
                }),
            ),
            (
                ClinkTraceEvent::Media {
                    at_nanos: 1,
                    session: 2,
                    live_frames: 3,
                    placeholder_frames: 4,
                    dropped_frames: 5,
                    keyframes: 6,
                    video_bytes: 7,
                    rtp_packets: 8,
                    rtp_bytes: 9,
                    send_stalls: 10,
                    audio_frames: 11,
                    queue_overflows: 12,
                },
                json!({
                    "type":"Media",
                    "atNanos":1,
                    "session":2,
                    "liveFrames":3,
                    "placeholderFrames":4,
                    "droppedFrames":5,
                    "keyframes":6,
                    "videoBytes":7,
                    "rtpPackets":8,
                    "rtpBytes":9,
                    "sendStalls":10,
                    "audioFrames":11,
                    "queueOverflows":12
                }),
            ),
            (
                ClinkTraceEvent::Video {
                    at_nanos: 1,
                    session: 2,
                    event: "format".into(),
                    source: "".into(),
                    codec: None,
                    width: None,
                    height: None,
                    sps: vec!["abcd".into()],
                    pps: vec![],
                },
                json!({
                    "type":"Video",
                    "atNanos":1,
                    "session":2,
                    "event":"format",
                    "source":"",
                    "codec":null,
                    "width":null,
                    "height":null,
                    "sps":["abcd"],
                    "pps":[]
                }),
            ),
            (
                ClinkTraceEvent::Input {
                    at_nanos: 1,
                    session: 2,
                    kind: "touch".into(),
                    coordinates: vec![(0.5, 0.25)],
                    raw_coordinates: vec![(640, 180)],
                    stream_size: Some((1156, 764)),
                    view_size: Some((1532, 1013)),
                },
                json!({
                    "type":"Input",
                    "atNanos":1,
                    "session":2,
                    "kind":"touch",
                    "coordinates":[[0.5,0.25]],
                    "rawCoordinates":[[640,180]],
                    "streamSize":[1156,764],
                    "viewSize":[1532,1013]
                }),
            ),
            (
                ClinkTraceEvent::Error {
                    at_nanos: 1,
                    session: 2,
                    code: "session_failed".into(),
                    exception_class: "auth_closed".into(),
                    location: "ClinkSession.run".into(),
                },
                json!({
                    "type":"Error",
                    "atNanos":1,
                    "session":2,
                    "code":"session_failed",
                    "exceptionClass":"auth_closed",
                    "location":"ClinkSession.run"
                }),
            ),
            (
                ClinkTraceEvent::Link {
                    at_nanos: u64::MAX,
                    session: 9007199254740993,
                    stage: "stage".into(),
                    fields: BTreeMap::new(),
                },
                json!({
                    "type":"Link",
                    "atNanos":u64::MAX,
                    "session":9007199254740993u64,
                    "stage":"stage",
                    "fields":{}
                }),
            ),
        ];
        for (event, expected) in cases {
            assert_eq!(event.to_json(), expected);
            assert_eq!(event.type_name(), expected["type"].as_str().unwrap());
        }
    }
    #[test]
    fn redaction_nulls_and_large_numbers() {
        for service in ["auth", "cert"] {
            let e = ClinkTraceEvent::Frame {
                at_nanos: 1,
                session: 2,
                direction: "RX".into(),
                port: 1,
                service: service.into(),
                category: 1,
                method: 1,
                kind: 1,
                sequence: u64::MAX,
                body_size: 42,
                fields: BTreeMap::from([(1, ClinkTraceField::Number(u64::MAX))]),
            };
            assert_eq!(e.to_json()["fields"], json!({}));
            assert_eq!(e.to_json()["sequence"].as_u64(), Some(u64::MAX));
        }
        let e = ClinkTraceEvent::Input {
            at_nanos: 1,
            session: 2,
            kind: "touch".into(),
            coordinates: vec![],
            raw_coordinates: vec![],
            stream_size: None,
            view_size: None,
        };
        assert_eq!(
            e.to_json(),
            json!({
                "type":"Input",
                "atNanos":1,
                "session":2,
                "kind":"touch",
                "coordinates":[],
                "rawCoordinates":[],
                "streamSize":null,
                "viewSize":null
            })
        );
        let b = Fields(BTreeMap::from([(1, FieldValue::Varint(u64::MAX))]))
            .encode()
            .unwrap();
        assert_eq!(
            decode_fields("control", &b, true)[&1],
            ClinkTraceField::Number(u64::MAX)
        );
    }
    #[test]
    fn field_previews_and_rejected_bodies() {
        let b = Fields(BTreeMap::from([
            (1, FieldValue::Bytes(vec![b'a'; 100])),
            (2, FieldValue::Bytes(vec![0, 255, 32])),
            (3, FieldValue::Bytes(vec![])),
        ]))
        .encode()
        .unwrap();
        let f = decode_fields("control", &b, true);
        assert_eq!(
            f[&1],
            ClinkTraceField::Bytes {
                length: 100,
                preview: "a".repeat(64)
            }
        );
        assert_eq!(
            f[&2],
            ClinkTraceField::Bytes {
                length: 3,
                preview: "00ff20".into()
            }
        );
        assert_eq!(
            f[&3],
            ClinkTraceField::Bytes {
                length: 0,
                preview: "".into()
            }
        );
        for service in ["auth", "cert"] {
            assert!(decode_fields(service, &b, true).is_empty());
        }
        assert!(decode_fields("control", &b, false).is_empty());
        for b in [&[0u8][..], &[8, 128][..], &[10, 9, 1][..]] {
            assert!(decode_fields("control", b, true).is_empty());
        }
    }
}
