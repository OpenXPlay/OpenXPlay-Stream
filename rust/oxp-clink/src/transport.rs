// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

use crate::{ClinkAddresses, Result, SocketProvider, StdSockets};
use std::{
    collections::BTreeMap,
    io::{self, Read, Write},
    net::{IpAddr, Shutdown, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

pub trait ClinkStream: Send + Sync {
    fn read(&self, buffer: &mut [u8]) -> io::Result<usize>;
    /// Each call carries one complete protocol message, even on message-oriented transports.
    fn write_all(&self, message: &[u8]) -> io::Result<()>;
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
    /// Must unblock pending reads and writes.
    fn shutdown(&self) -> io::Result<()>;
}

impl ClinkStream for TcpStream {
    fn read(&self, buffer: &mut [u8]) -> io::Result<usize> {
        Read::read(&mut &*self, buffer)
    }
    fn write_all(&self, message: &[u8]) -> io::Result<()> {
        Write::write_all(&mut &*self, message)
    }
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        TcpStream::set_read_timeout(self, timeout)
    }
    fn shutdown(&self) -> io::Result<()> {
        TcpStream::shutdown(self, Shutdown::Both)
    }
}

pub trait ClinkTransport: Send + Sync {
    fn offer(&self, service: u16) -> Result<()>;
    /// Accept an offered service or dial a remote service. A timeout is retryable.
    fn open(&self, service: u16, timeout: Duration) -> Result<Arc<dyn ClinkStream>>;
    fn authenticated(&self) -> Result<()>;
    /// Allow another reverse connection after an optional channel closes.
    fn release(&self, _: u16) {}
}

struct OfferedStream {
    receiver: Mutex<Receiver<Result<Arc<dyn ClinkStream>>>>,
    accepted: Arc<AtomicBool>,
}

pub struct TcpTransport {
    addresses: ClinkAddresses,
    sockets: Arc<dyn SocketProvider>,
    listeners: Mutex<BTreeMap<u16, Arc<OfferedStream>>>,
    stopped: Arc<AtomicBool>,
}
impl TcpTransport {
    pub fn new(addresses: ClinkAddresses) -> Self {
        Self::with_sockets(addresses, Arc::new(StdSockets))
    }
    pub fn with_sockets(addresses: ClinkAddresses, sockets: Arc<dyn SocketProvider>) -> Self {
        Self {
            addresses,
            sockets,
            listeners: Mutex::new(BTreeMap::new()),
            stopped: Arc::new(AtomicBool::new(false)),
        }
    }
}
impl ClinkTransport for TcpTransport {
    fn offer(&self, service: u16) -> Result<()> {
        let listener = self.sockets.listen(self.addresses.local_ipv4, service)?;
        listener.set_nonblocking(true)?;
        let (tx, rx) = mpsc::channel();
        let stopped = self.stopped.clone();
        let peer = self.addresses.head_unit_ipv4;
        let accepted = Arc::new(AtomicBool::new(false));
        let gate = accepted.clone();
        thread::Builder::new()
            .name(format!("clink-tcp-accept-{service}"))
            .spawn(move || {
                while !stopped.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((socket, address)) => {
                            if gate.load(Ordering::Acquire) || address.ip() != IpAddr::V4(peer) {
                                continue;
                            }
                            // BSD accepted sockets inherit nonblocking mode; session readers block.
                            if let Err(e) = socket.set_nonblocking(false) {
                                let _ = tx.send(Err(e.into()));
                                break;
                            }
                            let _ = socket.set_nodelay(true);
                            let stream: Arc<dyn ClinkStream> = Arc::new(socket);
                            gate.store(true, Ordering::Release);
                            if tx.send(Ok(stream)).is_err() {
                                break;
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10))
                        }
                        Err(e) => {
                            let _ = tx.send(Err(e.into()));
                            break;
                        }
                    }
                }
            })?;
        self.listeners.lock().unwrap().insert(
            service,
            Arc::new(OfferedStream {
                receiver: Mutex::new(rx),
                accepted,
            }),
        );
        Ok(())
    }
    fn open(&self, service: u16, timeout: Duration) -> Result<Arc<dyn ClinkStream>> {
        let listener = self.listeners.lock().unwrap().get(&service).cloned();
        if let Some(listener) = listener {
            return match listener.receiver.lock().unwrap().recv_timeout(timeout) {
                Ok(result) => result,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    Err(io::Error::from(io::ErrorKind::TimedOut).into())
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => Err("transport_closed".into()),
            };
        }
        let port = if service == 57209 {
            self.addresses.auth_port
        } else {
            service
        };
        let socket = self.sockets.connect(
            self.addresses.local_ipv4,
            self.addresses.head_unit_ipv4,
            port,
            timeout,
        )?;
        socket.set_nodelay(true)?;
        Ok(Arc::new(socket))
    }
    fn authenticated(&self) -> Result<()> {
        Ok(())
    }
    fn release(&self, service: u16) {
        if let Some(listener) = self.listeners.lock().unwrap().get(&service) {
            listener.accepted.store(false, Ordering::Release);
        }
    }
}

impl Drop for TcpTransport {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
    }
}
