// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

//! Optional plugin transport. Its locks and sockets are independent of media delivery.
use crate::{http, Runtime, Transport};
use oxp_core::ContextUpdate;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tungstenite::{
    protocol::{Role, WebSocketConfig},
    Message, WebSocket,
};

pub const SPEC: &str = "oxp-web-plugin/0";
pub const TOPICS: [&str; 6] = [
    "session",
    "nowPlaying",
    "call",
    "activity",
    "route",
    "audioSource",
];
const LIMIT: usize = 65536;
#[derive(Clone)]
pub struct PluginsConfig {
    pub root: PathBuf,
    pub enabled: Vec<String>,
    /// Relay-provider topics available from this relay. Defaults to all registry relay topics.
    pub topics: Vec<String>,
    /// None selects volatile storage; otherwise writes below state/plugins/storage.
    pub state_dir: Option<PathBuf>,
}
impl Default for PluginsConfig {
    fn default() -> Self {
        Self {
            root: PathBuf::new(),
            enabled: Vec::new(),
            topics: TOPICS.into_iter().map(str::to_owned).collect(),
            state_dir: None,
        }
    }
}
pub fn valid_id(id: &str) -> bool {
    let parts: Vec<_> = id.split('.').collect();
    id.len() <= 64
        && parts.len() > 1
        && parts.iter().enumerate().all(|(i, p)| {
            !p.is_empty()
                && p.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || (i > 0 && b == b'-'))
        })
}
pub fn mime(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "webp" => "image/webp",
        "jpg" => "image/jpeg",
        "woff2" => "font/woff2",
        "wasm" => "application/wasm",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "txt" => "text/plain",
        _ => return None,
    })
}
pub fn safe_path(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._/-".contains(&b))
        && s.split('/').all(|p| !p.is_empty() && !p.starts_with('.'))
        && mime(s.rsplit('.').next().unwrap_or("")).is_some()
}
pub(crate) fn origin(secure: bool, authority: &str) -> Option<String> {
    if !authority
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"._:-[]".contains(&b))
    {
        return None;
    }
    let authority: tungstenite::http::uri::Authority = authority.parse().ok()?;
    let host = authority.host().to_ascii_lowercase();
    if host.starts_with('[') {
        host.trim_matches(['[', ']'])
            .parse::<std::net::Ipv6Addr>()
            .ok()?;
    }
    let port = authority.port_u16();
    if authority.port().is_some() && port.is_none() {
        return None;
    }
    let suffix = port
        .filter(|p| *p != if secure { 443 } else { 80 })
        .map(|p| format!(":{p}"))
        .unwrap_or_default();
    Some(format!(
        "{}://{host}{suffix}",
        if secure { "https" } else { "http" }
    ))
}
pub fn headers(origin: &str, plugin: Option<&str>) -> BTreeMap<String, String> {
    let base = format!(
        "{origin}/plugins/{}/",
        plugin.map(|p| format!("p/{p}")).unwrap_or("host".into())
    );
    let guard = format!("{origin}/plugins/host/guard.js");
    let artwork = format!("{origin}/plugins/artwork/");
    let channel = format!("{}/plugins/channel", origin.replacen("http", "ws", 1));
    let (images, connect, frames) = if plugin.is_some() {
        (String::new(), "'none'".into(), "'none'".into())
    } else {
        (
            format!(" {artwork} {origin}/plugins/p/"),
            format!("{channel} {artwork}"),
            format!("{origin}/plugins/p/"),
        )
    };
    let csp = format!("default-src 'none'; script-src {base} {guard} 'wasm-unsafe-eval'; style-src {base} 'unsafe-inline'; img-src {base} data: blob:{images}; media-src {base} data: blob:; font-src {base} data:; worker-src {base} blob:; connect-src {connect}; frame-src {frames}; form-action 'none'; base-uri 'none'");
    [("X-Content-Type-Options","nosniff"), ("Referrer-Policy","no-referrer"), ("Access-Control-Allow-Origin","null"), ("Cross-Origin-Resource-Policy","same-origin"), ("Cache-Control","no-cache"), ("Permissions-Policy","camera=(), microphone=(), geolocation=(), usb=(), bluetooth=(), serial=(), hid=(), payment=()"), ("Content-Security-Policy", &csp)].into_iter().map(|(k,v)|(k.into(),v.into())).collect()
}
fn file(root: &Path, relative: &str) -> Option<PathBuf> {
    if !safe_path(relative) {
        return None;
    }
    let root = root.canonicalize().ok()?;
    let full = root.join(relative);
    if !std::fs::symlink_metadata(&full).ok()?.is_file() {
        return None;
    }
    let path = full.canonicalize().ok()?;
    (path.starts_with(&root) && path.is_file()).then_some(path)
}
struct Data {
    values: BTreeMap<String, Value>,
    writes: BTreeMap<String, Instant>,
    pending: BTreeSet<String>,
}
type LatestTopics = (u64, BTreeMap<String, (u64, Value)>);

