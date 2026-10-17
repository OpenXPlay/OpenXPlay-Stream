// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

//! Bounds the kernel send buffer of browser sockets. Without a bound the kernel accepts several
//! hundred kilobytes for a slow receiver, so frames the server counts as sent still wait there and
//! the per-client frame queue never sees the backlog. With it, a new frame starts only while little
//! is unsent, later frames stay in that queue, and the queue drops them to the next key frame within
//! the playback budget. A frame already started is always written completely.
//!
//! std exposes none of these socket options, so this module calls them through libc. It is the
//! only module in this crate that may use unsafe code.
#![allow(unsafe_code)]

use serde_json::{json, Value};
use std::net::TcpStream;

/// The bound a socket received, logged once per browser client.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    )),
    allow(dead_code)
)]
pub(crate) enum SendBound {
    /// TCP_NOTSENT_LOWAT: no new frame starts while this many bytes are unsent. Sent but
    /// unacknowledged bytes do not count, so it does not limit throughput.
    NotSentLowWater(usize),
    /// SO_SNDBUF as the kernel reports it, unsent plus unacknowledged bytes, where the low-water
    /// option is unavailable.
    SendBuffer(usize),
    /// Disabled by configuration (zero bytes) or unsupported on this platform.
    Default,
    Failed(String),
}
impl SendBound {
    pub(crate) fn json(&self) -> Value {
        match self {
            Self::NotSentLowWater(bytes) => json!({"notSentLowWaterBytes": bytes}),
            Self::SendBuffer(bytes) => json!({"sendBufferBytes": bytes}),
            Self::Default => json!({"sendBuffer": "default"}),
            Self::Failed(error) => json!({"sendBuffer": "default", "error": error}),
        }
    }
}

/// Bytes the kernel still holds for a socket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct KernelQueue {
    /// Unsent plus sent but unacknowledged bytes: Linux SIOCOUTQ (the Send-Q column of `ss`) or
    /// Darwin SO_NWRITE.
    pub(crate) queued: u64,
    /// Bytes not yet sent at all (Linux SIOCOUTQNSD); Darwin does not report it.
    pub(crate) unsent: Option<u64>,
}

/// Applies the bound for `unsent_bytes`; zero keeps the operating system's default.
pub(crate) fn bound(socket: &TcpStream, unsent_bytes: usize) -> SendBound {
    if unsent_bytes == 0 {
        return SendBound::Default;
    }
    sys::bound(socket, i32::try_from(unsent_bytes).unwrap_or(i32::MAX / 2))
}

pub(crate) fn kernel_queue(socket: &TcpStream) -> Option<KernelQueue> {
    sys::kernel_queue(socket)
}

/// Whether the socket may take another frame. Linux enforces TCP_NOTSENT_LOWAT in write() itself,
/// so this is always true there. Darwin applies the option only to readiness, so a frame may start
/// only while poll() reports the socket writable, that is while less than the bound is unsent; a
/// frame already started is still written completely.
pub(crate) fn writable(socket: &TcpStream) -> bool {
    sys::writable(socket)
}

