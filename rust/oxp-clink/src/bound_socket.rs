// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

use std::{
    io, mem,
    net::{Ipv4Addr, TcpStream},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    time::{Duration, Instant},
};

fn address(ip: Ipv4Addr, port: u16) -> libc::sockaddr_in {
    // SAFETY: sockaddr_in is a plain C address structure; all fields are set below.
    let mut address: libc::sockaddr_in = unsafe { mem::zeroed() };
    address.sin_family = libc::AF_INET as _;
    #[cfg(target_os = "macos")]
    {
        address.sin_len = mem::size_of::<libc::sockaddr_in>() as _;
    }
    address.sin_port = port.to_be();
    address.sin_addr.s_addr = u32::from_ne_bytes(ip.octets());
    address
}

pub(crate) fn connect(
    local: Ipv4Addr,
    peer: Ipv4Addr,
    port: u16,
    timeout: Duration,
) -> io::Result<TcpStream> {
    if timeout.is_zero() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "zero connect timeout",
        ));
    }
    // SAFETY: socket returns a fresh descriptor, immediately placed under RAII ownership.
    let raw = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: raw is a newly created, uniquely owned socket descriptor.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    // SAFETY: fd is live; FD_CLOEXEC prevents system-tool children inheriting it.
    if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let local_address = address(local, 0);
    // SAFETY: fd is live and local_address is a valid IPv4 sockaddr with the given length.
    if unsafe {
        libc::bind(
            fd.as_raw_fd(),
            (&local_address as *const libc::sockaddr_in).cast(),
            mem::size_of_val(&local_address) as _,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    #[cfg(target_os = "linux")]
    bind_device(fd.as_raw_fd(), local);
    let stream = TcpStream::from(fd);
    stream.set_nonblocking(true)?;
    let remote = address(peer, port);
    // SAFETY: stream owns a live socket; remote points to a complete IPv4 sockaddr.
    let result = unsafe {
        libc::connect(
            stream.as_raw_fd(),
            (&remote as *const libc::sockaddr_in).cast(),
            mem::size_of_val(&remote) as _,
        )
    };
    if result != 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(error);
        }
        let end = Instant::now() + timeout;
        loop {
            let remaining = end.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "connect timeout"));
            }
            let mut event = libc::pollfd {
                fd: stream.as_raw_fd(),
                events: libc::POLLOUT,
                revents: 0,
            };
            let millis = remaining
                .as_millis()
                .saturating_add(1)
                .min(i32::MAX as u128) as i32;
            // SAFETY: event is a live single-element pollfd array and stream remains owned.
            let ready = unsafe { libc::poll(&mut event, 1, millis) };
            if ready < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            if ready == 0 {
                continue;
            }
            if let Some(error) = stream.take_error()? {
                return Err(error);
            }
            break;
        }
    }
    stream.set_nonblocking(false)?;
    Ok(stream)
}

#[cfg(target_os = "linux")]
fn bind_device(fd: std::os::fd::RawFd, local: Ipv4Addr) {
    let mut list = std::ptr::null_mut();
    // SAFETY: getifaddrs writes an allocated list pointer, freed exactly once below.
    if unsafe { libc::getifaddrs(&mut list) } != 0 {
        return;
    }
    let mut current = list;
    while !current.is_null() {
        // SAFETY: current points into the live getifaddrs list until freeifaddrs below.
        let entry = unsafe { &*current };
        if !entry.ifa_addr.is_null() && !entry.ifa_name.is_null() {
            // SAFETY: a non-null ifa_addr is a sockaddr; AF_INET has sockaddr_in layout.
            unsafe {
                if (*entry.ifa_addr).sa_family as i32 == libc::AF_INET {
                    let address = &*entry.ifa_addr.cast::<libc::sockaddr_in>();
                    if address.sin_addr.s_addr.to_ne_bytes() == local.octets() {
                        let name = std::ffi::CStr::from_ptr(entry.ifa_name).to_bytes_with_nul();
                        // Best effort: unprivileged processes still use the mandatory address bind.
                        libc::setsockopt(
                            fd,
                            libc::SOL_SOCKET,
                            libc::SO_BINDTODEVICE,
                            name.as_ptr().cast(),
                            name.len() as _,
                        );
                        break;
                    }
                }
            }
        }
        current = entry.ifa_next;
    }
    // SAFETY: list was returned by successful getifaddrs and has not been freed.
    unsafe { libc::freeifaddrs(list) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SocketProvider, StdSockets};
    #[test]
    fn outbound_socket_is_bound_before_connecting() {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let socket = StdSockets
            .connect(
                Ipv4Addr::LOCALHOST,
                Ipv4Addr::LOCALHOST,
                port,
                Duration::from_secs(1),
            )
            .unwrap();
        let (_, peer) = listener.accept().unwrap();
        assert_eq!(peer.ip(), socket.local_addr().unwrap().ip());
        assert_eq!(socket.local_addr().unwrap().ip(), Ipv4Addr::LOCALHOST);
        assert!(StdSockets
            .connect(
                Ipv4Addr::new(192, 0, 2, 1),
                Ipv4Addr::LOCALHOST,
                port,
                Duration::from_secs(1)
            )
            .is_err());
        listener.set_nonblocking(true).unwrap();
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }
}