pub(crate) struct Plugins {
    config: PluginsConfig,
    manifests: Vec<Value>,
    data: Mutex<Data>,
    latest: Mutex<LatestTopics>,
    connections: Mutex<VecDeque<Arc<AtomicBool>>>,
    receivers: AtomicUsize,
}
impl Plugins {
    pub fn new(mut config: PluginsConfig) -> oxp_wire::Result<Self> {
        if config.topics.iter().any(|t| !TOPICS.contains(&t.as_str())) {
            return Err("Plugin topics must be registry relay topics".into());
        }
        let mut seen = BTreeSet::new();
        config.topics.retain(|t| seen.insert(t.clone()));
        let latest = config
            .topics
            .iter()
            .map(|t| (t.clone(), (0, Value::Null)))
            .collect();
        let mut manifests = Vec::new();
        let mut values = BTreeMap::new();
        for id in &config.enabled {
            if !valid_id(id) || manifests.iter().any(|m: &Value| m["id"] == *id) {
                continue;
            }
            let manifest = file(&config.root, &format!("p/{id}/oxp-plugin.json"))
                .and_then(|p| std::fs::read(p).ok())
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
            let Some(m) = manifest.filter(|m| m["id"] == *id && valid_manifest(m)) else {
                continue;
            };
            if permission(&m, "storage") {
                if let Some(dir) = &config.state_dir {
                    if let Ok(b) =
                        std::fs::read(dir.join("plugins/storage").join(format!("{id}.json")))
                    {
                        if let Ok(v) = serde_json::from_slice::<Value>(&b) {
                            if v.is_object() && v.to_string().len() <= LIMIT {
                                values.insert(id.clone(), v);
                            }
                        }
                    }
                }
            }
            manifests.push(m);
        }
        let p = Self {
            config,
            manifests,
            data: Mutex::new(Data {
                values,
                writes: BTreeMap::new(),
                pending: BTreeSet::new(),
            }),
            latest: Mutex::new((0, latest)),
            connections: Mutex::new(VecDeque::new()),
            receivers: AtomicUsize::new(0),
        };
        if p.welcome().to_string().len() > LIMIT {
            return Err("Plugin catalog exceeds channel cap".into());
        }
        Ok(p)
    }
    fn welcome(&self) -> Value {
        json!({"type":"welcome","spec":SPEC,"plugins":self.manifests,"topics":self.config.topics,"storage":{"quotaBytes":LIMIT,"persistent":self.config.state_dir.is_some()}})
    }
    fn index(&self) -> Value {
        json!({"spec":SPEC,"plugins":self.manifests.iter().map(|m|json!({"id":m["id"],"version":m["version"],"manifest":format!("/plugins/p/{}/oxp-plugin.json",m["id"].as_str().unwrap())})).collect::<Vec<_>>()})
    }
    pub fn update(&self, update: ContextUpdate) {
        if let Ok(v) = update.to_json() {
            let topic = v["topic"].as_str().unwrap();
            if !self.config.topics.iter().any(|t| t == topic) {
                return;
            }
            // Validate the plugin topic contract before publishing a snapshot.
            let value = if valid_topic(topic, &v["value"]) {
                v["value"].clone()
            } else {
                Value::Null
            };
            let mut latest = self.latest.lock().unwrap();
            let seq = latest.0 + 1;
            if json!({"type":"state","topic":topic,"value":value,"seq":seq})
                .to_string()
                .len()
                <= LIMIT
            {
                latest.0 = seq;
                latest.1.insert(topic.into(), (seq, value));
            }
        }
    }
    fn storage(&self, m: &Value) -> Value {
        let error = |code| json!({"type":"error","id":m["id"],"code":code});
        let id = m["plugin"].as_str().unwrap();
        let Some(manifest) = self.manifests.iter().find(|p| p["id"] == id) else {
            return error("unknown-plugin");
        };
        if !permission(manifest, "storage") {
            return error("permission");
        }
        let mut data = self.data.lock().unwrap();
        if m["type"] == "storage-get" {
            let reply = json!({"type":"storage","id":m["id"],"value":data.values.get(id).unwrap_or(&Value::Null)});
            return if reply.to_string().len() <= LIMIT {
                reply
            } else {
                error("quota")
            };
        }
        let bytes = m["value"].to_string();
        if bytes.len() > LIMIT {
            return error("quota");
        }
        if data.pending.contains(id)
            || data
                .writes
                .get(id)
                .is_some_and(|t| t.elapsed() < Duration::from_secs(2))
        {
            return error("rate");
        }
        data.pending.insert(id.into());
        drop(data);
        // Disk I/O belongs only to this connection, never to a shared lock.
        let result = if let Some(state) = &self.config.state_dir {
            (|| -> std::io::Result<()> {
                let dir = state.join("plugins/storage");
                std::fs::create_dir_all(&dir)?;
                static NEXT_WRITE: AtomicUsize = AtomicUsize::new(0);
                let tmp = dir.join(format!(
                    "{id}.{}.{}.tmp",
                    std::process::id(),
                    NEXT_WRITE.fetch_add(1, Ordering::Relaxed)
                ));
                let mut f = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&tmp)?;
                let result = (|| {
                    f.write_all(bytes.as_bytes())?;
                    f.sync_all()?;
                    std::fs::rename(&tmp, dir.join(format!("{id}.json")))
                })();
                if result.is_err() {
                    let _ = std::fs::remove_file(tmp);
                }
                result
            })()
        } else {
            Ok(())
        };
        let mut data = self.data.lock().unwrap();
        data.pending.remove(id);
        if result.is_err() {
            return error("protocol");
        }
        data.writes.insert(id.into(), Instant::now());
        data.values.insert(id.into(), m["value"].clone());
        json!({"type":"storage-ok","id":m["id"]})
    }
    pub fn reserve_receiver(&self, max: usize) -> Option<ReceiverPermit<'_>> {
        self.receivers
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < max).then_some(n + 1)
            })
            .ok()?;
        Some(ReceiverPermit(&self.receivers))
    }
    pub fn reserve(&self) -> ChannelPermit<'_> {
        let replaced = Arc::new(AtomicBool::new(false));
        let mut connections = self.connections.lock().unwrap();
        if connections.len() == 4 {
            connections
                .pop_front()
                .unwrap()
                .store(true, Ordering::Release);
        }
        connections.push_back(replaced.clone());
        ChannelPermit {
            plugins: self,
            replaced,
        }
    }
    pub fn channel(
        &self,
        transport: Transport,
        run: &Runtime,
        permit: &ChannelPermit<'_>,
    ) -> oxp_wire::Result<()> {
        self.channel_with_timing(transport, run, permit, ChannelTiming::default())
    }
    fn channel_with_timing(
        &self,
        mut transport: Transport,
        run: &Runtime,
        permit: &ChannelPermit<'_>,
        timing: ChannelTiming,
    ) -> oxp_wire::Result<()> {
        transport.clear_deadline();
        transport.socket().set_nonblocking(false)?;
        transport.socket().set_write_timeout(Some(timing.write))?;
        let mut ws = WebSocket::from_raw_socket(
            ChannelIo {
                transport,
                activity: FrameActivity::new(),
                wake: Instant::now(),
                idle: timing.idle,
                write_failed: false,
            },
            Role::Server,
            Some(
                WebSocketConfig::default()
                    .write_buffer_size(0)
                    .max_message_size(Some(LIMIT))
                    .max_frame_size(Some(LIMIT))
                    .max_write_buffer_size(6 * LIMIT),
            ),
        );
        let mut session = Channel::default();
        let mut ping = Instant::now();
        let mut flush = Instant::now();
        while run.active.load(Ordering::Acquire) {
            if permit.replaced.load(Ordering::Acquire)
                || ws.get_ref().activity.received.elapsed() >= timing.idle
            {
                let reason = if permit.replaced.load(Ordering::Acquire) {
                    "replaced"
                } else {
                    "idle"
                };
                let _ = ws.close(Some(tungstenite::protocol::CloseFrame {
                    code: tungstenite::protocol::frame::coding::CloseCode::Away,
                    reason: reason.into(),
                }));
                return Ok(());
            }
            if ping.elapsed() >= timing.ping {
                ws.send(Message::Ping(Vec::new().into()))?;
                ping = Instant::now();
            }
            if flush.elapsed() >= Duration::from_millis(250) {
                for message in session.flush(self, Instant::now()) {
                    ws.send(Message::text(message.to_string()))?;
                }
                flush = Instant::now();
            }
            let wait = timing
                .ping
                .saturating_sub(ping.elapsed())
                .min(
                    timing
                        .idle
                        .saturating_sub(ws.get_ref().activity.received.elapsed()),
                )
                .min(Duration::from_millis(250).saturating_sub(flush.elapsed()))
                .max(Duration::from_millis(1));
            ws.get_mut().wake = Instant::now() + wait;
            let message = match ws.read() {
                Ok(message) => message,
                Err(tungstenite::Error::Io(e))
                    if !ws.get_ref().write_failed
                        && matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) =>
                {
                    continue
                }
                Err(e) => return Err(e.into()),
            };
            match message {
                Message::Text(raw) => {
                    for reply in session.receive(self, &raw) {
                        ws.send(Message::text(reply.to_string()))?;
                    }
                }
                Message::Close(_) => {
                    let _ = ws.flush();
                    return Ok(());
                }
                Message::Binary(_) => ws.send(Message::text(
                    json!({"type":"error","code":"protocol"}).to_string(),
                ))?,
                _ => {}
            }
            // Flush automatic pong replies; a failed write ends this connection.
            ws.flush()?;
        }
        Ok(())
    }
}
// Observe complete frames, including continuations hidden by tungstenite's message assembler.
// The absolute wake deadline also interrupts a peer trickling an incomplete frame.
struct ChannelIo {
    transport: Transport,
    activity: FrameActivity,
    wake: Instant,
    idle: Duration,
    write_failed: bool,
}
struct FrameActivity {
    received: Instant,
    header: Vec<u8>,
    remaining: u64,
}
impl FrameActivity {
    fn new() -> Self {
        Self {
            received: Instant::now(),
            header: Vec::with_capacity(14),
            remaining: 0,
        }
    }
    fn observe(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if self.remaining > 0 {
                self.remaining -= 1;
                if self.remaining == 0 {
                    self.received = Instant::now();
                }
                continue;
            }
            self.header.push(byte);
            if self.header.len() < 2 {
                continue;
            }
            let size = self.header[1] & 127;
            let extended = match size {
                126 => 2,
                127 => 8,
                _ => 0,
            };
            let needed = 2 + extended + if self.header[1] & 128 != 0 { 4 } else { 0 };
            if self.header.len() < needed {
                continue;
            }
            self.remaining = if extended == 0 {
                u64::from(size)
            } else {
                self.header[2..2 + extended]
                    .iter()
                    .fold(0u64, |n, b| (n << 8) | u64::from(*b))
            };
            self.header.clear();
            if self.remaining == 0 {
                self.received = Instant::now();
            }
        }
    }
}
impl Read for ChannelIo {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let end = self.wake.min(self.activity.received + self.idle);
        let wait = end
            .checked_duration_since(Instant::now())
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::TimedOut, "Channel deadline"))?;
        self.transport
            .socket()
            .set_read_timeout(Some(wait.max(Duration::from_millis(1))))?;
        let count = self.transport.read(bytes)?;
        self.activity.observe(&bytes[..count]);
        Ok(count)
    }
}
impl Write for ChannelIo {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let result = self.transport.write(bytes);
        self.write_failed |= result.is_err();
        result
    }
    fn flush(&mut self) -> std::io::Result<()> {
        let result = self.transport.flush();
        self.write_failed |= result.is_err();
        result
    }
}
#[derive(Clone, Copy)]
struct ChannelTiming {
    ping: Duration,
    idle: Duration,
    write: Duration,
}
impl Default for ChannelTiming {
    fn default() -> Self {
        Self {
            ping: Duration::from_secs(10),
            idle: Duration::from_secs(30),
            write: Duration::from_secs(2),
        }
    }
}
pub(crate) struct ChannelPermit<'a> {
    plugins: &'a Plugins,
    replaced: Arc<AtomicBool>,
}
impl Drop for ChannelPermit<'_> {
    fn drop(&mut self) {
        self.plugins
            .connections
            .lock()
            .unwrap()
            .retain(|c| !Arc::ptr_eq(c, &self.replaced));
    }
}
pub(crate) struct ReceiverPermit<'a>(&'a AtomicUsize);
impl Drop for ReceiverPermit<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(crate) fn serve(
    plugins: Option<&Plugins>,
    r: &http::Request,
    origin: &str,
    out: &mut impl Write,
) -> Result<(), u16> {
    let path = r.target.split('?').next().unwrap_or("");
    let plugin = path
        .strip_prefix("/plugins/p/")
        .and_then(|p| p.split_once('/'))
        .map(|(id, _)| id)
        .filter(|id| {
            id.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b".-".contains(&b))
        });
    let mut status = if plugins.is_some() && path == "/plugins/channel" {
        503
    } else {
        404
    };
    let mut body = Vec::new();
    let mut ty = "text/plain";
    if let Some(p) = plugins.filter(|_| matches!(r.method.as_str(), "GET" | "HEAD")) {
        if path == "/plugins/index.json" {
            status = 200;
            body = p.index().to_string().into_bytes();
            ty = "application/json";
        } else {
            let relative = if let Some(tail) = path.strip_prefix("/plugins/host/") {
                Some(format!("host/{tail}"))
            } else {
                plugin
                    .filter(|id| p.manifests.iter().any(|m| m["id"] == *id))
                    .map(|_| path[9..].to_owned())
            };
            if let Some(relative) = relative {
                if let Some(data) =
                    file(&p.config.root, &relative).and_then(|f| std::fs::read(f).ok())
                {
                    status = 200;
                    body = data;
                    ty = mime(relative.rsplit('.').next().unwrap_or("")).unwrap();
                }
            }
        }
    }
    write!(
        out,
        "HTTP/1.1 {status} {}\r\nContent-Type: {ty}\r\nContent-Length: {}\r\nConnection: close\r\n",
        match status {
            200 => "OK",
            503 => "Service Unavailable",
            _ => "Not Found",
        },
        body.len()
    )
    .map_err(|_| 400u16)?;
    for (k, v) in headers(origin, plugin) {
        write!(out, "{k}: {v}\r\n").map_err(|_| 400u16)?;
    }
    write!(out, "\r\n").map_err(|_| 400u16)?;
    if r.method != "HEAD" {
        out.write_all(&body).map_err(|_| 400u16)?;
    }
    out.flush().map_err(|_| 400)
}
fn permission(m: &Value, p: &str) -> bool {
    m["permissions"]
        .as_array()
        .is_some_and(|a| a.iter().any(|v| v == p))
}
fn text(v: &Value, max: usize) -> bool {
    v.as_str()
        .is_some_and(|s| !s.is_empty() && s.chars().count() <= max)
}
fn local_name(v: &Value) -> bool {
    v.as_str().is_some_and(|s| {
        !s.is_empty()
            && s.len() <= 64
            && s.bytes()
                .enumerate()
                .all(|(i, b)| b.is_ascii_lowercase() || b.is_ascii_digit() || (i > 0 && b == b'-'))
    })
}
fn localized(v: &Value) -> bool {
    v.as_object().is_some_and(|o| {
        text(&v["en"], 2048)
            && o.iter().all(|(k, v)| {
                let parts: Vec<_> = k.split('-').collect();
                !parts.is_empty()
                    && !parts[0].is_empty()
                    && parts[0].len() <= 8
                    && parts[0].bytes().all(|b| b.is_ascii_alphabetic())
                    && parts.iter().skip(1).all(|p| {
                        !p.is_empty()
                            && p.len() <= 8
                            && p.bytes().all(|b| b.is_ascii_alphanumeric())
                    })
                    && text(v, 2048)
            })
    })
}
fn semver(v: &Value) -> bool {
    let Some(s) = v.as_str().filter(|_| text(v, 256)) else {
        return false;
    };
    let (main, build) = s
        .split_once('+')
        .map(|(a, b)| (a, Some(b)))
        .unwrap_or((s, None));
    let (core, pre) = main
        .split_once('-')
        .map(|(a, b)| (a, Some(b)))
        .unwrap_or((main, None));
    let numeric = |s: &str| {
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && (s == "0" || !s.starts_with('0'))
    };
    let parts: Vec<_> = core.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|s| numeric(s))
        && [pre, build].iter().enumerate().all(|(i, part)| {
            part.is_none_or(|s| {
                s.split('.').all(|p| {
                    !p.is_empty()
                        && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                        && (i != 0 || !p.bytes().all(|b| b.is_ascii_digit()) || numeric(p))
                })
            })
        })
}
fn license(v: &Value) -> bool {
    let Some(s) = v.as_str().filter(|_| text(v, 256)) else {
        return false;
    };
    if !s
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b".+(): -".contains(&b))
    {
        return false;
    }
    let mut tokens = Vec::new();
    let mut start = 0;
    for (i, c) in s.char_indices() {
        if c == '(' || c == ')' || c == ' ' {
            if start < i {
                tokens.push(&s[start..i]);
            }
            if c != ' ' {
                tokens.push(&s[i..i + 1]);
            }
            start = i + 1;
        }
    }
    if start < s.len() {
        tokens.push(&s[start..]);
    }
    fn atom(t: &[&str], i: &mut usize) -> bool {
        if t.get(*i) == Some(&"(") {
            *i += 1;
            return expr(t, i) && {
                let ok = t.get(*i) == Some(&")");
                *i += 1;
                ok
            };
        }
        let identifier = |s: &&str| {
            !["AND", "OR", "WITH"].contains(s)
                && s.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b".+-:".contains(&b))
        };
        if !t.get(*i).is_some_and(identifier) {
            return false;
        }
        *i += 1;
        if t.get(*i) == Some(&"WITH") {
            *i += 1;
            if !t.get(*i).is_some_and(identifier) {
                return false;
            }
            *i += 1;
        }
        true
    }
    fn expr(t: &[&str], i: &mut usize) -> bool {
        if !atom(t, i) {
            return false;
        }
        while matches!(t.get(*i), Some(&"AND" | &"OR")) {
            *i += 1;
            if !atom(t, i) {
                return false;
            }
        }
        true
    }
    let mut i = 0;
    expr(&tokens, &mut i) && i == tokens.len()
}
pub fn valid_manifest(m: &Value) -> bool {
    let Some(permissions) = m["permissions"].as_array() else {
        return false;
    };
    let Some(surfaces) = m["surfaces"].as_array() else {
        return false;
    };
    let Some(actions) = m["quickActions"].as_array() else {
        return false;
    };
    let path = |v: &Value, ext: &[&str]| {
        v.as_str().is_some_and(|s| {
            text(v, 256)
                && s.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
                && safe_path(s)
                && ext.contains(&s.rsplit('.').next().unwrap_or(""))
        })
    };
    let mut ids = BTreeSet::new();
    let mut quick = BTreeSet::new();
    m.is_object()
        && m["spec"] == SPEC
        && m["id"].as_str().is_some_and(valid_id)
        && semver(&m["version"])
        && license(&m["license"])
        && localized(&m["name"])
        && localized(&m["description"])
        && path(&m["icon"], &["svg", "png"])
        && permissions.len() <= 7
        && permissions.iter().all(|v| {
            matches!(
                v.as_str(),
                Some(
                    "session"
                        | "nowPlaying"
                        | "nowPlaying.artwork"
                        | "call"
                        | "activity"
                        | "storage"
                        | "route"
                )
            )
        })
        && permissions
            .iter()
            .map(Value::to_string)
            .collect::<BTreeSet<_>>()
            .len()
            == permissions.len()
        && surfaces.len() <= 32
        && surfaces.iter().all(|s| {
            s.is_object()
                && local_name(&s["id"])
                && ids.insert(s["id"].as_str().unwrap())
                && matches!(
                    s["kind"].as_str(),
                    Some("companion" | "panel" | "ambient" | "fullscreen")
                )
                && path(&s["entry"], &["html"])
                && matches!(s["usage"].as_str(), Some("driving" | "parked"))
                && (s["kind"] != "fullscreen" || s["usage"] == "parked")
                && s.get("label").is_none_or(localized)
                && s.get("anchor").is_none_or(|a| {
                    matches!(
                        a.as_str(),
                        Some(
                            "top-left"
                                | "top-right"
                                | "bottom-left"
                                | "bottom-right"
                                | "left"
                                | "right"
                                | "top"
                                | "bottom"
                        )
                    )
                })
                && s.get("size").is_none_or(|z| {
                    z.is_object()
                        && ["w", "h"].iter().all(|k| {
                            z[k].as_f64().is_some_and(|n| {
                                n >= 1.0 && (s["kind"] != "companion" || n <= 160.0)
                            })
                        })
                })
        })
        && actions.len() <= 32
        && actions.iter().all(|a| {
            a.is_object()
                && local_name(&a["id"])
                && quick.insert(a["id"].as_str().unwrap())
                && a["icon"].as_str().is_some_and(|s| {
                    text(&a["icon"], 256)
                        && s.bytes().all(|b| {
                            b.is_ascii_lowercase() || b.is_ascii_digit() || b"._/-".contains(&b)
                        })
                        && s.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
                })
                && localized(&a["label"])
                && a.get("opens")
                    .is_none_or(|v| local_name(v) && ids.contains(v.as_str().unwrap()))
        })
}
pub fn valid_topic(topic: &str, v: &Value) -> bool {
    if v.is_null() {
        return true;
    }
    if !v.is_object() {
        return false;
    }
    match topic {
        "route" | "audioSource" => {
            ContextUpdate::from_json(&json!({"topic":topic,"value":v})).is_ok()
        }
        "session" => matches!(v["carplay"].as_str(), Some("connected" | "disconnected")),
        "call" => matches!(
            v["state"].as_str(),
            Some("idle" | "ringing" | "dialing" | "active" | "held" | "ending")
        ),
        "activity" => ["siri", "phoneCall", "navigation"]
            .iter()
            .all(|k| v[k].is_boolean()),
        "nowPlaying" => {
            ["title", "artist", "album"]
                .iter()
                .all(|k| v[k].is_string())
                && ["durationMs", "elapsedMs", "rate"]
                    .iter()
                    .all(|k| v[k].as_f64().is_some_and(|n| n >= 0.0))
                && v["playing"].is_boolean()
                && v.get("artwork").is_none_or(|a| {
                    a.is_object()
                        && a["url"]
                            .as_str()
                            .and_then(|s| s.strip_prefix("/plugins/artwork/"))
                            .is_some_and(|s| {
                                s.len() == 64
                                    && s.bytes()
                                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                            })
                        && matches!(
                            a["type"].as_str(),
                            Some("image/png" | "image/jpeg" | "image/webp")
                        )
                })
        }
        _ => false,
    }
}
pub fn valid_channel(m: &Value) -> bool {
    if !m.is_object() || !text(&m["type"], usize::MAX) {
        return false;
    }
    match m["type"].as_str().unwrap() {
        "hello" => m["spec"] == SPEC && text(&m["host"], 256),
        "subscribe" => m["topics"]
            .as_array()
            .is_some_and(|a| a.iter().all(Value::is_string)),
        "storage-get" | "storage-set" => {
            text(&m["id"], 256)
                && m["plugin"].as_str().is_some_and(valid_id)
                && (m["type"] != "storage-set" || m["value"].is_object())
        }
        "welcome" => {
            m["spec"] == SPEC
                && m["plugins"]
                    .as_array()
                    .is_some_and(|a| a.iter().all(valid_manifest))
                && m["topics"].as_array().is_some_and(|a| {
                    a.iter()
                        .all(|t| t.as_str().is_some_and(|t| TOPICS.contains(&t)))
                })
                && m["storage"]["quotaBytes"] == LIMIT
                && m["storage"]["persistent"].is_boolean()
        }
        "state" => {
            m["topic"].as_str().is_some_and(|t| TOPICS.contains(&t))
                && m["seq"]
                    .as_u64()
                    .is_some_and(|n| n <= 9_007_199_254_740_991)
                && m.get("value")
                    .is_some_and(|v| valid_topic(m["topic"].as_str().unwrap(), v))
        }
        "storage" => {
            text(&m["id"], 256) && (m["value"].is_object() || m.get("value") == Some(&Value::Null))
        }
        "storage-ok" => text(&m["id"], 256),
        "error" => {
            matches!(
                m["code"].as_str(),
                Some("quota" | "rate" | "unknown-plugin" | "permission" | "protocol")
            ) && m.get("id").is_none_or(|v| text(v, 256))
        }
        _ => true,
    }
}
#[derive(Default)]
struct Channel {
    ready: bool,
    topics: Vec<String>,
    pending: BTreeMap<String, u64>,
    sent: BTreeMap<String, (u64, Instant)>,
}
impl Channel {
    fn receive(&mut self, p: &Plugins, raw: &str) -> Vec<Value> {
        let error = |id: Option<&Value>| {
            let mut e = json!({"type":"error","code":"protocol"});
            if let Some(id) = id.filter(|id| text(id, 256)) {
                e["id"] = id.clone();
            }
            vec![e]
        };
        let Ok(m) = serde_json::from_str::<Value>(raw) else {
            return error(None);
        };
        if raw.len() > LIMIT || !valid_channel(&m) {
            return error(None);
        }
        let t = m["type"].as_str().unwrap();
        if ![
            "hello",
            "subscribe",
            "storage-get",
            "storage-set",
            "welcome",
            "state",
            "storage",
            "storage-ok",
            "error",
        ]
        .contains(&t)
        {
            return vec![];
        }
        if t == "hello" && !self.ready {
            self.ready = true;
            return vec![p.welcome()];
        }
        if !self.ready {
            return error(m.get("id"));
        }
        match t {
            "subscribe" => {
                let mut seen = BTreeSet::new();
                self.topics = m["topics"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().to_owned())
                    .filter(|topic| TOPICS.contains(&topic.as_str()) && seen.insert(topic.clone()))
                    .collect();
                let seq = p.latest.lock().unwrap().0;
                self.pending = self
                    .topics
                    .iter()
                    .map(|topic| (topic.clone(), seq))
                    .collect();
                self.flush(p, Instant::now())
            }
            "storage-get" | "storage-set" => vec![p.storage(&m)],
            _ => error(m.get("id")),
        }
    }
    fn flush(&mut self, p: &Plugins, now: Instant) -> Vec<Value> {
        let latest = p.latest.lock().unwrap();
        self.topics
            .iter()
            .filter_map(|topic| {
                let (topic_seq, value) = latest.1.get(topic)?;
                let seq = (*topic_seq).max(self.pending.get(topic).copied().unwrap_or(0));
                if self.sent.get(topic).is_some_and(|(s, t)| {
                    (*s >= seq && !self.pending.contains_key(topic))
                        || now.duration_since(*t) < Duration::from_millis(250)
                }) {
                    return None;
                }
                self.sent.insert(topic.clone(), (seq, now));
                self.pending.remove(topic);
                Some(json!({"type":"state","topic":topic,"value":value,"seq":seq}))
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "plugins_tests.rs"]
mod tests;
