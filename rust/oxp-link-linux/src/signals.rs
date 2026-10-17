// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-15

use oxp_clink::Result;
use std::io;

use std::sync::atomic::{AtomicBool, Ordering};

static SIGNALS_INSTALLED: AtomicBool = AtomicBool::new(false);
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

extern "C" fn shutdown_signal(signal: libc::c_int) {
    // AtomicBool is lock-free on the supported Linux targets. No allocation,
    // locks, logging, or network operations are allowed in this handler.
    if SHUTDOWN_REQUESTED.swap(true, Ordering::SeqCst) {
        // SAFETY: _exit is async-signal-safe and intentionally skips cleanup
        // when the user sends a second signal, including during leave().
        unsafe { libc::_exit(128 + signal) }
    }
}

/// Process-wide SIGINT/SIGTERM handlers for a connection lifetime.
/// Keep this guard alive until network cleanup has completed.
pub struct ShutdownSignals {
    previous: [libc::sigaction; 2],
}
impl ShutdownSignals {
    pub fn install() -> Result<Self> {
        if SIGNALS_INSTALLED.swap(true, Ordering::SeqCst) {
            return Err("shutdown_signal_handlers_already_installed".into());
        }
        SHUTDOWN_REQUESTED.store(false, Ordering::SeqCst);
        // SAFETY: Unix sigaction and sigset_t are C structs valid when zeroed;
        // sigemptyset initializes the mask before it is passed to sigaction.
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        // SAFETY: previous contains writable sigaction storage; successful
        // sigaction calls fill each element before it is used for restoration.
        let mut previous: [libc::sigaction; 2] = unsafe { std::mem::zeroed() };
        action.sa_sigaction = shutdown_signal as *const () as usize;
        action.sa_flags = libc::SA_RESTART;
        // SAFETY: The mask points to live writable storage. Block both signals
        // in this handler; the atomic swap also handles delivery on other threads.
        unsafe {
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaddset(&mut action.sa_mask, libc::SIGINT);
            libc::sigaddset(&mut action.sa_mask, libc::SIGTERM);
        }
        for (index, signal) in [libc::SIGINT, libc::SIGTERM].into_iter().enumerate() {
            // SAFETY: action is initialized and previous[index] is writable.
            if unsafe { libc::sigaction(signal, &action, &mut previous[index]) } < 0 {
                let error = io::Error::last_os_error();
                if index == 1 {
                    // SAFETY: The first install succeeded and saved this action.
                    unsafe {
                        libc::sigaction(libc::SIGINT, &previous[0], std::ptr::null_mut());
                    }
                }
                SIGNALS_INSTALLED.store(false, Ordering::SeqCst);
                return Err(error.into());
            }
        }
        Ok(Self { previous })
    }

    pub fn requested(&self) -> bool {
        SHUTDOWN_REQUESTED.load(Ordering::SeqCst)
    }
}
impl Drop for ShutdownSignals {
    fn drop(&mut self) {
        for (signal, action) in [libc::SIGINT, libc::SIGTERM]
            .into_iter()
            .zip(&self.previous)
        {
            // SAFETY: Both saved actions were filled by successful installations.
            unsafe {
                libc::sigaction(signal, action, std::ptr::null_mut());
            }
        }
        SIGNALS_INSTALLED.store(false, Ordering::SeqCst);
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn check() -> Result<()> {
    if SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
        Err("shutdown_requested".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shutdown_signal_child() {
        let Ok(mode) = std::env::var("OXP_LINK_SIGNAL_TEST") else {
            return;
        };
        // Reproduce a background launch inheriting SIGINT as ignored.
        // SAFETY: SIG_IGN is a valid disposition for these catchable signals.
        unsafe {
            libc::signal(libc::SIGINT, libc::SIG_IGN);
        }
        let signals = ShutdownSignals::install().unwrap();
        assert!(ShutdownSignals::install().is_err());
        assert!(!signals.requested());
        let first = if mode.starts_with("int") {
            libc::SIGINT
        } else {
            libc::SIGTERM
        };
        // SAFETY: raise delivers a catchable signal with our installed handler.
        assert_eq!(unsafe { libc::raise(first) }, 0);
        assert!(signals.requested());
        if mode.ends_with("once") {
            drop(signals);
            // SAFETY: The guard restored the inherited SIG_IGN disposition.
            assert_eq!(unsafe { libc::raise(libc::SIGINT) }, 0);
            return;
        }
        let second = if first == libc::SIGINT {
            libc::SIGTERM
        } else {
            libc::SIGINT
        };
        // SAFETY: The second signal must terminate this isolated subprocess.
        unsafe {
            libc::raise(second);
        }
        panic!("second signal did not exit");
    }

    #[test]
    fn shutdown_signals_override_ignored_int_and_force_second_exit() {
        for (mode, code) in [
            ("int-once", 0),
            ("term-once", 0),
            ("int-twice", 143),
            ("term-twice", 130),
        ] {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "signals::tests::shutdown_signal_child"])
                .env("OXP_LINK_SIGNAL_TEST", mode)
                .status()
                .unwrap();
            assert_eq!(status.code(), Some(code), "{mode}");
        }
    }
}
