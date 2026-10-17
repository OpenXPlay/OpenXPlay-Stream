// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use crate::{
    interfaces::ClinkPorts,
    model::{DecoderSupport, DisplayInfo},
    require, Result,
};
use std::collections::BTreeMap;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RtspMessage {
    pub line: String,
    pub headers: BTreeMap<String, String>,
    pub body: String,
}
impl RtspMessage {
    pub fn encode(&self) -> Result<Vec<u8>> {
        require(!self.line.contains(['\r', '\n']), "invalid_rtsp_line")?;
        let mut text = format!("{}\r\n", self.line);
        for (k, v) in &self.headers {
            require(
                !k.contains(['\r', '\n']) && !v.contains(['\r', '\n']),
                "invalid_rtsp_header",
            )?;
            let k = k.to_lowercase();
            if k != "content-length" && k != "content-type" {
                text.push_str(&format!("{k}: {v}\r\n"));
            }
        }
        if !self.body.is_empty() {
            text.push_str(&format!(
                "content-length: {}\r\ncontent-type: text/parameters\r\n",
                self.body.len()
            ));
        }
        text.push_str("\r\n");
        let mut out: Vec<u8> = text
            .chars()
            .map(|c| if c.is_ascii() { c as u8 } else { b'?' })
            .collect();
        out.extend_from_slice(self.body.as_bytes());
        Ok(out)
    }
}
#[derive(Default)]
pub struct RtspReader {
    buffer: Vec<u8>,
}
impl RtspReader {
    pub fn feed(&mut self, b: &[u8]) -> Result<Vec<RtspMessage>> {
        require(self.buffer.len() + b.len() <= 131072, "rtsp_buffer_limit")?;
        self.buffer.extend_from_slice(b);
        let mut messages = Vec::new();
        while !self.buffer.is_empty() {
            if self.buffer[0] == b'$' {
                if self.buffer.len() < 4 {
                    break;
                }
                let n = u16::from_be_bytes(self.buffer[2..4].try_into()?) as usize;
                if self.buffer.len() < n + 4 {
                    break;
                }
                self.buffer.drain(..n + 4);
                continue;
            }
            let end = self.buffer.windows(4).position(|b| b == b"\r\n\r\n");
            let end = match end {
                Some(n) => n,
                None => {
                    require(self.buffer.len() <= 8192, "rtsp_buffer_limit")?;
                    break;
                }
            };
            require(end <= 8192, "rtsp_buffer_limit")?;
            let text: String = self.buffer[..end]
                .iter()
                .map(|&b| if b < 128 { b as char } else { '\u{fffd}' })
                .collect();
            let mut lines = text.split("\r\n");
            let line = lines.next().unwrap().to_string();
            require(!line.is_empty(), "invalid_rtsp_line")?;
            let mut headers = BTreeMap::new();
            for l in lines {
                let (k, v) = l.split_once(':').ok_or("invalid_rtsp_header")?;
                require(!k.is_empty(), "invalid_rtsp_header")?;
                let key = k.trim().to_lowercase();
                require(
                    !key.is_empty() && !headers.contains_key(&key),
                    "duplicate_rtsp_header",
                )?;
                headers.insert(key, v.trim().to_string());
            }
            let n = headers
                .get("content-length")
                .map(|v| v.parse::<usize>())
                .transpose()?
                .unwrap_or(0);
            require(n <= 65536, "invalid_content_length")?;
            if self.buffer.len() < end + 4 + n {
                break;
            }
            headers.remove("content-length");
            headers.remove("content-type");
            messages.push(RtspMessage {
                line,
                headers,
                body: String::from_utf8_lossy(&self.buffer[end + 4..end + 4 + n]).into(),
            });
            self.buffer.drain(..end + 4 + n);
        }
        Ok(messages)
    }
}
pub fn parameters(body: &str) -> BTreeMap<String, String> {
    body.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| {
            let (k, v) = l.split_once(':').unwrap_or((l, ""));
            (k.trim().into(), v.trim().into())
        })
        .collect()
}
pub fn parameter_body(values: &BTreeMap<String, String>) -> String {
    values
        .iter()
        .map(|(k, v)| format!("{k}: {v}\r\n"))
        .collect()
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WfdMode {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub cea: u8,
    pub profile: u8,
    pub level: u8,
}
impl Default for WfdMode {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            fps: 30,
            cea: 5,
            profile: 1,
            level: 1,
        }
    }
}
pub struct WfdSource {
    address: String,
    ports: ClinkPorts,
    pub mode: WfdMode,
    pub decoders: Vec<DecoderSupport>,
    pub playing: bool,
    pub audio: bool,
    pub rtp_port: u16,
    setup: bool,
    keyframe: bool,
    sequence: u32,
    pending: BTreeMap<String, String>,
    session: String,
    advertised_rtp: String,
}
impl WfdSource {
    pub fn new(address: String, ports: ClinkPorts) -> Self {
        use rand::RngCore;
        let mut id = [0; 16];
        rand::rngs::OsRng.fill_bytes(&mut id);
        id[6] = (id[6] & 15) | 64;
        id[8] = (id[8] & 63) | 128;
        Self {
            address,
            mode: WfdMode::default(),
            decoders: Vec::new(),
            playing: false,
            audio: false,
            rtp_port: ports.rtp,
            setup: false,
            keyframe: false,
            sequence: 0,
            pending: BTreeMap::new(),
            session: id.iter().map(|v| format!("{v:02x}")).collect(),
            advertised_rtp: format!("RTP/AVP/TCP;unicast {} 0 mode=play", ports.rtp),
            ports,
        }
    }
    fn request(
        &mut self,
        method: &str,
        stage: &str,
        body: String,
        mut headers: BTreeMap<String, String>,
    ) -> RtspMessage {
        self.sequence += 1;
        let seq = self.sequence.to_string();
        self.pending.insert(seq.clone(), stage.into());
        headers.insert("cseq".into(), seq);
        RtspMessage {
            line: format!("{method} * RTSP/1.0"),
            headers,
            body,
        }
    }
    pub fn start(&mut self) -> RtspMessage {
        self.request(
            "OPTIONS",
            "options",
            String::new(),
            BTreeMap::from([("require".into(), "org.wfa.wfd1.0".into())]),
        )
    }
    pub fn take_keyframe(&mut self) -> bool {
        std::mem::take(&mut self.keyframe)
    }
    pub fn capabilities(&self, d: &DisplayInfo) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("wfd_video_formats".into(),format!("{:02x} 00 {:02x} {:02x} {:08x} 00000000 00000000 00 0000 0000 00 none none",self.mode.cea<<3,self.mode.profile,self.mode.level,1u64<<self.mode.cea)),
            ("wfd_audio_codecs".into(),if self.audio {"AAC 00000001 00"} else {"none"}.into()),("wfd_client_rtp_ports".into(),self.advertised_rtp.clone()),
            ("wfd_uibc_capability".into(),format!("input_category_list=GENERIC;generic_cap_list=Keyboard, MultiTouch;hidc_cap_list=none;port={};uibc_encrypted=true",self.ports.uibc)),
            ("wfd_uibc_setting".into(),"enable".into()),("wfd_content_protection".into(),"none".into()),("wfd_standby_resume_capability".into(),"none".into()),
            ("wfd_presentation_URL".into(),format!("rtsp://{}:{}/wfd1.0/streamid=0 none",self.address,self.ports.rtsp)),
            ("wfd_car_display_mode".into(),format!("mode=0;display={}:{};dpi={};fps={}",self.mode.width,self.mode.height,d.density_dpi,self.mode.fps))])
    }
    fn negotiate(&mut self, body: &str) -> Result<()> {
        let peer = parameters(body);
        self.audio = peer.get("wfd_audio_codecs").is_some_and(|s| {
            s.split(',').any(|entry| {
                let f: Vec<_> = entry.split_whitespace().collect();
                f.len() == 3
                    && f[0] == "AAC"
                    && i64::from_str_radix(f[1], 16).is_ok_and(|n| n & 1 != 0)
            })
        });
        let formats = peer
            .get("wfd_video_formats")
            .ok_or("projection_formats_required")?;
        let mut supported = Vec::new();
        let mut selected = None;
        for (i, entry) in formats.split(',').enumerate() {
            let fields: Vec<_> = entry.split_whitespace().collect();
            let o = if i == 0 { 2 } else { 0 };
            require(fields.len() >= o + 3, "projection_formats_required")?;
            let profiles = u32::from_str_radix(fields[o], 16)?;
            let levels = u32::from_str_radix(fields[o + 1], 16)?;
            let cea = u64::from_str_radix(fields[o + 2], 16)?;
            for profile in [1, 2] {
                for (bit, level) in [(1, 31), (2, 32), (4, 40), (8, 41), (16, 42)] {
                    if profiles & profile != 0 && levels & bit != 0 {
                        let decoder = DecoderSupport {
                            codec_string: format!(
                                "avc1.{}{level:02X}",
                                if profile == 1 { "42E0" } else { "6400" }
                            ),
                            hardware: true,
                        };
                        if !supported.contains(&decoder) {
                            supported.push(decoder);
                        }
                        if selected.is_none() && cea & (1 << 5) != 0 {
                            selected = Some(WfdMode {
                                profile: profile as u8,
                                level: bit as u8,
                                ..Default::default()
                            });
                        }
                    }
                }
            }
        }
        self.mode = selected.ok_or("projection_timing_mismatch")?;
        self.decoders = supported;
        if let Some(r) = peer.get("wfd_client_rtp_ports") {
            require(
                r.starts_with("RTP/AVP/TCP;unicast"),
                "tcp_transport_required",
            )?;
            self.advertised_rtp = r.clone();
        }
        Ok(())
    }
    pub fn handle(&mut self, m: &RtspMessage, display: &DisplayInfo) -> Result<Vec<RtspMessage>> {
        let seq = m.headers.get("cseq").ok_or("rtsp_sequence_required")?;
        let parts: Vec<_> = m.line.split(' ').collect();
        require(parts.len() >= 3, "invalid_rtsp_line")?;
        if parts[0].starts_with("RTSP/") {
            let stage = match self.pending.remove(seq) {
                Some(s) => s,
                None => return Ok(Vec::new()),
            };
            require(parts[1] == "200", "projection_negotiation_rejected")?;
            let r=match stage.as_str() {
                "options"=>Some(self.request("GET_PARAMETER","capabilities","wfd_video_formats\r\nwfd_audio_codecs\r\nwfd_client_rtp_ports\r\nwfd_uibc_capability\r\n".into(),BTreeMap::new())),
                "capabilities"=>{self.negotiate(&m.body)?;Some(self.request("SET_PARAMETER","selected",parameter_body(&self.capabilities(display)),BTreeMap::new()))},
                "selected"=>Some(self.request("SET_PARAMETER","trigger","wfd_trigger_method: SETUP\r\n".into(),BTreeMap::new())),_=>None};
            return Ok(r.into_iter().collect());
        }
        let mut headers = BTreeMap::from([("cseq".into(), seq.clone())]);
        let mut status = "200 OK";
        let mut body = String::new();
        match parts[0] {
            "OPTIONS" => {
                headers.insert("public".into(),"org.wfa.wfd1.0, OPTIONS, GET_PARAMETER, SET_PARAMETER, SETUP, PLAY, PAUSE, TEARDOWN".into());
            }
            "GET_PARAMETER" => {
                let wanted = parameters(&m.body);
                body = parameter_body(
                    &self
                        .capabilities(display)
                        .into_iter()
                        .filter(|(k, _)| wanted.contains_key(k))
                        .collect(),
                );
            }
            "SET_PARAMETER" => {
                if parameters(&m.body).contains_key("wfd_idr_request") {
                    self.keyframe = true;
                }
            }
            "SETUP" => {
                let transport = m.headers.get("transport").map(String::as_str).unwrap_or("");
                let port = transport.split(';').find_map(|p| {
                    p.strip_prefix("client_port=").and_then(|s| {
                        let mut p = s.split('-');
                        let first = p.next()?;
                        if first.is_empty() || !first.bytes().all(|b| b.is_ascii_digit()) {
                            return None;
                        }
                        if let Some(second) = p.next() {
                            if second.is_empty()
                                || !second.bytes().all(|b| b.is_ascii_digit())
                                || p.next().is_some()
                            {
                                return None;
                            }
                        }
                        Some(first)
                    })
                });
                let port = port.filter(|_| transport.starts_with("RTP/AVP/TCP;"));
                if let Some(port) = port {
                    self.rtp_port = port.parse()?;
                    require(self.rtp_port > 0, "invalid_rtp_port")?;
                    headers.insert(
                        "transport".into(),
                        format!(
                            "{transport};server_port={}-{}",
                            self.ports.rtp,
                            self.ports.rtp.saturating_add(1)
                        ),
                    );
                    headers.insert("session".into(), self.session.clone());
                    self.setup = true;
                } else {
                    status = "461 Unsupported Transport";
                }
            }
            "PLAY" | "PAUSE" | "TEARDOWN" => {
                if !self.setup
                    || m.headers.get("session").and_then(|s| s.split(';').next())
                        != Some(self.session.as_str())
                {
                    status = "454 Session Not Found";
                } else {
                    self.playing = parts[0] == "PLAY";
                    headers.insert("session".into(), self.session.clone());
                    if parts[0] == "TEARDOWN" {
                        self.setup = false;
                    }
                    if self.playing {
                        self.keyframe = true;
                    }
                }
            }
            _ => status = "405 Method Not Allowed",
        }
        Ok(vec![RtspMessage {
            line: format!("RTSP/1.0 {status}"),
            headers,
            body,
        }])
    }
}
