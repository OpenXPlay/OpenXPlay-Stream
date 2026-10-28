// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

//! The subset of the V4L2 user-space ABI used by the memory-to-memory codec path. Layouts follow
//! the kernel's documented structures; sizes are checked at compile time on 64-bit targets.

use libc::{c_int, c_long, c_ulong};
use std::mem::size_of;

pub const BUF_TYPE_VIDEO_CAPTURE: u32 = 1;
pub const BUF_TYPE_VIDEO_OUTPUT: u32 = 2;
pub const BUF_TYPE_VIDEO_CAPTURE_MPLANE: u32 = 9;
pub const BUF_TYPE_VIDEO_OUTPUT_MPLANE: u32 = 10;
pub const MEMORY_MMAP: u32 = 1;
pub const FIELD_NONE: u32 = 1;
pub const CAP_VIDEO_M2M_MPLANE: u32 = 0x0000_4000;
pub const CAP_STREAMING: u32 = 0x0400_0000;
pub const CAP_DEVICE_CAPS: u32 = 0x8000_0000;
pub const BUF_FLAG_ERROR: u32 = 0x0000_0040;
pub const BUF_FLAG_LAST: u32 = 0x0010_0000;
pub const EVENT_SOURCE_CHANGE: u32 = 5;
pub const SEL_TGT_CROP: u32 = 0x0000;
pub const SEL_TGT_COMPOSE: u32 = 0x0100;
pub const VIDEO_MAX_PLANES: usize = 8;

