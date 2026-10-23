// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-22

//! Bounded, asynchronous Carlink trace and MPEG-TS recording.
use oxp_clink::trace::{ClinkMediaTap, ClinkTrace, ClinkTraceEvent};
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufWriter, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

type Callback = Arc<dyn Fn(Value) + Send + Sync>;
const TRACE_LIMIT: u64 = 8 * 1024 * 1024;
const CAPTURE_LIMIT: u64 = 64 * 1024 * 1024;
// Buffer ~2.7 s at 8 Mbit/s because trace sync_data can stall for hundreds of ms on eMMC.
const CAPTURE_QUEUE: usize = 2048;

struct Shared {
    dropped: AtomicU64,
    capturing: AtomicBool,
    overflow: AtomicBool,
    status: Mutex<Value>,
}
enum Command {
    Event(ClinkTraceEvent),
    Clear,
    Start(Duration),
    Stop,
    Shutdown,
    #[cfg(test)]
    Barrier(mpsc::Sender<()>),
}

/// Disk work and status callbacks run exclusively on the writer thread.
pub struct TraceRecorder {
    sender: SyncSender<Command>,
    capture: Arc<MediaCapture>,
    shared: Arc<Shared>,
    worker: Mutex<Option<JoinHandle<()>>>,
}
impl TraceRecorder {
    pub fn new(directory: impl Into<PathBuf>, callback: Callback) -> Self {
        Self::with_limits(directory.into(), callback, TRACE_LIMIT, CAPTURE_LIMIT)
    }
    fn with_limits(
        directory: PathBuf,
        callback: Callback,
        trace_limit: u64,
        capture_limit: u64,
    ) -> Self {
        let (sender, receiver) = mpsc::sync_channel(8192);
        let (packets, packet_receiver) = mpsc::sync_channel(CAPTURE_QUEUE);
        let shared = Arc::new(Shared {
            dropped: AtomicU64::new(0),
            capturing: AtomicBool::new(false),
            overflow: AtomicBool::new(false),
            status: Mutex::new(Value::Null),
        });
        let capture = Arc::new(MediaCapture {
            sender: sender.clone(),
            packets,
            shared: shared.clone(),
        });
        let state = shared.clone();
        let worker = thread::spawn(move || {
            Writer::new(directory, callback, state, trace_limit, capture_limit)
                .run(receiver, packet_receiver);
        });
        Self {
            sender,
            capture,
            shared,
            worker: Mutex::new(Some(worker)),
        }
    }
    pub fn capture(&self) -> Arc<MediaCapture> {
        self.capture.clone()
    }
    /// Controls may wait for queue capacity; session callbacks never do.
    pub fn clear(&self) {
        let _ = self.sender.send(Command::Clear);
    }
    pub fn status(&self) -> Value {
        let mut status = self
            .shared
            .status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if status.is_object() {
            status["dropped"] = self.shared.dropped.load(Ordering::Relaxed).into();
        }
        status
    }
    pub fn shutdown(&self) {
        let mut worker = self.worker.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(worker) = worker.take() {
            let _ = self.sender.send(Command::Shutdown);
            let _ = worker.join();
        }
    }
    #[cfg(test)]
    fn barrier(&self) {
        let (tx, rx) = mpsc::channel();
        self.sender.send(Command::Barrier(tx)).unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
    }
}
impl ClinkTrace for TraceRecorder {
    fn record(&self, event: ClinkTraceEvent) {
        if self.sender.try_send(Command::Event(event)).is_err() {
            self.shared.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}
impl Drop for TraceRecorder {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Only constructed with an enabled trace recorder.
pub struct MediaCapture {
    sender: SyncSender<Command>,
    packets: SyncSender<Vec<u8>>,
    shared: Arc<Shared>,
}
impl MediaCapture {
    pub fn start(&self, seconds: u64) {
        if (1..=300).contains(&seconds) {
            let _ = self
                .sender
                .send(Command::Start(Duration::from_secs(seconds)));
        }
    }
    pub fn stop(&self) {
        let _ = self.sender.send(Command::Stop);
    }
}
impl ClinkMediaTap for MediaCapture {
    fn rtp(&self, packet: &[u8]) {
        if packet.len() <= 12 || !self.shared.capturing.load(Ordering::Acquire) {
            return;
        }
        if let Err(TrySendError::Full(_)) = self.packets.try_send(packet[12..].to_vec()) {
            self.shared.overflow.store(true, Ordering::Release);
            self.shared.capturing.store(false, Ordering::Release);
        }
    }
}

struct Capture {
    writer: BufWriter<File>,
    path: PathBuf,
    deadline: Instant,
}
struct Writer {
    directory: PathBuf,
    callback: Callback,
    shared: Arc<Shared>,
    trace: Option<BufWriter<File>>,
    capture: Option<Capture>,
    status: Value,
    run: u64,
    started_ms: u128,
    part: u64,
    trace_limit: u64,
    capture_limit: u64,
    last_sync: Instant,
    dirty: bool,
    last_status: Instant,
    published: Value,
    capture_number: u64,
}
impl Writer {
    fn new(
        directory: PathBuf,
        callback: Callback,
        shared: Arc<Shared>,
        trace_limit: u64,
        capture_limit: u64,
    ) -> Self {
        let status = json!({
            "type": "clink-trace", "directory": directory.to_string_lossy(),
            "file": null, "bytes": 0, "lines": 0, "dropped": 0, "failure": null,
            "capture": {"state": "idle", "file": null, "bytes": 0, "reason": null}
        });
        Self {
            directory,
            callback,
            shared,
            trace: None,
            capture: None,
            status,
            run: 0,
            started_ms: unix_ms(),
            part: 0,
            trace_limit,
            capture_limit,
            last_sync: Instant::now(),
            dirty: false,
            last_status: Instant::now(),
            published: Value::Null,
            capture_number: 0,
        }
    }
    fn run(mut self, receiver: Receiver<Command>, packets: Receiver<Vec<u8>>) {
        if let Err(e) = self.initialize() {
            self.fail(e);
        }
        self.publish(true);
        loop {
            let timeout = Duration::from_millis(if self.capture.is_some() { 20 } else { 250 });
            let command = match receiver.recv_timeout(timeout) {
                Ok(c) => Some(c),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            let mut shutdown = false;
            if let Some(c) = command {
                shutdown = self.command(c, &packets);
            }
            // Bound each batch so capture deadlines and durability are checked under load.
            if !shutdown {
                for _ in 0..255 {
                    match receiver.try_recv() {
                        Ok(c) => {
                            if self.command(c, &packets) {
                                shutdown = true;
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
            self.drain_capture(&packets);
            if let Some(w) = self.trace.as_mut() {
                if let Err(e) = w.flush() {
                    self.fail(e);
                }
            }
            if self.dirty && self.last_sync.elapsed() >= Duration::from_secs(2) {
                if let Err(e) = self.sync_trace() {
                    self.fail(e);
                }
            }
            self.publish(false);
            if shutdown {
                break;
            }
        }
        self.finish_capture("stop");
        if let Err(e) = self.sync_trace() {
            self.fail(e);
        }
        self.publish(true);
    }
    fn initialize(&mut self) -> io::Result<()> {
        directory(&self.directory)?;
        let largest = matching(&self.directory, "", "")?
            .iter()
            .filter_map(|p| recording_order(p).map(|(run, _)| run))
            .max()
            .unwrap_or(0);
        self.run = next_number(largest)?;
        for path in matching(&self.directory, "media-", ".ts.part")? {
            fs::rename(&path, path.with_extension(""))?;
        }
        retain(&self.directory, "trace-", ".jsonl", 8)?;
        retain(&self.directory, "media-", ".ts", 2)
    }
    fn command(&mut self, c: Command, packets: &Receiver<Vec<u8>>) -> bool {
        match c {
            Command::Event(e) => {
                if self.status["failure"].is_null() {
                    if let Err(e) = self.write_event(e) {
                        self.fail(e);
                        self.shared.dropped.fetch_add(1, Ordering::Relaxed);
                    }
                } else {
                    self.shared.dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
            Command::Clear => {
                self.finish_capture("stop");
                let _ = self.sync_trace();
                self.trace = None;
                self.dirty = false;
                self.status["file"] = Value::Null;
                self.status["bytes"] = 0.into();
                self.status["lines"] = 0.into();
                self.status["failure"] = Value::Null;
                self.shared.dropped.store(0, Ordering::Relaxed);
                self.status["capture"] =
                    json!({"state":"idle","file":null,"bytes":0,"reason":null});
                let result = directory(&self.directory)
                    .and_then(|_| remove_matching(&self.directory, "trace-", ".jsonl"))
                    .and_then(|_| remove_matching(&self.directory, "media-", ".ts"))
                    .and_then(|_| remove_matching(&self.directory, "media-", ".ts.part"));
                if let Err(e) = result {
                    self.fail(e);
                }
                self.publish(false);
            }
            Command::Start(duration) => self.start_capture(duration, packets),
            Command::Stop => {
                self.drain_capture(packets);
                self.finish_capture("stop");
            }
            Command::Shutdown => return true,
            #[cfg(test)]
            Command::Barrier(done) => {
                self.drain_capture(packets);
                if let Some(w) = self.trace.as_mut() {
                    w.flush().unwrap();
                }
                self.publish(true);
                let _ = done.send(());
            }
        }
        false
    }
    fn write_event(&mut self, e: ClinkTraceEvent) -> io::Result<()> {
        let mut line = serde_json::to_vec(&e.to_json())?;
        line.push(b'\n');
        let bytes = self.status["bytes"].as_u64().unwrap();
        if self.trace.is_none() || (bytes > 0 && bytes + line.len() as u64 > self.trace_limit) {
            self.sync_trace()?;
            self.trace = None;
            directory(&self.directory)?;
            if self.run == 0 {
                self.initialize()?;
            }
            self.part = next_number(self.part)?;
            let name = format!(
                "trace-{:08}-{}-{:08}.jsonl",
                self.run, self.started_ms, self.part
            );
            self.trace = Some(BufWriter::new(create(&self.directory.join(&name))?));
            self.status["file"] = name.into();
            self.status["bytes"] = 0.into();
            retain_except(
                &self.directory,
                "trace-",
                ".jsonl",
                8,
                Some(&self.directory.join(self.status["file"].as_str().unwrap())),
            )?;
        }
        self.trace.as_mut().unwrap().write_all(&line)?;
        self.status["bytes"] = (self.status["bytes"].as_u64().unwrap() + line.len() as u64).into();
        self.status["lines"] = (self.status["lines"].as_u64().unwrap() + 1).into();
        self.dirty = true;
        Ok(())
    }
    fn sync_trace(&mut self) -> io::Result<()> {
        if let Some(w) = self.trace.as_mut() {
            w.flush()?;
            if self.dirty {
                w.get_ref().sync_data()?;
            }
        }
        self.dirty = false;
        self.last_sync = Instant::now();
        Ok(())
    }
    fn fail(&mut self, e: io::Error) {
        // ErrorKind is a stable, short Rust substitute for an exception class.
        self.status["failure"] = format!("{:?}", e.kind()).into();
        self.trace = None;
        self.dirty = false;
    }
    fn start_capture(&mut self, duration: Duration, packets: &Receiver<Vec<u8>>) {
        if self.capture.is_some() {
            return;
        }
        if self.run == 0 {
            if let Err(e) = self.initialize() {
                self.fail(e);
                self.status["capture"] =
                    json!({"state":"failed","file":null,"bytes":0,"reason":"error"});
                self.publish(true);
                return;
            }
        }
        while packets.try_recv().is_ok() {}
        self.shared.overflow.store(false, Ordering::Release);
        let number = next_number(self.capture_number);
        let name = format!(
            "media-{:08}-{:08}-{}.ts.part",
            self.run,
            number.as_ref().copied().unwrap_or_default(),
            unix_ms()
        );
        let path = self.directory.join(&name);
        self.status["capture"] = json!({"state":"capturing","file":name,"bytes":0,"reason":null});
        match number.and_then(|number| {
            self.capture_number = number;
            directory(&self.directory).and_then(|_| create(&path))
        }) {
            Ok(file) => {
                self.capture = Some(Capture {
                    writer: BufWriter::new(file),
                    path,
                    deadline: Instant::now() + duration,
                });
                self.shared.capturing.store(true, Ordering::Release);
            }
            Err(_) => {
                self.status["capture"]["state"] = "failed".into();
                self.status["capture"]["reason"] = "error".into();
                self.status["capture"]["file"] = Value::Null;
            }
        }
        self.publish(true);
    }
    fn drain_capture(&mut self, packets: &Receiver<Vec<u8>>) {
        if self.capture.is_none() {
            return;
        }
        if self.shared.overflow.load(Ordering::Acquire) {
            self.finish_capture("overflow");
            return;
        }
        if self.capture.as_ref().unwrap().deadline <= Instant::now() {
            self.finish_capture("deadline");
            return;
        }
        for _ in 0..256 {
            let Ok(payload) = packets.try_recv() else {
                break;
            };
            let bytes = self.status["capture"]["bytes"].as_u64().unwrap();
            if bytes + payload.len() as u64 > self.capture_limit {
                self.finish_capture("limit");
                return;
            }
            if self
                .capture
                .as_mut()
                .unwrap()
                .writer
                .write_all(&payload)
                .is_err()
            {
                self.finish_capture("error");
                return;
            }
            self.status["capture"]["bytes"] = (bytes + payload.len() as u64).into();
            if bytes + payload.len() as u64 == self.capture_limit {
                self.finish_capture("limit");
                return;
            }
        }
    }
    fn finish_capture(&mut self, reason: &str) {
        let Some(mut c) = self.capture.take() else {
            return;
        };
        self.shared.capturing.store(false, Ordering::Release);
        let finished = c.path.with_extension("");
        let result = c
            .writer
            .flush()
            .and_then(|_| c.writer.get_ref().sync_data());
        drop(c.writer);
        let renamed = fs::rename(&c.path, &finished)
            .and_then(|_| retain(&self.directory, "media-", ".ts", 2));
        let result = result.and(renamed);
        let failed = reason == "error" || result.is_err();
        self.status["capture"]["state"] = if failed { "failed" } else { "idle" }.into();
        self.status["capture"]["reason"] = if failed { "error" } else { reason }.into();
        if finished.exists() {
            self.status["capture"]["file"] = finished
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
                .into();
        }
        self.publish(true);
    }
    fn publish(&mut self, force: bool) {
        self.status["dropped"] = self.shared.dropped.load(Ordering::Relaxed).into();
        *self.shared.status.lock().unwrap_or_else(|e| e.into_inner()) = self.status.clone();
        if force
            || (self.last_status.elapsed() >= Duration::from_secs(1)
                && self.published != self.status)
        {
            (self.callback)(self.status.clone());
            self.published = self.status.clone();
            self.last_status = Instant::now();
        }
    }
}
fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
fn directory(path: &Path) -> io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}
fn create(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}
fn matching(directory: &Path, prefix: &str, suffix: &str) -> io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for e in fs::read_dir(directory)? {
        let e = e?;
        let n = e.file_name();
        let n = n.to_string_lossy();
        if n.starts_with(prefix) && n.ends_with(suffix) && e.file_type()?.is_file() {
            paths.push(e.path());
        }
    }
    Ok(paths)
}
fn remove_matching(directory: &Path, prefix: &str, suffix: &str) -> io::Result<()> {
    for p in matching(directory, prefix, suffix)? {
        fs::remove_file(p)?;
    }
    Ok(())
}
// Strict parsing keeps unrelated and legacy names out of retention and run numbering.
fn recording_order(path: &Path) -> Option<(u64, u64)> {
    let name = path.file_name()?.to_str()?;
    let (body, media) = if let Some(body) = name.strip_prefix("trace-") {
        (body.strip_suffix(".jsonl")?, false)
    } else {
        let body = name.strip_prefix("media-")?;
        (
            body.strip_suffix(".ts.part")
                .or_else(|| body.strip_suffix(".ts"))?,
            true,
        )
    };
    let fields: Vec<_> = body.split('-').collect();
    if fields.len() != 3 {
        return None;
    }
    let (run, index, timestamp) = (
        fields[0],
        fields[if media { 1 } else { 2 }],
        fields[if media { 2 } else { 1 }],
    );
    if run.len() != 8
        || index.len() != 8
        || timestamp.is_empty()
        || !fields.iter().all(|s| s.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    timestamp.parse::<u128>().ok()?;
    Some((run.parse().ok()?, index.parse().ok()?))
}
fn next_number(number: u64) -> io::Result<u64> {
    if number >= 99_999_999 {
        return Err(io::Error::other("recording number exhausted"));
    }
    Ok(number + 1)
}
fn retain(directory: &Path, prefix: &str, suffix: &str, keep: usize) -> io::Result<()> {
    retain_except(directory, prefix, suffix, keep, None)
}
fn retain_except(
    directory: &Path,
    prefix: &str,
    suffix: &str,
    keep: usize,
    protected: Option<&Path>,
) -> io::Result<()> {
    let mut files = matching(directory, prefix, suffix)?
        .into_iter()
        .filter_map(|p| recording_order(&p).map(|order| (order, p)))
        .collect::<Vec<_>>();
    files.sort();
    let remove = files.len().saturating_sub(keep);
    for (_, p) in files
        .into_iter()
        .filter(|(_, p)| protected != Some(p.as_path()))
        .take(remove)
    {
        fs::remove_file(p)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    static TEST_ID: AtomicU64 = AtomicU64::new(0);
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "oxp-trace-{}-{}-{}",
                std::process::id(),
                unix_ms(),
                TEST_ID.fetch_add(1, Ordering::Relaxed)
            ));
            Self(p)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
            let _ = fs::remove_file(&self.0);
        }
    }
    fn event(n: u64) -> ClinkTraceEvent {
        ClinkTraceEvent::Link {
            at_nanos: n,
            session: 7,
            stage: "stage".into(),
            fields: BTreeMap::new(),
        }
    }
    fn recorder(p: &Path, trace: u64, media: u64) -> TraceRecorder {
        TraceRecorder::with_limits(p.to_owned(), Arc::new(|_| {}), trace, media)
    }
    fn wait(rec: &TraceRecorder, predicate: impl Fn(&Value) -> bool) -> Value {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            let s = rec.status();
            if predicate(&s) {
                return s;
            }
            assert!(Instant::now() < until, "status: {s}");
            thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn rotation_keep_eight_clear_and_private_modes() {
        let dir = Temp::new();
        let r = recorder(&dir.0, 160, 1024);
        for n in 0..30 {
            r.record(event(n));
        }
        r.barrier();
        let paths = matching(&dir.0, "trace-", ".jsonl").unwrap();
        assert_eq!(paths.len(), 8);
        assert_eq!(
            dir.0.metadata().unwrap().permissions().mode() & 0o777,
            0o700
        );
        for p in paths {
            assert!(p.metadata().unwrap().len() <= 160);
            assert_eq!(p.metadata().unwrap().permissions().mode() & 0o777, 0o600);
            for line in fs::read_to_string(p).unwrap().lines() {
                assert_eq!(serde_json::from_str::<Value>(line).unwrap()["type"], "Link");
            }
        }
        fs::write(dir.0.join("media-00000001-00000001-0.ts"), b"finished").unwrap();
        fs::write(dir.0.join("unrelated"), b"keep").unwrap();
        r.clear();
        r.barrier();
        assert!(matching(&dir.0, "trace-", ".jsonl").unwrap().is_empty());
        assert!(matching(&dir.0, "media-", ".ts").unwrap().is_empty());
        assert!(dir.0.join("unrelated").exists());
        assert_eq!(r.status()["lines"], 0);
        r.record(event(99));
        r.shutdown();
        assert_eq!(matching(&dir.0, "trace-", ".jsonl").unwrap().len(), 1);
    }
    fn future_mtime(path: &Path) {
        let future = SystemTime::now() + Duration::from_secs(365 * 24 * 60 * 60);
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(future))
            .unwrap();
    }
    #[test]
    fn runs_increase_across_recorders_despite_future_times() {
        let dir = Temp::new();
        directory(&dir.0).unwrap();
        let old = dir.0.join("media-00000041-00000003-9999999999999.ts.part");
        fs::write(&old, b"truncated TS").unwrap();
        future_mtime(&old);
        for run in [42, 43] {
            let r = recorder(&dir.0, 160, 1024);
            r.record(event(1));
            r.capture().start(60);
            r.barrier();
            assert!(r.status()["file"]
                .as_str()
                .unwrap()
                .starts_with(&format!("trace-{run:08}-")));
            assert!(r.status()["capture"]["file"]
                .as_str()
                .unwrap()
                .starts_with(&format!("media-{run:08}-00000001-")));
            r.shutdown();
            if run == 42 {
                assert_eq!(fs::read(old.with_extension("")).unwrap(), b"truncated TS");
            }
            for path in matching(&dir.0, "", "").unwrap() {
                future_mtime(&path);
            }
        }
    }
    #[test]
    fn startup_recovers_partials_and_retains_by_run_and_index() {
        let dir = Temp::new();
        directory(&dir.0).unwrap();
        for part in 1..=10 {
            let path = dir.0.join(format!("trace-00000002-1-{part:08}.jsonl"));
            fs::write(&path, b"trace").unwrap();
            if part <= 2 {
                future_mtime(&path);
            }
        }
        let old_trace = dir.0.join("trace-00000001-9999999999999-99999999.jsonl");
        fs::write(&old_trace, b"old").unwrap();
        future_mtime(&old_trace);
        for (run, capture, timestamp) in
            [(1, 99, 9999999999999_u64), (2, 1, 9), (2, 2, 8), (2, 3, 1)]
        {
            let path = dir
                .0
                .join(format!("media-{run:08}-{capture:08}-{timestamp}.ts.part"));
            fs::write(&path, b"TS").unwrap();
            if run == 1 {
                future_mtime(&path);
            }
        }
        let invalid = [
            "trace-legacy.jsonl",
            "media-invalid.ts",
            "trace-00000099-x-00000001.jsonl",
        ];
        for name in invalid {
            fs::write(dir.0.join(name), b"keep").unwrap();
        }
        let r = recorder(&dir.0, 1024, 1024);
        r.barrier();
        assert!(!old_trace.exists());
        for part in 1..=10 {
            assert_eq!(
                dir.0
                    .join(format!("trace-00000002-1-{part:08}.jsonl"))
                    .exists(),
                part >= 3
            );
        }
        assert!(matching(&dir.0, "media-", ".ts.part").unwrap().is_empty());
        for name in [
            "media-00000002-00000002-8.ts",
            "media-00000002-00000003-1.ts",
        ] {
            assert_eq!(fs::read(dir.0.join(name)).unwrap(), b"TS");
        }
        assert_eq!(matching(&dir.0, "media-", ".ts").unwrap().len(), 3);
        for name in invalid {
            assert!(dir.0.join(name).exists());
        }
        r.record(event(1));
        r.barrier();
        assert!(r.status()["file"]
            .as_str()
            .unwrap()
            .starts_with("trace-00000003-"));
    }
    #[test]
    fn writer_failure_counts_and_clear_retries() {
        let dir = Temp::new();
        fs::write(&dir.0, b"not a directory").unwrap();
        let r = recorder(&dir.0, 1024, 1024);
        r.record(event(1));
        r.barrier();
        assert!(r.status()["failure"].is_string());
        assert_eq!(r.status()["dropped"], 1);
        fs::remove_file(&dir.0).unwrap();
        r.clear();
        r.record(event(2));
        r.barrier();
        assert!(r.status()["failure"].is_null());
        assert_eq!(r.status()["lines"], 1);
        assert_eq!(r.status()["dropped"], 0);
    }
    #[test]
    fn stalled_writer_drops_at_bounded_capacity_without_waiting() {
        let dir = Temp::new();
        let (entered, ready) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let released = Mutex::new(released);
        let first = AtomicBool::new(true);
        let r = TraceRecorder::new(
            &dir.0,
            Arc::new(move |_| {
                if first.swap(false, Ordering::Relaxed) {
                    entered.send(()).unwrap();
                    released.lock().unwrap().recv().unwrap();
                }
            }),
        );
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        for n in 0..8193 {
            r.record(event(n));
        }
        assert_eq!(r.shared.dropped.load(Ordering::Relaxed), 1);
        release.send(()).unwrap();
        r.shutdown();
        assert_eq!(r.status()["lines"], 8192);
    }
    #[test]
    fn capture_strips_header_renames_limits_and_keeps_two() {
        let dir = Temp::new();
        let r = recorder(&dir.0, 1024, 188);
        let tap = r.capture();
        let mut packet = vec![0; 12];
        packet.extend([0x47; 188]);
        for capture in 1..=3 {
            tap.start(60);
            r.barrier();
            let active = r.status();
            assert_eq!(active["capture"]["state"], "capturing");
            assert!(active["capture"]["file"]
                .as_str()
                .unwrap()
                .starts_with(&format!("media-00000001-{capture:08}-")));
            assert!(dir
                .0
                .join(active["capture"]["file"].as_str().unwrap())
                .exists());
            tap.rtp(&packet);
            let s = wait(&r, |s| s["capture"]["reason"] == "limit");
            assert_eq!(s["capture"]["bytes"], 188);
            assert_eq!(s["capture"]["state"], "idle");
            let p = dir.0.join(s["capture"]["file"].as_str().unwrap());
            assert_eq!(fs::read(&p).unwrap(), vec![0x47; 188]);
            assert_eq!(p.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert_eq!(matching(&dir.0, "media-", ".ts").unwrap().len(), 2);
        assert!(matching(&dir.0, "media-", ".part").unwrap().is_empty());
    }
    #[test]
    fn capture_stop_deadline_start_noop_and_clear() {
        let dir = Temp::new();
        let r = recorder(&dir.0, 1024, 1024);
        let tap = r.capture();
        tap.start(1);
        r.barrier();
        let name = r.status()["capture"]["file"].clone();
        tap.start(60);
        r.barrier();
        assert_eq!(r.status()["capture"]["file"], name);
        wait(&r, |s| s["capture"]["reason"] == "deadline");
        tap.start(60);
        r.barrier();
        tap.stop();
        r.barrier();
        assert_eq!(r.status()["capture"]["reason"], "stop");
        tap.start(60);
        r.barrier();
        fs::write(dir.0.join("media-leftover.ts.part"), b"partial").unwrap();
        r.clear();
        r.barrier();
        assert!(matching(&dir.0, "media-", ".ts.part").unwrap().is_empty());
        assert_eq!(r.status()["capture"]["state"], "idle");
        assert!(matching(&dir.0, "media-", ".ts").unwrap().is_empty());
    }
    #[test]
    fn stalled_capture_queue_overflow_ends_capture() {
        let dir = Temp::new();
        let (entered, ready) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let released = Mutex::new(released);
        let r = TraceRecorder::new(
            &dir.0,
            Arc::new(move |s| {
                if s["capture"]["state"] == "capturing" {
                    entered.send(()).unwrap();
                    released.lock().unwrap().recv().unwrap();
                }
            }),
        );
        let tap = r.capture();
        tap.start(60);
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        for _ in 0..2048 {
            tap.rtp(&[0; 200]);
        }
        assert!(r.shared.capturing.load(Ordering::Acquire));
        assert!(!r.shared.overflow.load(Ordering::Acquire));
        tap.rtp(&[0; 200]);
        assert!(!r.shared.capturing.load(Ordering::Acquire));
        assert!(r.shared.overflow.load(Ordering::Acquire));
        release.send(()).unwrap();
        let s = wait(&r, |s| s["capture"]["reason"] == "overflow");
        assert_eq!(s["capture"]["state"], "idle");
        assert_eq!(matching(&dir.0, "media-", ".ts").unwrap().len(), 1);
    }
    #[test]
    fn capture_error_is_reported_and_invalid_duration_is_ignored() {
        let dir = Temp::new();
        fs::write(&dir.0, b"file").unwrap();
        let r = recorder(&dir.0, 1024, 1024);
        let tap = r.capture();
        tap.start(0);
        r.barrier();
        assert_eq!(r.status()["capture"]["state"], "idle");
        tap.start(60);
        r.barrier();
        assert_eq!(r.status()["capture"]["state"], "failed");
        assert_eq!(r.status()["capture"]["reason"], "error");
    }
    #[test]
    fn default_rotation_boundary_is_eight_mib() {
        let dir = Temp::new();
        let r = TraceRecorder::new(&dir.0, Arc::new(|_| {}));
        let mut e = event(1);
        if let ClinkTraceEvent::Link { fields, .. } = &mut e {
            fields.insert("payload".into(), String::new());
        }
        let overhead = serde_json::to_vec(&e.to_json()).unwrap().len() + 1;
        if let ClinkTraceEvent::Link { fields, .. } = &mut e {
            fields.insert(
                "payload".into(),
                "x".repeat(TRACE_LIMIT as usize - overhead),
            );
        }
        r.record(e);
        r.record(event(2));
        r.shutdown();
        let paths = matching(&dir.0, "trace-", ".jsonl").unwrap();
        assert_eq!(paths.len(), 2);
        let mut sizes = paths
            .iter()
            .map(|p| p.metadata().unwrap().len())
            .collect::<Vec<_>>();
        sizes.sort();
        assert_eq!(sizes[1], TRACE_LIMIT);
        assert!(sizes[0] < TRACE_LIMIT);
    }
    #[test]
    fn status_changes_are_rate_limited_except_capture_transitions() {
        let dir = Temp::new();
        let events = Arc::new(Mutex::new(Vec::new()));
        let received = events.clone();
        let shared = Arc::new(Shared {
            dropped: AtomicU64::new(0),
            capturing: AtomicBool::new(false),
            overflow: AtomicBool::new(false),
            status: Mutex::new(Value::Null),
        });
        let mut w = Writer::new(
            dir.0.clone(),
            Arc::new(move |s| received.lock().unwrap().push(s)),
            shared,
            TRACE_LIMIT,
            CAPTURE_LIMIT,
        );
        directory(&dir.0).unwrap();
        w.publish(true);
        w.write_event(event(1)).unwrap();
        w.publish(false);
        let (_, packets) = mpsc::sync_channel(CAPTURE_QUEUE);
        w.command(Command::Clear, &packets);
        assert_eq!(events.lock().unwrap().len(), 1);
        w.last_status = Instant::now() - Duration::from_secs(1);
        w.publish(false);
        assert_eq!(events.lock().unwrap().len(), 1); // Clear restored the published counters.
        w.write_event(event(2)).unwrap();
        w.publish(false);
        assert_eq!(events.lock().unwrap().len(), 2);
        w.start_capture(Duration::from_secs(60), &packets);
        w.finish_capture("stop");
        assert_eq!(events.lock().unwrap().len(), 4);
    }
}
