// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use crate::{att::Socket, AddressType};
use oxp_clink::Result;
use std::{io, mem::size_of, time::Instant};

const BTPROTO_L2CAP: libc::c_int = 0;
fn failure(code: &'static str) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(crate::diagnostics::PlatformFailure::io(
        code,
        io::Error::last_os_error(),
    ))
}

#[repr(C)]
struct SockaddrL2 {
    family: libc::sa_family_t,
    psm: u16,
    address: [u8; 6],
    cid: u16,
    address_type: u8,
    padding: u8,
}
fn sockaddr(mac: &str, kind: AddressType) -> Result<SockaddrL2> {
    let mut address = crate::mac_bytes(mac)?;
    // Linux bdaddr_t is little endian; Go's SockaddrL2 conversion also reverses it.
    address.reverse();
    Ok(SockaddrL2 {
        family: libc::AF_BLUETOOTH as _,
        psm: 0,
        address,
        cid: 4u16.to_le(),
        address_type: kind as u8,
        padding: 0,
    })
}
pub(crate) struct NativeSocket {
    fd: libc::c_int,
    deadline: Instant,
}
impl Drop for NativeSocket {
    fn drop(&mut self) {
        // SAFETY: This object uniquely owns the valid socket descriptor.
        unsafe {
            libc::close(self.fd);
        }
    }
}
impl NativeSocket {
    pub(crate) fn connect(
        local: &str,
        remote: &str,
        kind: AddressType,
        deadline: Instant,
    ) -> Result<Self> {
        let local = sockaddr(local, AddressType::Public)?;
        let remote = sockaddr(remote, kind)?;
        // SAFETY: Constants specify a nonblocking Linux L2CAP socket; no pointers are used.
        let fd = unsafe {
            libc::socket(
                libc::AF_BLUETOOTH,
                libc::SOCK_SEQPACKET | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                BTPROTO_L2CAP,
            )
        };
        if fd < 0 {
            return Err(failure("bluetooth_socket_failed"));
        }
        let s = Self { fd, deadline };
        // SAFETY: local is an initialized repr(C) sockaddr_l2 with its exact size.
        let rc = unsafe {
            libc::bind(
                fd,
                (&local as *const SockaddrL2).cast(),
                size_of::<SockaddrL2>() as _,
            )
        };
        if rc < 0 {
            return Err(failure("bluetooth_bind_failed"));
        }
        // SAFETY: remote is an initialized repr(C) sockaddr_l2 with its exact size.
        let rc = unsafe {
            libc::connect(
                fd,
                (&remote as *const SockaddrL2).cast(),
                size_of::<SockaddrL2>() as _,
            )
        };
        if rc < 0 {
            if io::Error::last_os_error().raw_os_error() != Some(libc::EINPROGRESS) {
                return Err(failure("bluetooth_connect_failed"));
            }
            s.poll(libc::POLLOUT)?;
            let mut code: libc::c_int = 0;
            let mut len = size_of::<libc::c_int>() as libc::socklen_t;
            // SAFETY: code and len are writable and sized for SO_ERROR, and fd is live.
            let rc = unsafe {
                libc::getsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_ERROR,
                    (&mut code as *mut libc::c_int).cast(),
                    &mut len,
                )
            };
            if rc < 0 {
                return Err(failure("bluetooth_connect_failed"));
            }
            if code != 0 {
                return Err(crate::diagnostics::PlatformFailure::io(
                    "bluetooth_connect_failed",
                    io::Error::from_raw_os_error(code),
                )
                .into());
            }
        }
        Ok(s)
    }
    fn poll(&self, event: i16) -> Result<()> {
        loop {
            crate::signals::check()?;
            let remaining = self.deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("bluetooth_handoff_timeout".into());
            }
            let mut p = libc::pollfd {
                fd: self.fd,
                events: event,
                revents: 0,
            };
            // SAFETY: p is one initialized pollfd referring to our live socket.
            let rc = unsafe { libc::poll(&mut p, 1, remaining.as_millis().clamp(1, 200) as i32) };
            if rc < 0 {
                if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(failure("bluetooth_socket_failed"));
            }
            if p.revents & event != 0 {
                return Ok(());
            }
            if p.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                return Err("bluetooth_handoff_closed".into());
            }
        }
    }
}
impl Socket for NativeSocket {
    fn set_deadline(&mut self, deadline: Instant) {
        self.deadline = deadline;
    }
    fn cancel_prepared(&mut self) -> Result<()> {
        let old = self.deadline;
        self.deadline = Instant::now() + std::time::Duration::from_secs(1);
        let result = self.send(&[0x18, 0]);
        self.deadline = old;
        result
    }

    fn send(&mut self, b: &[u8]) -> Result<()> {
        loop {
            self.poll(libc::POLLOUT)?;
            // SAFETY: b provides b.len() readable bytes and fd is our live socket.
            let rc = unsafe { libc::send(self.fd, b.as_ptr().cast(), b.len(), libc::MSG_NOSIGNAL) };
            if rc >= 0 {
                return if rc as usize == b.len() {
                    Ok(())
                } else {
                    Err("bluetooth_partial_write".into())
                };
            }
            match io::Error::last_os_error().kind() {
                io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock => continue,
                _ => return Err(failure("bluetooth_write_failed")),
            }
        }
    }
    fn receive(&mut self) -> Result<Vec<u8>> {
        let mut b = vec![0; 513];
        loop {
            self.poll(libc::POLLIN)?;
            // SAFETY: b provides 513 writable bytes and fd is our live socket.
            let rc =
                unsafe { libc::recv(self.fd, b.as_mut_ptr().cast(), b.len(), libc::MSG_TRUNC) };
            if rc >= 0 {
                if rc == 0 || rc as usize > b.len() {
                    return Err("bluetooth_packet_limit".into());
                }
                b.truncate(rc as usize);
                return Ok(b);
            }
            match io::Error::last_os_error().kind() {
                io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock => continue,
                _ => return Err(failure("bluetooth_read_failed")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn linux_address_layout() {
        let s = sockaddr("aa:bb:cc:dd:ee:ff", AddressType::Random).unwrap();
        assert_eq!(s.address, [255, 238, 221, 204, 187, 170]);
        assert_eq!(s.cid, 4u16.to_le());
        assert_eq!(s.address_type, 2);
        assert_eq!(size_of::<SockaddrL2>(), 14);
    }
}
