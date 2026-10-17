// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

//! Safe wrappers over the V4L2 ioctls of one video node. All buffers use MMAP memory and the
//! multi-planar API; the descriptor is non-blocking so dequeues never stall the worker.

use super::sys::*;
use crate::negotiate::{DeviceInfo, Inventory, MplaneFormat};
use libc::c_int;
use std::{
    ffi::CString,
    io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::ffi::OsStrExt,
    },
    path::{Path, PathBuf},
};

pub struct Device {
    fd: OwnedFd,
    pub path: PathBuf,
}

/// A buffer handed back by the driver.
pub struct Dequeued {
    pub index: u32,
    pub flags: u32,
    /// The timestamp cookie the buffer was queued with (copied from input to output by codecs).
    pub cookie: u64,
    pub bytesused: u32,
    pub data_offset: u32,
}

/// One mapped plane of a driver buffer.
pub struct Mapping {
    ptr: *mut u8,
    len: usize,
}
// SAFETY: the mapping is owned by exactly one buffer set and only touched by its owner.
unsafe impl Send for Mapping {}
impl Mapping {
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn bytes(&self) -> &[u8] {
        // SAFETY: `ptr` maps `len` readable bytes until drop; callers access only buffers the
        // driver has returned, so the hardware is not writing them.
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
    pub fn bytes_mut(&mut self) -> &mut [u8] {
        // SAFETY: as in `bytes`, and `&mut self` makes the access exclusive.
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}
impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: `ptr` and `len` come from a successful mmap that is unmapped only here.
        unsafe { libc::munmap(self.ptr.cast(), self.len) };
    }
}
pub struct MappedBuffer {
    pub planes: Vec<Mapping>,
}

fn text(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}
fn cookie_time(cookie: u64) -> TimeVal {
    TimeVal {
        tv_sec: (cookie / 1_000_000) as libc::c_long,
        tv_usec: (cookie % 1_000_000) as libc::c_long,
    }
}

