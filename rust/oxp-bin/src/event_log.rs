// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{sync_channel, SyncSender, TrySendError},
        Arc, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};
/// Event lines written by one thread in arrival order, so a caller on a hot path never waits for a
/// stalled stdout/stderr reader: the hub drains callbacks on the caller's thread, so
/// [crate::clink_support::Sink] logs on the producer's wire thread. A full queue drops the event
/// and counts it; the count is written as a diagnostic where the events are missing.
pub struct EventLog {
    tx: SyncSender<Entry>,
    /// Drops not yet attached to a queued event.
    pending: Arc<AtomicU64>,
    dropped: AtomicU64,
}
enum Entry {
    /// An event and the drops counted just before it.
    Event(Value, u64),
    Flush(SyncSender<()>),
}
static GLOBAL: OnceLock<Arc<EventLog>> = OnceLock::new();
impl EventLog {
    pub const CAPACITY: usize = 1024;
    pub fn spawn(
        capacity: usize,
        mut write: impl FnMut(Value) + Send + 'static,
    ) -> std::io::Result<Arc<Self>> {
        let (tx, rx) = sync_channel(capacity);
        let pending = Arc::new(AtomicU64::new(0));
        let unreported = pending.clone();
        thread::Builder::new()
            .name("oxp-event-log".into())
            .spawn(move || {
                for entry in rx {
                    match entry {
                        Entry::Event(v, dropped) => {
                            if dropped > 0 {
                                write(dropped_event(dropped));
                            }
                            write(v);
                        }
                        Entry::Flush(done) => {
                            let dropped = unreported.swap(0, Ordering::AcqRel);
                            if dropped > 0 {
                                write(dropped_event(dropped));
                            }
                            let _ = done.send(());
                        }
                    }
                }
            })?;
        Ok(Arc::new(Self {
            tx,
            pending,
            dropped: AtomicU64::new(0),
        }))
    }
    /// The process-wide log, written with [crate::event].
    pub fn global() -> &'static Arc<EventLog> {
        GLOBAL.get_or_init(|| Self::spawn(Self::CAPACITY, crate::event).expect("event log thread"))
    }
    /// Flushes the process-wide log if it was used; see [Self::flush].
    pub fn flush_global(timeout: Duration) -> bool {
        GLOBAL.get().is_none_or(|log| log.flush(timeout))
    }
    /// Never waits: while the writer is `capacity` events behind, drops `v` and counts it.
    pub fn try_log(&self, v: Value) {
        let pending = self.pending.swap(0, Ordering::AcqRel);
        if self.tx.try_send(Entry::Event(v, pending)).is_err() {
            self.pending.fetch_add(pending + 1, Ordering::AcqRel);
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
    /// Events dropped since the log started.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
    /// Waits up to `timeout` until every event queued before the call and the drop count are
    /// written. False when the writer is still stalled.
    pub fn flush(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let (done, written) = sync_channel(1);
        let mut entry = Entry::Flush(done);
        loop {
            match self.tx.try_send(entry) {
                Ok(()) => break,
                Err(TrySendError::Full(e)) if Instant::now() < deadline => {
                    entry = e;
                    thread::sleep(Duration::from_millis(1));
                }
                Err(_) => return false,
            }
        }
        written
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .is_ok()
    }
}
fn dropped_event(count: u64) -> Value {
    json!({"type":"diagnostic","message":format!("event log dropped={count}")})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{mpsc, Mutex};
    type Lines = Arc<Mutex<Vec<Value>>>;
    /// A log whose writer blocks inside its first write until `release` is dropped.
    fn stalled(capacity: usize) -> (Arc<EventLog>, mpsc::Sender<()>, Lines) {
        let (release, gate) = mpsc::channel::<()>();
        let (started, writing) = mpsc::channel();
        let lines = Lines::default();
        let written = lines.clone();
        let log = EventLog::spawn(capacity, move |v| {
            let _ = started.send(());
            let _ = gate.recv();
            written.lock().unwrap().push(v);
        })
        .unwrap();
        log.try_log(json!({ "n": 0 }));
        writing.recv().unwrap();
        (log, release, lines)
    }
    fn n(v: &Value) -> Option<u64> {
        v["n"].as_u64()
    }
    #[test]
    fn stalled_writer_drops_with_a_count_in_place_and_keeps_order() {
        let (log, release, lines) = stalled(4);
        let start = Instant::now();
        for i in 1..100 {
            log.try_log(json!({ "n": i }));
        }
        assert!(start.elapsed() < Duration::from_millis(500));
        assert_eq!(95, log.dropped());
        assert!(
            !log.flush(Duration::from_millis(50)),
            "the writer is stalled"
        );
        drop(release);
        let deadline = Instant::now() + Duration::from_secs(5);
        while lines.lock().unwrap().len() < 5 {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        log.try_log(json!({ "n": 100 }));
        assert!(log.flush(Duration::from_secs(5)));
        // The first events in order, the drop count where the gap is, then the next event.
        let lines = lines.lock().unwrap();
        assert_eq!(
            vec![Some(0), Some(1), Some(2), Some(3), Some(4), None, Some(100)],
            lines.iter().map(n).collect::<Vec<_>>()
        );
        assert_eq!(
            lines[5],
            json!({"type":"diagnostic","message":"event log dropped=95"})
        );
    }
    #[test]
    fn flush_writes_drops_after_the_last_event() {
        let (log, release, lines) = stalled(1);
        for i in 1..10 {
            log.try_log(json!({ "n": i }));
        }
        assert_eq!(8, log.dropped());
        drop(release);
        assert!(log.flush(Duration::from_secs(5)));
        let lines = lines.lock().unwrap();
        assert_eq!(
            vec![Some(0), Some(1), None],
            lines.iter().map(n).collect::<Vec<_>>()
        );
        assert_eq!(lines[2]["message"], "event log dropped=8");
    }
}
