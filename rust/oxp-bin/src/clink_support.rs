// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use crate::*;
use oxp_clink::{model::*, *};
use oxp_wire::{Header, Kind, Message, Result};
use serde_json::json;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::SyncSender,
        Arc, Condvar, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
pub struct Pin {
    pub digits: Option<String>,
    pub file: Option<String>,
    pub stopped: Arc<AtomicBool>,
    /// Only a host that has already obtained explicit pairing consent may set this.
    pub auto_confirm: bool,
    pub requests: Option<Arc<PairingRequests>>,
}
impl Pin {
    pub fn request_with(
        &self,
        id: &str,
        timeout: Duration,
        emit: impl Fn(serde_json::Value),
    ) -> Option<String> {
        if let Some(requests) = &self.requests {
            if let Some(pin) = &self.digits {
                crate::clink_trace::remember_secret(pin);
                return Some(pin.clone());
            }
            if let Some(path) = &self.file {
                if let Err(e) = std::fs::remove_file(path) {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        return None;
                    }
                }
            }
            let pin = requests.ask_poll(id, false, timeout, &self.stopped, emit, || {
                let path = self.file.as_ref()?;
                match std::fs::read_to_string(path) {
                    Ok(pin) => {
                        let removed = std::fs::remove_file(path).is_ok();
                        let pin = pin.trim();
                        Some(
                            (removed
                                && !pin.is_empty()
                                && pin.len() <= 128
                                && pin.bytes().all(|b| b.is_ascii_digit()))
                            .then(|| pin.to_owned()),
                        )
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                    Err(_) => Some(None),
                }
            });
            if let Some(pin) = &pin {
                crate::clink_trace::remember_secret(pin);
            }
            if let Some(path) = &self.file {
                let _ = std::fs::remove_file(path);
            }
            return pin;
        }
        if let Some(path) = &self.file {
            if let Err(e) = std::fs::remove_file(path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    emit(json!({"type":"error","message":"Cannot clear PIN file"}));
                    return None;
                }
            }
        }
        emit(json!({"type":"pin-request","client":id}));
        let pin = if let Some(path) = &self.file {
            let deadline = Instant::now() + timeout;
            loop {
                if self.stopped.load(Ordering::Acquire) {
                    return None;
                }
                if Instant::now() >= deadline {
                    emit(json!({"type":"pin-timeout","client":id}));
                    return None;
                }
                match std::fs::read_to_string(path) {
                    Ok(value) => {
                        if std::fs::remove_file(path).is_err() {
                            emit(json!({"type":"error","message":"Cannot delete PIN file"}));
                            return None;
                        }
                        break value.trim().to_owned();
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => {
                        let _ = std::fs::remove_file(path);
                        emit(json!({"type":"error","message":"Cannot read PIN file"}));
                        return None;
                    }
                }
                std::thread::sleep(
                    Duration::from_millis(25)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            }
        } else {
            self.digits.clone()?
        };
        if pin.is_empty() || !pin.bytes().all(|b| b.is_ascii_digit()) {
            emit(json!({"type":"error","message":"PIN must contain digits"}));
            return None;
        }
        crate::clink_trace::remember_secret(&pin);
        Some(pin)
    }
}
impl PinProvider for Pin {
    fn request_pin(&self, id: &str) -> Option<String> {
        self.request_with(id, Duration::from_secs(120), event)
    }
    fn confirm_pairing(&self, id: &str) -> bool {
        if let Some(requests) = &self.requests {
            requests
                .ask(id, true, Duration::from_secs(120), &self.stopped, event)
                .as_deref()
                == Some("approved")
        } else {
            self.auto_confirm && !self.stopped.load(Ordering::Acquire)
        }
    }
    fn cancel(&self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(requests) = &self.requests {
            requests.cancel();
        }
    }
    fn pin_incorrect(&self, id: &str) {
        event(json!({"type":"clink-pin","result":"incorrect","client":id}));
    }
}

struct PendingPairing {
    id: u64,
    confirmation: bool,
    answer: Option<Option<String>>,
}
/// One output generation owns one prompt. Request IDs never repeat within this process.
#[derive(Default)]
pub struct PairingRequests {
    generation: u64,
    pending: Mutex<Option<PendingPairing>>,
    wake: Condvar,
}
impl PairingRequests {
    pub fn new(generation: u64) -> Self {
        Self {
            generation,
            ..Default::default()
        }
    }
    pub fn ask(
        &self,
        client: &str,
        confirmation: bool,
        timeout: Duration,
        stopped: &AtomicBool,
        emit: impl Fn(serde_json::Value),
    ) -> Option<String> {
        self.ask_poll(client, confirmation, timeout, stopped, emit, || None)
    }
    #[allow(clippy::too_many_arguments)] // The optional legacy PIN-file poll shares this prompt lifecycle.
    fn ask_poll(
        &self,
        client: &str,
        confirmation: bool,
        timeout: Duration,
        stopped: &AtomicBool,
        emit: impl Fn(serde_json::Value),
        mut poll: impl FnMut() -> Option<Option<String>>,
    ) -> Option<String> {
        use std::sync::atomic::AtomicU64;
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        {
            let mut pending = self.pending.lock().unwrap();
            if stopped.load(Ordering::Acquire) || pending.is_some() {
                return None;
            }
            *pending = Some(PendingPairing {
                id,
                confirmation,
                answer: None,
            });
        }
        emit(
            json!({"type":"pin-request","client":client,"request":id,"generation":self.generation,"confirmation":confirmation}),
        );
        let deadline = Instant::now() + timeout;
        let mut pending = self.pending.lock().unwrap();
        let result = loop {
            if stopped.load(Ordering::Acquire) {
                break None;
            }
            if let Some(answer) = pending.as_mut().and_then(|p| p.answer.take()) {
                break answer;
            }
            if let Some(answer) = poll() {
                break answer;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break None;
            }
            pending = self
                .wake
                .wait_timeout(pending, remaining.min(Duration::from_millis(25)))
                .unwrap()
                .0;
        };
        *pending = None;
        drop(pending);
        emit(
            json!({"type":"clink-pairing","request":id,"generation":self.generation,"result":if result.is_some() {"answered"} else {"cancelled"}}),
        );
        result
    }
    /// Returns an error for stale IDs, duplicate responses, or a response of the wrong kind.
    pub fn respond(&self, value: &serde_json::Value) -> std::result::Result<bool, &'static str> {
        let id = value["request"].as_u64().ok_or("invalid_request")?;
        let mut pending = self.pending.lock().unwrap();
        let p = pending
            .as_mut()
            .filter(|p| p.id == id && p.answer.is_none())
            .ok_or("stale_request")?;
        let cancelled = value["action"] == "cancel";
        let answer = match value["action"].as_str() {
            Some("cancel") => None,
            Some("approve") if p.confirmation => Some("approved".into()),
            Some("pin") if !p.confirmation => {
                let pin = value["pin"]
                    .as_str()
                    .filter(|s| {
                        !s.is_empty() && s.len() <= 128 && s.bytes().all(|b| b.is_ascii_digit())
                    })
                    .ok_or("invalid_pin")?;
                Some(pin.into())
            }
            _ => return Err("invalid_response"),
        };
        p.answer = Some(answer);
        self.wake.notify_all();
        Ok(cancelled)
    }
    pub fn cancel(&self) {
        if let Some(pending) = self.pending.lock().unwrap().as_mut() {
            pending.answer = Some(None);
            self.wake.notify_all();
        }
    }
}
pub fn private_event(mut value: serde_json::Value) {
    crate::clink_trace::redact(&mut value);
    event(value);
}
pub fn link_event(state: oxp_link_linux::LinkEvent) -> serde_json::Value {
    use oxp_link_linux::LinkEvent;
    match state {
        LinkEvent::Advertisement {
            advertisement: a,
            serial_same,
        } => {
            json!({"type":"ble_advertisement","major":a.major,"minor":a.minor,"serial":a.serial,"carId":a.car_id,"modelId":a.model_id,"serialSame":serial_same})
        }
        LinkEvent::Scanning => json!({"type":"link","state":"scanning"}),
        LinkEvent::Connecting { address, name } => {
            json!({"type":"link","state":"connecting","headUnit":{"address":address,"name":name}})
        }
        LinkEvent::Joined {
            address,
            name,
            ssid,
            interface,
            addresses,
        } => {
            json!({"type":"link","state":"joined","headUnit":{"address":address,"name":name},"network":{"ssid":ssid,"interface":interface},"headUnitIp":addresses.head_unit_ipv4.to_string(),"localIp":addresses.local_ipv4.to_string()})
        }
        LinkEvent::Lost { reason } => json!({"type":"link","state":"lost","reason":reason}),
        LinkEvent::Diagnostic { reason } => {
            json!({"type":"diagnostic","message":format!("link {reason}")})
        }
    }
}
/// Relay-shell event, deliberately outside the media wire protocol. Timestamps stay lossless in JS.
pub fn vehicle_event(origin: Origin, data: VehicleData) -> serde_json::Value {
    let data = match data {
        VehicleData::Gps {
            height,
            latitude,
            longitude,
            antenna_state,
            pdop,
            speed,
            heading,
            satellites_used,
            satellites_visible,
            timestamp,
        } => json!({"type":"Gps", "height":height,
            "latitude":latitude,"longitude":longitude,"antennaState":antenna_state,"pdop":pdop,"speed":speed,
            "heading":heading,"satellitesUsed":satellites_used,"satellitesVisible":satellites_visible,"timestamp":timestamp.to_string()}),
        VehicleData::Lights {
            low_beam,
            high_beam,
            position_lamp,
            reverse_lamp,
            brake_lamp,
        } => json!({"type":"Lights",
            "lowBeam":low_beam,"highBeam":high_beam,"positionLamp":position_lamp,"reverseLamp":reverse_lamp,"brakeLamp":brake_lamp}),
        VehicleData::Gyroscope {
            kind,
            x,
            y,
            z,
            timestamp,
        } => json!({"type":"Gyroscope","gyroscopeType":match kind {
            GyroscopeType::Unknown => "UNKNOWN", GyroscopeType::Single => "SINGLE", GyroscopeType::ThreeAxis => "THREE_AXIS" },
            "x":x,"y":y,"z":z,"timestamp":timestamp.to_string()}),
        VehicleData::Acceleration { x, y, z, timestamp } => {
            json!({"type":"Acceleration","x":x,"y":y,"z":z,"timestamp":timestamp.to_string()})
        }
        VehicleData::Fuel { max, current } => json!({"type":"Fuel","max":max,"current":current}),
        VehicleData::Gear { gear, speed } => json!({"type":"Gear","gear":match gear {
            GearPosition::Unknown => "UNKNOWN", GearPosition::P => "P", GearPosition::D => "D", GearPosition::R => "R", GearPosition::N => "N" },"speed":speed}),
        VehicleData::LightSensor {
            max_lux,
            min_lux,
            current_lux,
        } => {
            json!({"type":"LightSensor","maxLux":max_lux,"minLux":min_lux,"currentLux":current_lux})
        }
        VehicleData::DayNight { mode } => json!({"type":"DayNight","mode":match mode {
            DayNightMode::Unknown => "UNKNOWN", DayNightMode::Day => "DAY", DayNightMode::Night => "NIGHT" }}),
    };
    json!({"type":"clink-vehicle","atMillis":SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis(),"origin":{"outputId":origin.output_id,"clientId":origin.client_id},"data":data})
}
pub struct Listener;
fn view_size_event(width: u32, height: u32) -> serde_json::Value {
    json!({"type":"clink-view-size","width":width,"height":height})
}
fn media_stats_event(stats: ClinkStatistics) -> serde_json::Value {
    json!({
        "type":"media-stats",
        "sent":stats.live_frames + stats.placeholder_frames,
        "dropped":stats.dropped_frames,
        "liveFrames":stats.live_frames,
        "placeholderFrames":stats.placeholder_frames,
        "droppedFrames":stats.dropped_frames,
        "liveBytes":stats.live_bytes,
        "sendStalls":stats.send_stalls,
        "codec":stats.codec.map(|codec| match codec { VideoCodec::H264 => "H264", VideoCodec::H265 => "H265" }),
        "width":stats.width,
        "height":stats.height,
    })
}
impl ClinkListener for Listener {
    fn state(&self, s: ClinkState) {
        if s == ClinkState::Ended {
            // Callback events queued before the end stay ahead of it.
            EventLog::flush_global(Duration::from_millis(200));
        }
        event(json!({"type":"state","state":format!("{s:?}")}));
    }
    fn error(&self, e: &str) {
        private_event(json!({"type":"error","message":e}));
    }
    fn diagnostic(&self, message: &str) {
        private_event(json!({"type":"diagnostic","message":message}));
    }
    fn view_size(&self, width: u32, height: u32) {
        event(view_size_event(width, height));
    }
    fn statistics(&self, stats: ClinkStatistics) {
        event(media_stats_event(stats));
    }
    fn vehicle(&self, origin: Origin, data: VehicleData) {
        event(vehicle_event(origin, data));
    }
    fn vehicle_state(&self, active: bool, malformed: u64) {
        event(
            json!({"type":"clink-vehicle-state","sensorActive":active,"malformedFrames":malformed}),
        );
    }
    fn profile_mismatch(&self, input: &str, decoders: &[String]) {
        event(json!({"type":"profile-mismatch","input":input,"decoders":decoders}));
    }
}
pub struct Sink {
    pub tx: SyncSender<Message>,
    pub state: Mutex<(String, [u32; 256])>,
    pub clock: Instant,
    pub shutdown: Arc<dyn Fn() + Send + Sync>,
    pub failed: Arc<AtomicBool>,
    /// Callbacks may run on the producer's wire thread; events never wait for stderr there.
    pub log: Arc<EventLog>,
}
impl Sink {
    pub fn send(&self, m: Message) {
        if let Message::Json(v) = &m {
            self.log.try_log(v.clone());
        }
        if let Err(e) = self.tx.try_send(m) {
            self.log
                .try_log(json!({"type":"error","message":format!("Wire callback queue: {e}")}));
            self.failed.store(true, Ordering::Release);
            (self.shutdown)();
        }
    }
    fn binary(&self, s: &mut (String, [u32; 256]), kind: Kind, screen: u8, payload: Vec<u8>) {
        let seq = &mut s.1[screen as usize];
        let m = Message::Binary {
            header: Header {
                kind,
                stream: screen,
                flags: 0,
                sequence: *seq,
                pts_us: 0,
                sender_clock_us: self.clock.elapsed().as_micros() as i64,
            },
            payload,
        };
        *seq = seq.wrapping_add(1);
        self.send(m);
    }
    fn client(&self, s: &mut (String, [u32; 256]), id: &str) {
        if !id.is_empty() && s.0 != id {
            self.send(json_message(json!({"type":"client","client":id})));
            s.0 = id.into();
        }
    }
}
impl OutputSink for Sink {
    fn capabilities(&self, c: OutputCapabilities) {
        let displays: serde_json::Map<String, serde_json::Value> = c.displays.iter().map(|(s, d)| {
            let mut value = json!({"widthPx":d.width_px,"heightPx":d.height_px,"densityDpi":d.density_dpi,"refreshHz":d.refresh_hz,"safeArea":{"left":d.safe_area.left,"top":d.safe_area.top,"right":d.safe_area.right,"bottom":d.safe_area.bottom}});
            if d.frame_rate != FrameRatePreference::Auto { value["frameRate"] = json!(d.frame_rate.wire_name()); }
            if let Some(fps) = d.sustained_fps { value["sustainedFps"] = json!(fps); }
            if let Some(v) = &d.viewport { value["viewport"] = json!({"widthPx":v.width_px,"heightPx":v.height_px}); }
            if let Some(v) = d.fullscreen { value["fullscreen"] = json!(v); }
            if let Some(v) = d.fullscreen_preferred { value["fullscreenPreferred"] = json!(v); }
            (s.to_string(), value)
        }).collect();
        let mut value = json!({"type":"capabilities","latency":match c.latency{LatencyClass::Interactive=>"INTERACTIVE",LatencyClass::Buffered=>"BUFFERED"},"decoders":c.decoders.iter().map(|d|json!({"codecString":d.codec_string,"hardware":d.hardware})).collect::<Vec<_>>(),"displays":displays,"touch":c.touch,"keys":c.keys.iter().map(|k|k.wire_code()).collect::<Vec<_>>(),"audio":c.audio.iter().map(|a|match a{AudioCodec::AacLc=>"AAC_LC",AudioCodec::Opus=>"OPUS",AudioCodec::PcmS16le=>"PCM_S16LE"}).collect::<Vec<_>>(),"microphone":c.microphone});
        if c.prefers_h264 {
            value["prefersH264"] = json!(true);
        }
        self.send(json_message(value));
    }
    fn request_restart(&self, origin: Origin) {
        let mut s = self.state.lock().unwrap();
        self.client(&mut s, &origin.client_id);
        self.send(json_message(json!({"type":"restart"})));
    }
    fn control(&self, e: ControlEvent) {
        let mut s = self.state.lock().unwrap();

        match e {
            ControlEvent::DisplaySettings { settings, origin } => {
                self.client(&mut s, &origin.client_id);
                self.send(json_message(oxp_wire::display_settings(&settings)));
            }
            ControlEvent::Touch {
                screen,
                contacts,
                origin,
            } => {
                self.log.try_log(
                    json!({"type":"touch","screen":screen,"client":origin.client_id,"contacts":contacts.iter().map(|c|json!({"id":c.id,"x":c.x,"y":c.y,"pressed":c.pressed})).collect::<Vec<_>>()}),
                );
                self.client(&mut s, &origin.client_id);
                if let Ok(p) = oxp_wire::encode_touch(
                    &contacts
                        .iter()
                        .map(|c| oxp_wire::Contact {
                            id: c.id,
                            x: c.x,
                            y: c.y,
                            pressed: c.pressed,
                        })
                        .collect::<Vec<_>>(),
                ) {
                    self.binary(&mut s, Kind::Touch, screen as u8, p);
                }
            }
            ControlEvent::Key { code, down, origin } => {
                self.log.try_log(
                    json!({"type":"key","code":code.wire_code(),"down":down,"client":origin.client_id}),
                );
                self.client(&mut s, &origin.client_id);
                if let Ok(p) = oxp_wire::encode_key(code.wire_code(), down) {
                    self.binary(&mut s, Kind::Key, 0, p);
                }
            }
            ControlEvent::KeyFrameRequest { screen, origin } => {
                self.client(&mut s, &origin.client_id);
                self.send(json_message(json!({"type":"keyframe","screen":screen})));
            }
            ControlEvent::MicPacket {
                stream,
                data,
                pts_us,
                origin,
            } => {
                self.client(&mut s, &origin.client_id);
                if let Ok(stream) = u8::try_from(stream) {
                    let seq = &mut s.1[stream as usize];
                    let header = Header {
                        kind: Kind::Microphone,
                        stream,
                        flags: 0,
                        sequence: *seq,
                        pts_us,
                        sender_clock_us: oxp_core::monotonic_nanos() / 1000,
                    };
                    *seq = seq.wrapping_add(1);
                    self.send(Message::Binary {
                        header,
                        payload: data.to_vec(),
                    });
                }
            }
        }
    }
    fn stats(&self, s: OutputStats) {
        let mut state = self.state.lock().unwrap();
        self.client(&mut state, &s.origin.client_id);
        let mut value = json!({"type":"stats","screen":s.screen,"decodeMs":s.decode_time_us.map(|n|n as f64/1000.0),"decodeQueue":s.decode_queue,"dropped":s.dropped,"rendered":s.rendered,"decodedLatencyMs":null,"renderedLatencyMs":s.end_to_end_latency_us.map(|n|n as f64/1000.0),"clockOffsetUs":null,"rttUs":null,"audioBufferedMs":s.audio_buffered_us.map(|n|n as f64/1000.0),"audioUnderruns":s.audio_underruns});
        if let Some(session) = s.session {
            value["session"] = json!(session.to_string());
        }
        self.send(json_message(value));
    }
}
/// `--clink-transcode`: `auto` (default) transcodes streams the head unit cannot decode when a
/// hardware transcoder exists; `off` forwards every stream unchanged.
pub fn transcode_enabled(value: Option<&str>) -> Result<bool> {
    match value.unwrap_or("auto") {
        "auto" => Ok(true),
        "off" => Ok(false),
        _ => Err("--clink-transcode must be auto or off".into()),
    }
}
pub fn hevc_mode(value: Option<&str>) -> Result<ClinkHevcMode> {
    match value.unwrap_or("auto") {
        "auto" => Ok(ClinkHevcMode::Auto),
        "off" => Ok(ClinkHevcMode::Off),
        "signal" => Ok(ClinkHevcMode::Signal),
        _ => Err("--clink-hevc must be auto, off or signal".into()),
    }
}
/// Wraps an output so decisions and failures appear as relay diagnostics. The factory is probed
/// here, at startup, and the devices it found are logged once.
pub fn with_transcoding(
    output: Arc<dyn oxp_core::ProjectionOutput>,
    factory: Arc<dyn oxp_transcode_linux::TranscoderFactory>,
    log: Arc<dyn Fn(&str) + Send + Sync>,
) -> Arc<dyn oxp_core::ProjectionOutput> {
    let output = oxp_transcode_linux::TranscodingOutput::new(output, factory.clone(), log.clone());
    log(&factory.describe());
    Arc::new(output)
}
pub fn diagnostic(message: &str) {
    event(json!({"type":"diagnostic","message":message}));
}
pub fn parse_ports(value: Option<&str>) -> Result<ClinkPorts> {
    let mut ports = ClinkPorts::default();
    let mut seen = std::collections::BTreeSet::new();
    if let Some(value) = value {
        for entry in value.split(',') {
            let (name, number) = entry.split_once('=').ok_or("Expected service=port")?;
            if !seen.insert(name) {
                return Err("Duplicate port service".into());
            }
            let port: u16 = number.parse()?;
            match name {
                "auth" => ports.auth = port,
                "control" => ports.control = port,
                "media" => ports.media = port,
                "cert" => ports.cert = port,
                "rtsp" => ports.rtsp = port,
                "uibc" => ports.uibc = port,
                "rtp" => ports.rtp = port,
                "sensor" => ports.sensor = port,
                _ => return Err(format!("Unknown port service: {name}").into()),
            }
        }
    }
    ports.validate()?;
    Ok(ports)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::sync::atomic::AtomicUsize;
    static NEXT: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn view_size_json_uses_the_relay_event_contract() {
        assert_eq!(
            view_size_event(1532, 1013),
            json!({"type":"clink-view-size","width":1532,"height":1013})
        );
    }
    #[test]
    fn media_stats_json_keeps_old_fields_and_adds_session_counters_and_format() {
        assert_eq!(
            media_stats_event(ClinkStatistics::default()),
            json!({
                "type":"media-stats","sent":0,"dropped":0,"liveFrames":0,
                "placeholderFrames":0,"droppedFrames":0,"liveBytes":0,"sendStalls":0,
                "codec":null,"width":0,"height":0,
            })
        );
        for (codec, name) in [(VideoCodec::H264, "H264"), (VideoCodec::H265, "H265")] {
            assert_eq!(
                media_stats_event(ClinkStatistics {
                    live_frames: 2,
                    placeholder_frames: 3,
                    dropped_frames: 4,
                    live_bytes: 5678,
                    send_stalls: 6,
                    codec: Some(codec),
                    width: 1450,
                    height: 960,
                }),
                json!({
                    "type":"media-stats","sent":5,"dropped":4,"liveFrames":2,
                    "placeholderFrames":3,"droppedFrames":4,"liveBytes":5678,"sendStalls":6,
                    "codec":name,"width":1450,"height":960,
                })
            );
        }
    }
    #[test]
    fn wire_sink_preserves_preferences_and_orders_capabilities_before_restart() {
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        let sink = Sink {
            tx,
            state: Mutex::new(("default".into(), [0; 256])),
            clock: Instant::now(),
            shutdown: Arc::new(|| panic!("unexpected wire shutdown")),
            failed: Arc::new(AtomicBool::new(false)),
            log: EventLog::global().clone(),
        };
        for rate in (30..=60).step_by(5) {
            let preference = FrameRatePreference::from_wire(&format!("FPS_{rate}")).unwrap();
            sink.capabilities(OutputCapabilities {
                prefers_h264: rate == 35,
                latency: LatencyClass::Interactive,
                decoders: Vec::new(),
                displays: [(
                    0,
                    DisplayInfo {
                        frame_rate: preference,
                        sustained_fps: Some(55.0),
                        viewport: Some(Viewport {
                            width_px: 1533,
                            height_px: 907,
                        }),
                        fullscreen: Some(false),
                        fullscreen_preferred: Some(true),
                        ..DisplayInfo::default()
                    },
                )]
                .into(),
                touch: 0,
                keys: Default::default(),
                audio: Default::default(),
                microphone: false,
            });
            sink.request_restart(Origin {
                output_id: "web".into(),
                client_id: "browser".into(),
            });
            let Message::Json(caps) = rx.recv().unwrap() else {
                panic!("expected capabilities")
            };
            oxp_wire::validate_json(&caps).unwrap();
            assert_eq!(
                caps.get("prefersH264"),
                if rate == 35 { Some(&json!(true)) } else { None }
            );
            assert_eq!(caps["displays"]["0"]["frameRate"], format!("FPS_{rate}"));
            assert_eq!(caps["displays"]["0"]["sustainedFps"], 55.0);
            assert_eq!(
                caps["displays"]["0"]["viewport"],
                json!({"widthPx":1533,"heightPx":907})
            );
            assert_eq!(caps["displays"]["0"]["fullscreen"], false);
            assert_eq!(caps["displays"]["0"]["fullscreenPreferred"], true);
            let mut next = rx.recv().unwrap();
            if let Message::Json(v) = &next {
                if v["type"] == "client" {
                    next = rx.recv().unwrap();
                }
            }
            let Message::Json(restart) = next else {
                panic!("expected restart")
            };
            assert_eq!(restart, json!({"type":"restart"}));
        }
    }

    #[test]
    fn blocked_event_writer_cannot_stall_sink_send() {
        let (release, gate) = std::sync::mpsc::channel::<()>();
        let (started, writing) = std::sync::mpsc::channel();
        let lines = Arc::new(Mutex::new(Vec::new()));
        let written = lines.clone();
        // Stands in for a stderr whose reader stopped reading.
        let log = EventLog::spawn(8, move |v| {
            let _ = started.send(());
            let _ = gate.recv();
            written.lock().unwrap().push(v);
        })
        .unwrap();
        let (tx, rx) = std::sync::mpsc::sync_channel(4096);
        let sink = Sink {
            tx,
            state: Mutex::new(("default".into(), [0; 256])),
            clock: Instant::now(),
            shutdown: Arc::new(|| panic!("unexpected wire shutdown")),
            failed: Arc::new(AtomicBool::new(false)),
            log: log.clone(),
        };
        let keyframe = |n| json_message(json!({"type":"keyframe","screen":n}));
        sink.send(keyframe(0));
        writing.recv().unwrap();
        let start = Instant::now();
        for n in 1..1000 {
            sink.send(keyframe(n));
        }
        sink.control(ControlEvent::Touch {
            screen: 0,
            contacts: vec![TouchContact::new(0, 0.5, 0.25, true).unwrap()],
            origin: Origin {
                output_id: "web".into(),
                client_id: "browser".into(),
            },
        });
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "a blocked event writer stalled send"
        );
        // Every message still reaches the wire: 1000 keyframes, the client switch and the touch.
        assert_eq!(1002, rx.try_iter().count());
        // Events: 1000 keyframes, the touch and the client switch; one in the writer, eight queued.
        assert_eq!(1002 - 9, log.dropped());
        drop(release);
        assert!(log.flush(Duration::from_secs(5)));
        let lines = lines.lock().unwrap();
        assert_eq!(10, lines.len());
        assert!((0..9).all(|n| lines[n] == json!({"type":"keyframe","screen":n})));
        assert_eq!(
            lines[9],
            json!({"type":"diagnostic","message":"event log dropped=993"})
        );
    }

    fn pin() -> Pin {
        Pin {
            digits: None,
            file: Some(
                std::env::temp_dir()
                    .join(format!(
                        "oxp-pin-{}-{}",
                        std::process::id(),
                        NEXT.fetch_add(1, Ordering::Relaxed)
                    ))
                    .to_string_lossy()
                    .into_owned(),
            ),
            stopped: Arc::new(AtomicBool::new(false)),
            auto_confirm: false,
            requests: None,
        }
    }
    #[test]
    fn pin_waits_for_new_file_and_deletes_it_after_read() {
        let pin = pin();
        let path = pin.file.as_ref().unwrap();
        std::fs::write(path, "stale").unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let writer_path = path.clone();
        let writer = std::thread::spawn(move || {
            rx.recv().unwrap();
            assert!(!std::path::Path::new(&writer_path).exists());
            std::thread::sleep(Duration::from_millis(40));
            std::fs::write(writer_path, "001234\n").unwrap();
        });
        let events = RefCell::new(Vec::new());
        let result = pin.request_with("head-unit", Duration::from_secs(2), |v| {
            if v["type"] == "pin-request" {
                tx.send(()).unwrap();
            }
            events.borrow_mut().push(v);
        });
        writer.join().unwrap();
        assert_eq!(result.as_deref(), Some("001234"));
        assert!(!std::path::Path::new(path).exists());
        assert_eq!(
            *events.borrow(),
            vec![json!({"type":"pin-request","client":"head-unit"})]
        );
    }
    #[test]
    fn pin_timeout_emits_client_and_clears_stale_file() {
        let pin = pin();
        let path = pin.file.as_ref().unwrap();
        std::fs::write(path, "123456").unwrap();
        let events = RefCell::new(Vec::new());
        assert!(pin
            .request_with("unit", Duration::from_millis(30), |v| events
                .borrow_mut()
                .push(v))
            .is_none());
        assert!(!std::path::Path::new(path).exists());
        assert_eq!(
            *events.borrow(),
            vec![
                json!({"type":"pin-request","client":"unit"}),
                json!({"type":"pin-timeout","client":"unit"})
            ]
        );
    }
    #[test]
    fn pin_rejects_non_digits_and_deletes_file() {
        for value in ["", "12x4", "１２３４"] {
            let pin = pin();
            let path = pin.file.as_ref().unwrap();
            let events = RefCell::new(Vec::new());
            assert!(pin
                .request_with("unit", Duration::from_secs(1), |v| {
                    if v["type"] == "pin-request" {
                        std::fs::write(path, value).unwrap();
                    }
                    events.borrow_mut().push(v);
                })
                .is_none());
            assert!(!std::path::Path::new(path).exists());
            assert_eq!(
                events.borrow()[1],
                json!({"type":"error","message":"PIN must contain digits"})
            );
        }
    }
    #[test]
    fn pin_wait_is_interruptible_without_timeout_event() {
        let pin = pin();
        let events = RefCell::new(Vec::new());
        let start = Instant::now();
        assert!(pin
            .request_with("unit", Duration::from_secs(120), |v| {
                pin.stopped.store(true, Ordering::Release);
                events.borrow_mut().push(v);
            })
            .is_none());
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(events.borrow().len(), 1);
    }
    #[test]
    fn explicit_pin_remains_supported() {
        let mut pin = pin();
        pin.file = None;
        pin.digits = Some("001234".into());
        assert_eq!(
            pin.request_with("unit", Duration::ZERO, |_| {}),
            Some("001234".into())
        );
    }
    #[test]
    fn pairing_confirmation_requires_auto_confirm_and_an_active_session() {
        let mut pin = pin();
        assert!(!pin.confirm_pairing("unit"));
        pin.auto_confirm = true;
        assert!(pin.confirm_pairing("unit"));
        pin.stopped.store(true, Ordering::Release);
        assert!(!pin.confirm_pairing("unit"));
    }
    #[test]
    fn hevc_defaults_to_auto_and_accepts_signal() {
        assert_eq!(hevc_mode(None).unwrap(), ClinkHevcMode::Auto);
        assert_eq!(hevc_mode(Some("auto")).unwrap(), ClinkHevcMode::Auto);
        assert_eq!(hevc_mode(Some("off")).unwrap(), ClinkHevcMode::Off);
        assert_eq!(hevc_mode(Some("signal")).unwrap(), ClinkHevcMode::Signal);
        for value in ["", "on", "AUTO"] {
            assert_eq!(
                hevc_mode(Some(value)).unwrap_err().to_string(),
                "--clink-hevc must be auto, off or signal"
            );
        }
    }
    #[test]
    fn transcoding_defaults_to_auto_and_wraps_with_diagnostics() {
        assert!(transcode_enabled(None).unwrap());
        assert!(transcode_enabled(Some("auto")).unwrap());
        assert!(!transcode_enabled(Some("off")).unwrap());
        assert!(transcode_enabled(Some("on")).is_err());
        struct Inner;
        impl oxp_core::ProjectionOutput for Inner {
            fn id(&self) -> &str {
                "clink"
            }
            fn open(&self, _: Arc<dyn OutputHost>) -> oxp_core::Result<()> {
                Ok(())
            }
            fn on_session(&self, _: &SessionState, _: &SessionConfig) {}
            fn on_video_format(&self, _: u32, _: Option<Arc<VideoFormat>>) {}
            fn on_video_frame(&self, _: VideoFrame) {}
            fn close(&self) {}
        }
        let lines = Arc::new(Mutex::new(Vec::<String>::new()));
        let log = lines.clone();
        let output = with_transcoding(
            Arc::new(Inner),
            Arc::new(oxp_transcode_linux::Unavailable("test host".into())),
            Arc::new(move |m: &str| log.lock().unwrap().push(m.into())),
        );
        assert_eq!(output.id(), "clink");
        assert_eq!(
            *lines.lock().unwrap(),
            vec!["video transcoder unavailable: test host".to_owned()]
        );
    }
    #[test]
    fn unsupported_band_reason_passes_through_unchanged() {
        assert_eq!(
            link_event(oxp_link_linux::LinkEvent::Lost {
                reason: "head_unit_band_unsupported:5180".into(),
            }),
            json!({"type":"link","state":"lost","reason":"head_unit_band_unsupported:5180"})
        );
    }
    #[test]
    fn link_events_have_only_contract_fields() {
        use oxp_link_linux::LinkEvent;
        let head = json!({"address":"AA:BB:CC:DD:EE:FF","name":"Head unit"});
        assert_eq!(
            link_event(LinkEvent::Scanning),
            json!({"type":"link","state":"scanning"})
        );
        assert_eq!(
            link_event(LinkEvent::Connecting {
                address: "AA:BB:CC:DD:EE:FF".into(),
                name: "Head unit".into()
            }),
            json!({"type":"link","state":"connecting","headUnit":head})
        );
        assert_eq!(
            link_event(LinkEvent::Joined {
                address: "AA:BB:CC:DD:EE:FF".into(),
                name: "Head unit".into(),
                ssid: "DIRECT-unit".into(),
                interface: "wlan0".into(),
                addresses: ClinkAddresses::new([192, 168, 49, 1].into(), [192, 168, 49, 2].into()),
            }),
            json!({"type":"link","state":"joined","headUnit":head,"network":{"ssid":"DIRECT-unit","interface":"wlan0"},"headUnitIp":"192.168.49.1","localIp":"192.168.49.2"})
        );
        assert_eq!(
            link_event(LinkEvent::Lost {
                reason: "network_link_lost".into()
            }),
            json!({"type":"link","state":"lost","reason":"network_link_lost"})
        );
    }
    #[test]
    fn pairing_responses_require_the_current_request_and_explicit_approval() {
        for confirmation in [false, true] {
            let requests = Arc::new(PairingRequests::default());
            let r = requests.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            let worker = std::thread::spawn(move || {
                r.ask(
                    "car",
                    confirmation,
                    Duration::from_secs(2),
                    &AtomicBool::new(false),
                    |v| {
                        tx.send(v).unwrap();
                    },
                )
            });
            let request = rx.recv().unwrap();
            let id = request["request"].as_u64().unwrap();
            assert_eq!(request["confirmation"], confirmation);
            assert_eq!(
                requests.respond(&json!({"request":id+1,"action":"approve"})),
                Err("stale_request")
            );
            assert_eq!(requests.respond(&json!({"request":id,"action":if confirmation {"pin"} else {"approve"},"pin":"001234"})),Err("invalid_response"));
            assert_eq!(requests.respond(&json!({"request":id,"action":if confirmation {"approve"} else {"pin"},"pin":"001234"})),Ok(false));
            assert_eq!(
                worker.join().unwrap().as_deref(),
                Some(if confirmation { "approved" } else { "001234" })
            );
            assert_eq!(
                requests.respond(&json!({"request":id,"action":"approve"})),
                Err("stale_request")
            );
        }
    }
    #[test]
    fn cancelling_a_pin_file_prompt_wakes_it_and_keeps_late_responses_stale() {
        let mut pin = pin();
        let requests = Arc::new(PairingRequests::default());
        pin.requests = Some(requests.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        let start = Instant::now();
        let worker = std::thread::spawn(move || {
            pin.request_with("car", Duration::from_secs(120), |v| {
                tx.send(v).unwrap();
            })
        });
        let request = rx.recv().unwrap();
        assert_eq!(
            requests.respond(&json!({"request":request["request"],"action":"cancel"})),
            Ok(true)
        );
        assert!(worker.join().unwrap().is_none());
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(
            requests.respond(&json!({"request":request["request"],"action":"pin","pin":"001234"})),
            Err("stale_request")
        );
    }
    #[test]
    fn vehicle_event_preserves_origin_and_lossless_sensor_timestamp() {
        let event = vehicle_event(
            Origin {
                output_id: "clink".into(),
                client_id: "car".into(),
            },
            VehicleData::Gps {
                height: -12,
                latitude: 45,
                longitude: -123,
                antenna_state: u32::MAX,
                pdop: 9,
                speed: 72,
                heading: 180,
                satellites_used: 8,
                satellites_visible: 12,
                timestamp: u64::MAX,
            },
        );
        assert_eq!(event["type"], "clink-vehicle");
        assert_eq!(
            event["origin"],
            json!({"outputId":"clink","clientId":"car"})
        );
        assert_eq!(event["data"]["latitude"], 45);
        assert_eq!(event["data"]["timestamp"], u64::MAX.to_string());
    }
}
