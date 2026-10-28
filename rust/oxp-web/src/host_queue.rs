// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

use crate::Diagnostics;
use oxp_core::model::*;
use std::{
    collections::{BTreeMap, VecDeque},
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{Arc, Condvar, Mutex},
};

type Callback = Box<dyn FnOnce() + Send>;
type Key = (String, String, u32);
enum Pending {
    Essential(Callback),
    Report(Key),
}
#[derive(Default)]
struct State {
    pending: VecDeque<Pending>,
    reports: BTreeMap<Key, Callback>,
    waiting: usize,
    dropped: u64,
    stopped: bool,
}
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    capacity: usize,
}

/// One host thread per output. Essential callbacks retain order and are never dropped; waiting
/// reports retain their place but take the newest value. The capacity includes a running callback
/// and essential callbacks, just like Kotlin's HostQueue; essentials may exceed that limit.
#[derive(Clone)]
pub(crate) struct HostQueue(Arc<Shared>);
impl HostQueue {
    pub(crate) fn new(capacity: usize) -> std::io::Result<Self> {
        assert!(capacity > 0);
        let queue = Self(Arc::new(Shared {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
            capacity,
        }));
        let worker = queue.clone();
        std::thread::Builder::new()
            .name("oxp-web-host".into())
            .spawn(move || worker.run())?;
        Ok(queue)
    }

    pub(crate) fn essential(&self, callback: impl FnOnce() + Send + 'static) {
        let mut state = self.0.state.lock().unwrap();
        if !state.stopped {
            state
                .pending
                .push_back(Pending::Essential(Box::new(callback)));
            state.waiting += 1;
            self.0.wake.notify_one();
        }
    }

    fn report(&self, key: Key, callback: impl FnOnce() + Send + 'static) {
        let mut state = self.0.state.lock().unwrap();
        if state.stopped {
            return;
        }
        if let Some(waiting) = state.reports.get_mut(&key) {
            *waiting = Box::new(callback);
        } else if state.waiting >= self.0.capacity {
            state.dropped += 1;
        } else {
            state.reports.insert(key.clone(), Box::new(callback));
            state.pending.push_back(Pending::Report(key));
            state.waiting += 1;
            self.0.wake.notify_one();
        }
    }

    fn run(&self) {
        loop {
            let callback = {
                let mut state = self.0.state.lock().unwrap();
                while state.pending.is_empty() && !state.stopped {
                    state = self.0.wake.wait(state).unwrap();
                }
                if state.stopped {
                    return;
                }
                match state.pending.pop_front().unwrap() {
                    Pending::Essential(callback) => callback,
                    Pending::Report(key) => state.reports.remove(&key).unwrap(),
                }
            };
            // A host panic must not kill delivery of subsequent callbacks or poison the queue.
            let _ = catch_unwind(AssertUnwindSafe(callback));
            self.0.state.lock().unwrap().waiting -= 1;
        }
    }

    /// Stop accepting work and discard pending callbacks without waiting for a blocked host.
    pub(crate) fn shutdown(&self) {
        let mut state = self.0.state.lock().unwrap();
        state.stopped = true;
        state.waiting -= state.pending.len();
        state.pending.clear();
        state.reports.clear();
        self.0.wake.notify_one();
    }

    pub(crate) fn diagnostics(&self, callback: Diagnostics) -> Diagnostics {
        let queue = self.clone();
        Arc::new(move |id, value| {
            let key = (
                value["type"].as_str().unwrap_or_default().to_owned(),
                id.to_owned(),
                value["screen"].as_f64().unwrap_or(0.0) as u32,
            );
            let (id, value, callback) = (id.to_owned(), value.clone(), callback.clone());
            if value["type"] == "receiver-disconnected" {
                queue.essential(move || callback(&id, &value));
            } else {
                queue.report(key, move || callback(&id, &value));
            }
        })
    }

    #[cfg(test)]
    pub(crate) fn size(&self) -> usize {
        self.0.state.lock().unwrap().waiting
    }

    #[cfg(test)]
    pub(crate) fn await_idle(&self) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.essential(move || {
            let _ = tx.send(());
        });
        rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    }
}

pub(crate) struct QueuedHost {
    pub(crate) queue: HostQueue,
    pub(crate) host: Arc<dyn OutputHost>,
}
impl OutputHost for QueuedHost {
    fn capabilities(&self, caps: OutputCapabilities) {
        let host = self.host.clone();
        self.queue.essential(move || host.capabilities(caps));
    }
    fn control(&self, event: ControlEvent) {
        let host = self.host.clone();
        self.queue.essential(move || host.control(event));
    }
    fn stats(&self, stats: OutputStats) {
        let host = self.host.clone();
        let key = (
            "output-stats".into(),
            stats.origin.client_id.clone(),
            stats.screen,
        );
        self.queue.report(key, move || host.stats(stats));
    }
    fn request_restart(&self, origin: Origin) {
        let host = self.host.clone();
        self.queue.essential(move || host.request_restart(origin));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc::channel, time::Duration};

    #[test]
    fn coalesces_reports_preserves_essential_order_and_survives_panics() {
        let queue = HostQueue::new(4).unwrap();
        let (release, blocked) = channel();
        let (started, ready) = channel();
        queue.essential(move || {
            started.send(()).unwrap();
            blocked.recv_timeout(Duration::from_secs(5)).unwrap();
        });
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        let (tx, rx) = channel();
        for (key, value) in [("a", "a1"), ("a", "a2"), ("b", "b")] {
            let tx = tx.clone();
            queue.report((key.into(), String::new(), 0), move || {
                tx.send(value).unwrap()
            });
        }
        for value in ["control0", "control1", "control2"] {
            let tx = tx.clone();
            queue.essential(move || tx.send(value).unwrap());
        }
        queue.report(("c".into(), String::new(), 0), || panic!("dropped report"));
        let delivered = tx.clone();
        queue.report(("a".into(), String::new(), 0), move || {
            delivered.send("a3").unwrap()
        });
        queue.essential(|| panic!("failing host callback"));
        queue.essential(move || tx.send("control3").unwrap());
        assert_eq!(queue.0.state.lock().unwrap().dropped, 1);
        assert_eq!(queue.size(), 8);
        release.send(()).unwrap();
        queue.await_idle();
        assert_eq!(
            rx.try_iter().collect::<Vec<_>>(),
            ["a3", "b", "control0", "control1", "control2", "control3"]
        );
        queue.shutdown();
    }

    #[test]
    fn shutdown_never_waits_for_a_blocked_callback_and_discards_pending_work() {
        let queue = HostQueue::new(1).unwrap();
        let (release, blocked) = channel();
        let (started, ready) = channel();
        queue.essential(move || {
            started.send(()).unwrap();
            let _ = blocked.recv_timeout(Duration::from_secs(5));
        });
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        let (tx, rx) = channel();
        queue.essential(move || tx.send(()).unwrap());
        queue.shutdown();
        assert_eq!(queue.size(), 1);
        release.send(()).unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_err());
        queue.essential(|| panic!("stopped queue"));
    }
}
