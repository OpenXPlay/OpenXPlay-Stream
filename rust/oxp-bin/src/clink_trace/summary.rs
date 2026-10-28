// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

use super::{privacy, Callback};
use oxp_clink::trace::{ClinkTrace, ClinkTraceEvent};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{mpsc, Mutex},
    thread::JoinHandle,
};

/// Always-on, bounded baseline summary. Media and input hot events never reach its worker.
pub struct SummaryTrace {
    sender: Mutex<Option<mpsc::SyncSender<ClinkTraceEvent>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}
impl SummaryTrace {
    pub fn new(callback: Callback) -> Self {
        let (sender, receiver) = mpsc::sync_channel::<ClinkTraceEvent>(256);
        let worker = std::thread::spawn(move || {
            let mut summary = Summary::default();
            while let Ok(event) = receiver.recv() {
                if let Some(mut value) = summary.observe(event) {
                    privacy::redact(&mut value);
                    callback(value);
                }
            }
        });
        Self {
            sender: Mutex::new(Some(sender)),
            worker: Mutex::new(Some(worker)),
        }
    }
    pub fn shutdown(&self) {
        self.sender.lock().unwrap().take();
        if let Some(worker) = self.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
    }
}
impl ClinkTrace for SummaryTrace {
    fn record(&self, event: ClinkTraceEvent) {
        if matches!(
            event,
            ClinkTraceEvent::Input { .. } | ClinkTraceEvent::Media { .. }
        ) || matches!(&event, ClinkTraceEvent::Video {event,..} if ["live","reanchor","clock","codec_mismatch"].contains(&event.as_str()))
        {
            return;
        }
        if matches!(&event, ClinkTraceEvent::Frame {service,category,method,..}
            if service != "control" || *category != 1 || ![24,25].contains(method))
        {
            return;
        }
        if let Some(sender) = self.sender.lock().unwrap().as_ref() {
            let _ = sender.try_send(event);
        }
    }
}
impl Drop for SummaryTrace {
    fn drop(&mut self) {
        self.shutdown();
    }
}
#[derive(Default)]
struct Summary {
    fields: BTreeMap<String, String>,
    generation: u64,
    playing: bool,
    live: bool,
    emitted: bool,
}
impl Summary {
    fn observe(&mut self, event: ClinkTraceEvent) -> Option<Value> {
        let mut ended = false;
        match event {
            ClinkTraceEvent::Link { stage, fields, .. } => match stage.as_str() {
                "link_start" => {
                    *self = Self::default();
                    self.generation = fields
                        .get("generation")
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                }
                "ble_advertisement" | "link_connecting" | "ble_reply_name" => {
                    self.copy(&fields, "name", "name")
                }
                "head_unit_certificate" => {
                    self.copy(&fields, "vendor_name", "vendor");
                    self.copy(&fields, "product_name", "product");
                }
                "link_joined" | "wifi_handoff" => self.copy(&fields, "ssid", "ssid"),
                "hevc_m4" => self.copy(&fields, "peer", "hevc"),
                "ble_exchange_done" | "link_diagnostic" => {
                    if fields.contains_key("frequency") {
                        self.copy(&fields, "frequency", "frequency");
                    } else if let Some(detail) = fields.get("detail").or(fields.get("reason")) {
                        if let Some(frequency) = detail
                            .split("frequency=")
                            .nth(1)
                            .and_then(|s| s.split_whitespace().next())
                        {
                            self.fields.insert("frequency".into(), frequency.into());
                        }
                    }
                }
                "view" => self.copy(&fields, "size", "view"),
                "window" => self.copy(&fields, "size", "window"),
                "statistics" => {
                    self.copy(&fields, "stream", "stream");
                    self.copy(&fields, "codec", "codec");
                    self.live |= fields
                        .get("liveFrames")
                        .and_then(|s| s.parse::<u64>().ok())
                        .is_some_and(|n| n > 0);
                }
                "state" => {
                    let state = fields
                        .get("state")
                        .or(fields.get("detail"))
                        .map(String::as_str)
                        .unwrap_or("");
                    self.playing = matches!(state, "Playing" | "state=PLAYING");
                    ended = matches!(state, "Ended" | "state=ENDED");
                }
                _ => (),
            },
            ClinkTraceEvent::Negotiation {
                mode,
                decoders,
                audio,
                ..
            } => {
                self.fields.insert("mode".into(), mode);
                self.fields.insert("audio".into(), audio.to_string());
                self.fields.insert("codec".into(), decoders.join(","));
            }
            ClinkTraceEvent::Rtsp {
                direction, body, ..
            } if direction == "RX" => {
                for (key, value) in oxp_clink::wfd::parameters(&body) {
                    if let Some(name) = match key.as_str() {
                        "iccoa_support_hevc" => Some("hevc"),
                        "wfd_uibc_capability" => Some("uibc"),
                        _ => None,
                    } {
                        self.fields.insert(name.into(), value);
                    }
                }
            }
            ClinkTraceEvent::Video {
                event,
                codec,
                width,
                height,
                ..
            } if event == "format" => {
                if let Some(codec) = codec {
                    self.fields.insert("codec".into(), codec);
                }
                if let (Some(w), Some(h)) = (width, height) {
                    self.fields.insert("stream".into(), format!("{w}x{h}"));
                }
            }
            ClinkTraceEvent::Frame {
                direction, fields, ..
            } if direction == "RX" => {
                use oxp_clink::trace::ClinkTraceField::Number;
                if let (Some(Number(width)), Some(Number(height))) =
                    (fields.get(&2), fields.get(&3))
                {
                    self.fields
                        .insert("window".into(), format!("{width}x{height}"));
                }
            }
            _ => (),
        }
        if self.emitted
            || !(ended || self.playing && self.live)
            || self.fields.values().all(String::is_empty)
        {
            return None;
        }
        self.emitted = true;
        for name in [
            "vendor",
            "product",
            "name",
            "ssid",
            "frequency",
            "mode",
            "view",
            "window",
            "stream",
            "codec",
            "hevc",
            "audio",
            "uibc",
        ] {
            self.fields
                .entry(name.into())
                .or_insert_with(|| "unknown".into());
        }
        Some(
            json!({"type":"clink-summary","generation":self.generation,"atMillis":super::unix_ms(),"fields":self.fields}),
        )
    }
    fn copy(&mut self, fields: &BTreeMap<String, String>, key: &str, name: &str) {
        if let Some(value) = fields.get(key) {
            self.fields.insert(name.into(), value.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn link(stage: &str, fields: &[(&str, &str)]) -> ClinkTraceEvent {
        ClinkTraceEvent::Link {
            at_nanos: 0,
            session: 1,
            stage: stage.into(),
            fields: fields
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }
    #[test]
    fn frequency_fields_take_precedence_and_legacy_records_still_work() {
        for (stage, fields, frequency) in [
            (
                "ble_exchange_done",
                vec![("frequency", "5180"), ("detail", "frequency=2412")],
                "5180",
            ),
            (
                "ble_exchange_done",
                vec![("detail", "done frequency=2412 replyType=1002")],
                "2412",
            ),
            (
                "link_diagnostic",
                vec![("reason", "frequency=5745")],
                "5745",
            ),
        ] {
            let mut summary = Summary::default();
            assert!(summary.observe(link(stage, &fields)).is_none());
            let value = summary
                .observe(link("state", &[("state", "Ended")]))
                .unwrap();
            assert_eq!(value["fields"]["frequency"], frequency);
            assert!(summary
                .observe(link("state", &[("state", "Ended")]))
                .is_none());
            summary.observe(link("link_start", &[("generation", "3")]));
            summary.observe(link("ble_advertisement", &[("name", "next unit")]));
            let next = summary
                .observe(link("state", &[("state", "Ended")]))
                .unwrap();
            assert_eq!(next["generation"], 3);
            assert_eq!(next["fields"]["frequency"], "unknown");
        }
    }
    #[test]
    fn summary_keeps_android_identity_fields_and_emits_once() {
        let mut s = Summary::default();
        for e in [
            link(
                "head_unit_certificate",
                &[("vendor_name", "vendor"), ("product_name", "product")],
            ),
            link(
                "ble_exchange_done",
                &[("frequency", "5180"), ("replyType", "1002")],
            ),
            link("hevc_m4", &[("peer", "supported")]),
            ClinkTraceEvent::Negotiation {
                at_nanos: 0,
                session: 1,
                mode: "720p".into(),
                decoders: vec!["hvc1".into()],
                audio: true,
                rtp_port: 1,
                display: String::new(),
            },
            ClinkTraceEvent::Rtsp {
                at_nanos: 0,
                session: 1,
                direction: "RX".into(),
                start_line: String::new(),
                headers: BTreeMap::new(),
                body: "wfd_uibc_capability: touch".into(),
            },
            link("state", &[("state", "Playing")]),
        ] {
            assert!(s.observe(e).is_none());
        }
        let v = s
            .observe(link(
                "statistics",
                &[("liveFrames", "1"), ("stream", "1280x720")],
            ))
            .unwrap();
        for (key, value) in [
            ("vendor", "vendor"),
            ("product", "product"),
            ("frequency", "5180"),
            ("hevc", "supported"),
            ("audio", "true"),
            ("uibc", "touch"),
        ] {
            assert_eq!(v["fields"][key], value);
        }
        assert!(s.observe(link("state", &[("state", "Ended")])).is_none());
    }
}