impl Device {
    pub fn open(path: &Path) -> io::Result<Self> {
        let name = CString::new(path.as_os_str().as_bytes())?;
        // SAFETY: `name` is a valid NUL-terminated path.
        let fd = unsafe {
            libc::open(
                name.as_ptr(),
                libc::O_RDWR | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            // SAFETY: `fd` is a freshly opened descriptor owned by nobody else.
            fd: unsafe { OwnedFd::from_raw_fd(fd) },
            path: path.into(),
        })
    }
    pub fn raw(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
    pub fn info(&self) -> io::Result<DeviceInfo> {
        let mut c: Capability = zeroed();
        // SAFETY: QUERYCAP takes a `v4l2_capability`.
        unsafe { ioctl(self.raw(), VIDIOC_QUERYCAP, &mut c) }?;
        let caps = if c.capabilities & CAP_DEVICE_CAPS != 0 {
            c.device_caps
        } else {
            c.capabilities
        };
        let m2m = caps & CAP_VIDEO_M2M_MPLANE != 0 && caps & CAP_STREAMING != 0;
        let formats = |kind| if m2m { self.formats(kind) } else { Vec::new() };
        Ok(DeviceInfo {
            path: self.path.clone(),
            driver: text(&c.driver),
            card: text(&c.card),
            m2m,
            consumes: formats(BUF_TYPE_VIDEO_OUTPUT_MPLANE),
            produces: formats(BUF_TYPE_VIDEO_CAPTURE_MPLANE),
        })
    }
    fn formats(&self, kind: u32) -> Vec<u32> {
        let mut out = Vec::new();
        for index in 0..64 {
            let mut d: FmtDesc = zeroed();
            d.index = index;
            d.kind = kind;
            // SAFETY: ENUM_FMT takes a `v4l2_fmtdesc`.
            if unsafe { ioctl(self.raw(), VIDIOC_ENUM_FMT, &mut d) }.is_err() {
                break;
            }
            out.push(d.pixelformat);
        }
        out
    }
    pub fn format(&self, kind: u32) -> io::Result<MplaneFormat> {
        let mut f: Format = zeroed();
        f.kind = kind;
        // SAFETY: G_FMT takes a `v4l2_format`.
        unsafe { ioctl(self.raw(), VIDIOC_G_FMT, &mut f) }?;
        Ok(from_format(&f))
    }
    /// Requests a format; the driver adjusts it and the adjusted format is returned.
    pub fn set_format(&self, kind: u32, request: &MplaneFormat) -> io::Result<MplaneFormat> {
        let mut p: PixFormatMplane = zeroed();
        p.width = request.width;
        p.height = request.height;
        p.pixelformat = request.pixelformat;
        p.field = FIELD_NONE;
        p.num_planes = request.planes.len().clamp(1, VIDEO_MAX_PLANES) as u8;
        for (slot, (bytesperline, sizeimage)) in p.plane_fmt.iter_mut().zip(&request.planes) {
            slot.bytesperline = *bytesperline;
            slot.sizeimage = *sizeimage;
        }
        let mut f: Format = zeroed();
        f.kind = kind;
        f.fmt.pix_mp = p;
        // SAFETY: S_FMT takes a `v4l2_format`.
        unsafe { ioctl(self.raw(), VIDIOC_S_FMT, &mut f) }?;
        Ok(from_format(&f))
    }
    /// Returns the number of buffers the driver allocated.
    pub fn request_buffers(&self, kind: u32, count: u32) -> io::Result<u32> {
        let mut r: RequestBuffers = zeroed();
        r.count = count;
        r.kind = kind;
        r.memory = MEMORY_MMAP;
        // SAFETY: REQBUFS takes a `v4l2_requestbuffers`.
        unsafe { ioctl(self.raw(), VIDIOC_REQBUFS, &mut r) }?;
        Ok(r.count)
    }
    pub fn map_buffers(
        &self,
        kind: u32,
        count: u32,
        planes: usize,
    ) -> io::Result<Vec<MappedBuffer>> {
        let mut out = Vec::with_capacity(count as usize);
        for index in 0..count {
            let mut p = [zeroed::<Plane>(); VIDEO_MAX_PLANES];
            let mut b: Buffer = zeroed();
            b.index = index;
            b.kind = kind;
            b.memory = MEMORY_MMAP;
            b.m.planes = p.as_mut_ptr();
            b.length = planes.clamp(1, VIDEO_MAX_PLANES) as u32;
            // SAFETY: QUERYBUF takes a `v4l2_buffer` whose plane array holds `length` entries.
            unsafe { ioctl(self.raw(), VIDIOC_QUERYBUF, &mut b) }?;
            let mut mappings = Vec::new();
            for plane in p.iter().take(b.length as usize) {
                let len = plane.length as usize;
                // SAFETY: the driver filled `mem_offset` for MMAP buffers.
                let offset = unsafe { plane.m.mem_offset };
                // SAFETY: maps a driver buffer of `len` bytes at the driver-chosen offset.
                let ptr = unsafe {
                    libc::mmap(
                        std::ptr::null_mut(),
                        len,
                        libc::PROT_READ | libc::PROT_WRITE,
                        libc::MAP_SHARED,
                        self.raw(),
                        offset as libc::off_t,
                    )
                };
                if ptr == libc::MAP_FAILED {
                    return Err(io::Error::last_os_error());
                }
                mappings.push(Mapping {
                    ptr: ptr.cast(),
                    len,
                });
            }
            out.push(MappedBuffer { planes: mappings });
        }
        Ok(out)
    }
    /// `planes` holds (bytesused, length) per plane.
    pub fn queue(
        &self,
        kind: u32,
        index: u32,
        planes: &[(u32, u32)],
        cookie: u64,
    ) -> io::Result<()> {
        let mut p = [zeroed::<Plane>(); VIDEO_MAX_PLANES];
        for (slot, (bytesused, length)) in p.iter_mut().zip(planes) {
            slot.bytesused = *bytesused;
            slot.length = *length;
        }
        let mut b: Buffer = zeroed();
        b.index = index;
        b.kind = kind;
        b.memory = MEMORY_MMAP;
        b.field = FIELD_NONE;
        b.timestamp = cookie_time(cookie);
        b.m.planes = p.as_mut_ptr();
        b.length = planes.len().clamp(1, VIDEO_MAX_PLANES) as u32;
        // SAFETY: QBUF takes a `v4l2_buffer` whose plane array holds `length` entries.
        unsafe { ioctl(self.raw(), VIDIOC_QBUF, &mut b) }
    }
    /// None when no buffer is ready, or when the queue was drained by a LAST buffer.
    pub fn dequeue(&self, kind: u32, planes: usize) -> io::Result<Option<Dequeued>> {
        let mut p = [zeroed::<Plane>(); VIDEO_MAX_PLANES];
        let mut b: Buffer = zeroed();
        b.kind = kind;
        b.memory = MEMORY_MMAP;
        b.m.planes = p.as_mut_ptr();
        b.length = planes.clamp(1, VIDEO_MAX_PLANES) as u32;
        // SAFETY: DQBUF takes a `v4l2_buffer` whose plane array holds `length` entries.
        match unsafe { ioctl(self.raw(), VIDIOC_DQBUF, &mut b) } {
            Ok(()) => Ok(Some(Dequeued {
                index: b.index,
                flags: b.flags,
                cookie: b.timestamp.tv_sec as u64 * 1_000_000 + b.timestamp.tv_usec as u64,
                bytesused: p[0].bytesused,
                data_offset: p[0].data_offset,
            })),
            Err(e) if matches!(e.raw_os_error(), Some(libc::EAGAIN) | Some(libc::EPIPE)) => {
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }
    pub fn stream(&self, kind: u32, on: bool) -> io::Result<()> {
        let mut k = kind as c_int;
        let request = if on {
            VIDIOC_STREAMON
        } else {
            VIDIOC_STREAMOFF
        };
        // SAFETY: STREAMON and STREAMOFF take an `int` buffer type.
        unsafe { ioctl(self.raw(), request, &mut k) }
    }
    pub fn set_control(&self, id: u32, value: i32) -> io::Result<()> {
        let mut c: Control = zeroed();
        c.id = id;
        c.value = value;
        // SAFETY: S_CTRL takes a `v4l2_control`.
        unsafe { ioctl(self.raw(), VIDIOC_S_CTRL, &mut c) }
    }
    pub fn control(&self, id: u32) -> io::Result<i32> {
        let mut c: Control = zeroed();
        c.id = id;
        // SAFETY: G_CTRL takes a `v4l2_control`.
        unsafe { ioctl(self.raw(), VIDIOC_G_CTRL, &mut c) }?;
        Ok(c.value)
    }
    pub fn subscribe(&self, event: u32) -> io::Result<()> {
        let mut s: EventSubscription = zeroed();
        s.kind = event;
        // SAFETY: SUBSCRIBE_EVENT takes a `v4l2_event_subscription`.
        unsafe { ioctl(self.raw(), VIDIOC_SUBSCRIBE_EVENT, &mut s) }
    }
    /// The type of the next pending event, if any.
    pub fn next_event(&self) -> io::Result<Option<u32>> {
        let mut e: Event = zeroed();
        // SAFETY: DQEVENT takes a `v4l2_event`.
        match unsafe { ioctl(self.raw(), VIDIOC_DQEVENT, &mut e) } {
            Ok(()) => Ok(Some(e.kind)),
            Err(e) if e.raw_os_error() == Some(libc::ENOENT) => Ok(None),
            Err(e) => Err(e),
        }
    }
    /// Selection rectangles use the single-planar buffer types, as the specification asks.
    pub fn selection(&self, kind: u32, target: u32) -> io::Result<Rect> {
        let mut s: Selection = zeroed();
        s.kind = kind;
        s.target = target;
        // SAFETY: G_SELECTION takes a `v4l2_selection`.
        unsafe { ioctl(self.raw(), VIDIOC_G_SELECTION, &mut s) }?;
        Ok(s.r)
    }
    pub fn set_selection(&self, kind: u32, target: u32, r: Rect) -> io::Result<()> {
        let mut s: Selection = zeroed();
        s.kind = kind;
        s.target = target;
        s.r = r;
        // SAFETY: S_SELECTION takes a `v4l2_selection`.
        unsafe { ioctl(self.raw(), VIDIOC_S_SELECTION, &mut s) }
    }
    pub fn set_frame_rate(&self, kind: u32, fps: u32) -> io::Result<()> {
        let mut p: StreamParm = zeroed();
        p.kind = kind;
        p.parm.output = OutputParm {
            capability: 0,
            outputmode: 0,
            timeperframe: Fract {
                numerator: 1,
                denominator: fps.max(1),
            },
            extendedmode: 0,
            writebuffers: 0,
            reserved: [0; 4],
        };
        // SAFETY: S_PARM takes a `v4l2_streamparm`.
        unsafe { ioctl(self.raw(), VIDIOC_S_PARM, &mut p) }
    }
}

fn from_format(f: &Format) -> MplaneFormat {
    // SAFETY: multi-planar queues report their format in the `pix_mp` member.
    let p = unsafe { f.fmt.pix_mp };
    let (width, height, pixelformat) = (p.width, p.height, p.pixelformat);
    let planes = p.plane_fmt;
    let count = (p.num_planes as usize).min(VIDEO_MAX_PLANES);
    MplaneFormat {
        width,
        height,
        pixelformat,
        planes: planes[..count]
            .iter()
            .map(|plane| (plane.bytesperline, plane.sizeimage))
            .collect(),
    }
}

/// Opens every `/dev/videoN` node in numeric order and records what it can do.
pub fn discover() -> Inventory {
    let mut nodes: Vec<(u32, PathBuf)> = std::fs::read_dir("/dev")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let number = name.strip_prefix("video")?.parse().ok()?;
            Some((number, entry.path()))
        })
        .collect();
    nodes.sort();
    Inventory {
        devices: nodes
            .into_iter()
            .filter_map(|(_, path)| Device::open(&path).and_then(|d| d.info()).ok())
            .collect(),
    }
}