#[repr(C)]
pub struct Capability {
    pub driver: [u8; 16],
    pub card: [u8; 32],
    pub bus_info: [u8; 32],
    pub version: u32,
    pub capabilities: u32,
    pub device_caps: u32,
    pub reserved: [u32; 3],
}
#[repr(C)]
pub struct FrameSizeEnum {
    pub index: u32,
    pub pixel_format: u32,
    pub kind: u32,
    /// Discrete width/height, or min/max/step width followed by min/max/step height.
    pub size: [u32; 6],
    pub reserved: [u32; 2],
}
#[repr(C)]
pub struct FmtDesc {
    pub index: u32,
    pub kind: u32,
    pub flags: u32,
    pub description: [u8; 32],
    pub pixelformat: u32,
    pub mbus_code: u32,
    pub reserved: [u32; 3],
}
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct PlanePixFormat {
    pub sizeimage: u32,
    pub bytesperline: u32,
    pub reserved: [u16; 6],
}
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct PixFormatMplane {
    pub width: u32,
    pub height: u32,
    pub pixelformat: u32,
    pub field: u32,
    pub colorspace: u32,
    pub plane_fmt: [PlanePixFormat; VIDEO_MAX_PLANES],
    pub num_planes: u8,
    pub flags: u8,
    pub ycbcr_enc: u8,
    pub quantization: u8,
    pub xfer_func: u8,
    pub reserved: [u8; 7],
}
/// The kernel union also holds a structure with pointers, which gives it pointer alignment.
#[repr(C)]
pub union FormatUnion {
    pub pix_mp: PixFormatMplane,
    pub raw: [u8; 200],
    _align: [usize; 200 / size_of::<usize>()],
}
#[repr(C)]
pub struct Format {
    pub kind: u32,
    pub fmt: FormatUnion,
}
#[repr(C)]
pub struct RequestBuffers {
    pub count: u32,
    pub kind: u32,
    pub memory: u32,
    pub capabilities: u32,
    pub flags: u8,
    pub reserved: [u8; 3],
}
#[repr(C)]
#[derive(Clone, Copy)]
pub union PlaneMemory {
    pub mem_offset: u32,
    pub userptr: c_ulong,
    pub fd: i32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Plane {
    pub bytesused: u32,
    pub length: u32,
    pub m: PlaneMemory,
    pub data_offset: u32,
    pub reserved: [u32; 11],
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Timecode {
    pub kind: u32,
    pub flags: u32,
    pub frames: u8,
    pub seconds: u8,
    pub minutes: u8,
    pub hours: u8,
    pub userbits: [u8; 4],
}
/// The kernel's `struct timeval` as used by `v4l2_buffer` on this ABI.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TimeVal {
    pub tv_sec: c_long,
    pub tv_usec: c_long,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub union BufferMemory {
    pub offset: u32,
    pub userptr: c_ulong,
    pub planes: *mut Plane,
    pub fd: i32,
}
#[repr(C)]
pub struct Buffer {
    pub index: u32,
    pub kind: u32,
    pub bytesused: u32,
    pub flags: u32,
    pub field: u32,
    pub timestamp: TimeVal,
    pub timecode: Timecode,
    pub sequence: u32,
    pub memory: u32,
    pub m: BufferMemory,
    pub length: u32,
    pub reserved2: u32,
    pub request_fd: i32,
}
#[repr(C)]
pub struct Control {
    pub id: u32,
    pub value: i32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
}
#[repr(C)]
pub struct Selection {
    pub kind: u32,
    pub target: u32,
    pub flags: u32,
    pub r: Rect,
    pub reserved: [u32; 9],
}
#[repr(C)]
pub struct EventSubscription {
    pub kind: u32,
    pub id: u32,
    pub flags: u32,
    pub reserved: [u32; 5],
}
/// The event payload union contains a 64-bit control value, which sets its alignment.
#[repr(C)]
pub union EventData {
    pub data: [u8; 64],
    _align: [u64; 8],
}
#[repr(C)]
pub struct TimeSpec {
    pub tv_sec: c_long,
    pub tv_nsec: c_long,
}
#[repr(C)]
pub struct Event {
    pub kind: u32,
    pub u: EventData,
    pub pending: u32,
    pub sequence: u32,
    pub timestamp: TimeSpec,
    pub id: u32,
    pub reserved: [u32; 8],
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Fract {
    pub numerator: u32,
    pub denominator: u32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct OutputParm {
    pub capability: u32,
    pub outputmode: u32,
    pub timeperframe: Fract,
    pub extendedmode: u32,
    pub writebuffers: u32,
    pub reserved: [u32; 4],
}
#[repr(C)]
pub union StreamParmUnion {
    pub output: OutputParm,
    pub raw: [u8; 200],
}
#[repr(C)]
pub struct StreamParm {
    pub kind: u32,
    pub parm: StreamParmUnion,
}

#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(size_of::<Capability>() == 104);
    assert!(size_of::<FmtDesc>() == 64);
    assert!(size_of::<FrameSizeEnum>() == 44);
    assert!(size_of::<PixFormatMplane>() == 192);
    assert!(size_of::<Format>() == 208);
    assert!(size_of::<RequestBuffers>() == 20);
    assert!(size_of::<Plane>() == 64);
    assert!(size_of::<Buffer>() == 88);
    assert!(size_of::<Control>() == 8);
    assert!(size_of::<Selection>() == 64);
    assert!(size_of::<EventSubscription>() == 32);
    assert!(size_of::<Event>() == 136);
    assert!(size_of::<StreamParm>() == 204);
};

// The generic Linux ioctl encoding (arm, arm64, x86, riscv): direction, size, type, number.
const WRITE: c_ulong = 1;
const READ: c_ulong = 2;
const fn ioc(direction: c_ulong, number: c_ulong, size: usize) -> c_ulong {
    direction << 30 | (size as c_ulong) << 16 | (b'V' as c_ulong) << 8 | number
}
pub const VIDIOC_QUERYCAP: c_ulong = ioc(READ, 0, size_of::<Capability>());
pub const VIDIOC_ENUM_FMT: c_ulong = ioc(READ | WRITE, 2, size_of::<FmtDesc>());
pub const VIDIOC_ENUM_FRAMESIZES: c_ulong = ioc(READ | WRITE, 74, size_of::<FrameSizeEnum>());
pub const VIDIOC_G_FMT: c_ulong = ioc(READ | WRITE, 4, size_of::<Format>());
pub const VIDIOC_S_FMT: c_ulong = ioc(READ | WRITE, 5, size_of::<Format>());
pub const VIDIOC_REQBUFS: c_ulong = ioc(READ | WRITE, 8, size_of::<RequestBuffers>());
pub const VIDIOC_QUERYBUF: c_ulong = ioc(READ | WRITE, 9, size_of::<Buffer>());
pub const VIDIOC_QBUF: c_ulong = ioc(READ | WRITE, 15, size_of::<Buffer>());
pub const VIDIOC_DQBUF: c_ulong = ioc(READ | WRITE, 17, size_of::<Buffer>());
pub const VIDIOC_STREAMON: c_ulong = ioc(WRITE, 18, size_of::<c_int>());
pub const VIDIOC_STREAMOFF: c_ulong = ioc(WRITE, 19, size_of::<c_int>());
pub const VIDIOC_S_PARM: c_ulong = ioc(READ | WRITE, 22, size_of::<StreamParm>());
pub const VIDIOC_G_CTRL: c_ulong = ioc(READ | WRITE, 27, size_of::<Control>());
pub const VIDIOC_S_CTRL: c_ulong = ioc(READ | WRITE, 28, size_of::<Control>());
pub const VIDIOC_DQEVENT: c_ulong = ioc(READ, 89, size_of::<Event>());
pub const VIDIOC_SUBSCRIBE_EVENT: c_ulong = ioc(WRITE, 90, size_of::<EventSubscription>());
pub const VIDIOC_G_SELECTION: c_ulong = ioc(READ | WRITE, 94, size_of::<Selection>());
pub const VIDIOC_S_SELECTION: c_ulong = ioc(READ | WRITE, 95, size_of::<Selection>());

#[cfg(target_pointer_width = "64")]
const _: () = {
    // Spot checks against the values the kernel headers expand to on 64-bit targets.
    assert!(VIDIOC_QUERYCAP == 0x8068_5600);
    assert!(VIDIOC_S_FMT == 0xC0D0_5605);
    assert!(VIDIOC_QBUF == 0xC058_560F);
    assert!(VIDIOC_DQBUF == 0xC058_5611);
    assert!(VIDIOC_STREAMON == 0x4004_5612);
    assert!(VIDIOC_DQEVENT == 0x8088_5659);
};

/// Plain old data: all-zero bytes form a valid value.
///
/// # Safety
/// Implement only for types made of integers, arrays, unions and raw pointers.
pub unsafe trait Pod {}
macro_rules! pod {
    ($($t:ty),*) => { $(unsafe impl Pod for $t {})* };
}
pod!(
    Capability,
    FmtDesc,
    FrameSizeEnum,
    PixFormatMplane,
    Format,
    RequestBuffers,
    Plane,
    Buffer,
    Control,
    Selection,
    EventSubscription,
    Event,
    StreamParm
);

/// Zero-initialized ABI structure.
pub fn zeroed<T: Pod>() -> T {
    // SAFETY: `Pod` types accept all-zero bytes.
    unsafe { std::mem::zeroed() }
}

/// Issues one ioctl, retrying when a signal interrupts it.
///
/// # Safety
/// `request` must be an ioctl whose argument type is `T`.
pub unsafe fn ioctl<T>(fd: c_int, request: c_ulong, arg: &mut T) -> std::io::Result<()> {
    loop {
        // SAFETY: the caller pairs `request` with its argument type; `arg` is a live exclusive
        // reference for the duration of the call.
        let r = unsafe { libc::ioctl(fd, request as libc::Ioctl, arg as *mut T) };
        if r != -1 {
            return Ok(());
        }
        let e = std::io::Error::last_os_error();
        if e.kind() != std::io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}