#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios"
))]
mod ffi {
    use std::{ffi::c_int, io, mem::size_of, net::TcpStream, os::fd::AsRawFd};
    pub(super) fn set(
        socket: &TcpStream,
        level: c_int,
        name: c_int,
        value: c_int,
    ) -> io::Result<()> {
        // SAFETY: The descriptor is live for the borrow, and the option value is a readable int of
        // exactly the size passed.
        let result = unsafe {
            libc::setsockopt(
                socket.as_raw_fd(),
                level,
                name,
                (&value as *const c_int).cast(),
                size_of::<c_int>() as libc::socklen_t,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
    pub(super) fn get(socket: &TcpStream, level: c_int, name: c_int) -> io::Result<c_int> {
        let mut value: c_int = 0;
        let mut length = size_of::<c_int>() as libc::socklen_t;
        // SAFETY: The descriptor is live for the borrow; value and length are writable and the
        // length matches the value's size.
        let result = unsafe {
            libc::getsockopt(
                socket.as_raw_fd(),
                level,
                name,
                (&mut value as *mut c_int).cast(),
                &mut length,
            )
        };
        if result == 0 {
            Ok(value)
        } else {
            Err(io::Error::last_os_error())
        }
    }
    /// TCP_NOTSENT_LOWAT, falling back to SO_SNDBUF at twice the bound.
    pub(super) fn bound(socket: &TcpStream, low_water: c_int, bytes: c_int) -> super::SendBound {
        use super::SendBound;
        if set(socket, libc::IPPROTO_TCP, low_water, bytes).is_ok() {
            let effective = get(socket, libc::IPPROTO_TCP, low_water).unwrap_or(bytes);
            return SendBound::NotSentLowWater(effective.max(0) as usize);
        }
        // Linux before 3.12 lacks the option; a whole-buffer bound also caps bytes in flight.
        match set(
            socket,
            libc::SOL_SOCKET,
            libc::SO_SNDBUF,
            bytes.saturating_mul(2),
        ) {
            Ok(()) => match get(socket, libc::SOL_SOCKET, libc::SO_SNDBUF) {
                Ok(effective) => SendBound::SendBuffer(effective.max(0) as usize),
                Err(e) => SendBound::Failed(e.to_string()),
            },
            Err(e) => SendBound::Failed(e.to_string()),
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod sys {
    use super::{ffi, KernelQueue, SendBound};
    use std::{ffi::c_int, io, net::TcpStream, os::fd::AsRawFd};
    pub(super) fn bound(socket: &TcpStream, bytes: c_int) -> SendBound {
        // The kernel checks this before it queues each new segment, so a nonblocking write stops
        // after at most one segment beyond it.
        ffi::bound(socket, libc::TCP_NOTSENT_LOWAT, bytes)
    }
    fn query(socket: &TcpStream, request: u64) -> io::Result<u64> {
        let mut value: c_int = 0;
        // SAFETY: The descriptor is live for the borrow, and these socket requests write one int
        // through the pointer. The request type differs between C libraries, hence the cast.
        let result =
            unsafe { libc::ioctl(socket.as_raw_fd(), request as _, &mut value as *mut c_int) };
        if result == 0 {
            Ok(value.max(0) as u64)
        } else {
            Err(io::Error::last_os_error())
        }
    }
    // The constants' types differ between C libraries, so a cast is redundant on some targets.
    #[allow(clippy::unnecessary_cast)]
    pub(super) fn kernel_queue(socket: &TcpStream) -> Option<KernelQueue> {
        // SIOCOUTQ shares its value with TIOCOUTQ, which libc defines per architecture.
        Some(KernelQueue {
            queued: query(socket, libc::TIOCOUTQ as u64).ok()?,
            unsent: query(socket, libc::SIOCOUTQNSD as u64).ok(),
        })
    }
    pub(super) fn writable(_: &TcpStream) -> bool {
        true
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod sys {
    use super::{ffi, KernelQueue, SendBound};
    use std::{ffi::c_int, net::TcpStream, os::fd::AsRawFd};
    /// From <netinet/tcp.h>; libc does not define it for Darwin.
    const TCP_NOTSENT_LOWAT: c_int = 0x201;
    pub(super) fn bound(socket: &TcpStream, bytes: c_int) -> SendBound {
        ffi::bound(socket, TCP_NOTSENT_LOWAT, bytes)
    }
    pub(super) fn writable(socket: &TcpStream) -> bool {
        let mut entry = libc::pollfd {
            fd: socket.as_raw_fd(),
            events: libc::POLLOUT,
            revents: 0,
        };
        // SAFETY: One initialized entry is passed with a count of one, and a zero timeout never
        // blocks. The descriptor is live for the borrow.
        let ready = unsafe { libc::poll(&mut entry, 1, 0) };
        // An error or hangup also reports ready, so the next write surfaces it.
        ready != 0
    }
    pub(super) fn kernel_queue(socket: &TcpStream) -> Option<KernelQueue> {
        let queued = ffi::get(socket, libc::SOL_SOCKET, libc::SO_NWRITE).ok()?;
        Some(KernelQueue {
            queued: queued.max(0) as u64,
            unsent: None,
        })
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios"
)))]
mod sys {
    use super::{KernelQueue, SendBound};
    use std::net::TcpStream;
    pub(super) fn bound(_: &TcpStream, _: i32) -> SendBound {
        SendBound::Default
    }
    pub(super) fn kernel_queue(_: &TcpStream) -> Option<KernelQueue> {
        None
    }
    pub(super) fn writable(_: &TcpStream) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };

    #[test]
    fn bound_is_applied_and_the_kernel_queue_is_reported() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut server, _) = listener.accept().unwrap();
        assert_eq!(bound(&server, 0), SendBound::Default);
        let applied = bound(&server, 16 * 1024);
        let supported = cfg!(any(
            target_os = "macos",
            all(
                target_os = "linux",
                any(target_arch = "x86_64", target_arch = "aarch64")
            )
        ));
        if supported {
            assert_eq!(applied, SendBound::NotSentLowWater(16 * 1024));
        }
        assert!(applied.json().is_object());
        server.write_all(&[7; 1000]).unwrap();
        let mut received = [0; 1000];
        client.read_exact(&mut received).unwrap();
        assert_eq!(received, [7; 1000]);
        let queue = kernel_queue(&server);
        if supported {
            assert!(queue.is_some());
        }
        if let Some(queue) = queue {
            // Everything was read; only a delayed acknowledgement may keep bytes queued.
            assert!(queue.queued <= 1000, "{queue:?}");
            assert!(queue.unsent.unwrap_or(0) <= queue.queued, "{queue:?}");
        }
        // Writable again once the peer has read everything.
        assert!(writable(&server));
    }
}
