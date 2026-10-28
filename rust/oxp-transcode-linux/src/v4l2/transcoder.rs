// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

//! A stateful V4L2 decoder feeding a V4L2 H.264 or HEVC encoder. Decoded NV12 pictures are copied (and
//! resized when needed) between the two drivers' MMAP buffers by the CPU, which keeps the path
//! independent of DMA-buffer sharing support.

use super::{
    device::{Dequeued, Device, MappedBuffer},
    sys::*,
};
use crate::{
    annexb::{self, ParameterSets},
    negotiate::{self, ControlStep, DevicePair, MplaneFormat},
    nv12::{Nv12Layout, Nv12Scaler},
    pacing::{FramePacer, TimestampRing},
    EncodedFrame, Result, TranscodeConfig, Transcoder,
};
use std::{
    io,
    time::{Duration, Instant},
};

const OUTPUT: u32 = BUF_TYPE_VIDEO_OUTPUT_MPLANE;
const CAPTURE: u32 = BUF_TYPE_VIDEO_CAPTURE_MPLANE;
/// Encoded frames tolerated before the encoder has emitted SPS and PPS.
const UNCONFIGURED_LIMIT: usize = 30;

fn context(device: &Device, what: &str) -> impl FnOnce(io::Error) -> crate::Error {
    let prefix = format!("{} {what}", device.path.display());
    move |e| format!("{prefix}: {e}").into()
}

/// One driver queue and its mapped buffers.
struct Queue {
    kind: u32,
    planes: usize,
    buffers: Vec<MappedBuffer>,
    /// Buffers owned by user space, ready to be filled and queued.
    free: Vec<u32>,
}
impl Queue {
    fn allocate(device: &Device, kind: u32, count: u32, planes: usize, what: &str) -> Result<Self> {
        let n = device
            .request_buffers(kind, count)
            .map_err(context(device, &format!("{what} buffers")))?;
        if n == 0 {
            return Err(format!("{} allocated no {what} buffers", device.path.display()).into());
        }
        let buffers = device
            .map_buffers(kind, n, planes)
            .map_err(context(device, &format!("{what} mapping")))?;
        Ok(Self {
            kind,
            planes: planes.max(1),
            buffers,
            free: (0..n).rev().collect(),
        })
    }
    fn sizes(&self, index: u32, used: Option<u32>) -> [(u32, u32); VIDEO_MAX_PLANES] {
        let mut sizes = [(0, 0); VIDEO_MAX_PLANES];
        for (slot, plane) in sizes.iter_mut().zip(&self.buffers[index as usize].planes) {
            let length = plane.len() as u32;
            *slot = (used.unwrap_or(length), length);
        }
        sizes
    }
    /// Hands an empty buffer to the driver for it to fill.
    fn give(&self, device: &Device, index: u32) -> io::Result<()> {
        let sizes = self.sizes(index, Some(0));
        device.queue(self.kind, index, &sizes[..self.planes], 0)
    }
    fn give_all(&mut self, device: &Device) -> io::Result<()> {
        for index in 0..self.buffers.len() as u32 {
            self.give(device, index)?;
        }
        self.free.clear();
        Ok(())
    }
    /// Takes back every buffer the driver has finished with.
    fn reclaim(&mut self, device: &Device) -> io::Result<()> {
        while let Some(b) = device.dequeue(self.kind, self.planes)? {
            self.free.push(b.index);
        }
        Ok(())
    }
}

fn apply(device: &Device, steps: &[ControlStep]) -> Result<()> {
    for step in steps {
        let mut error = None;
        let accepted = step
            .values
            .iter()
            .any(|v| match device.set_control(step.id, *v) {
                Ok(()) => true,
                Err(e) => {
                    error = Some(e);
                    false
                }
            });
        if !accepted && step.required {
            let e = error.map_or("no value".into(), |e| e.to_string());
            return Err(format!("{} rejected {}: {e}", device.path.display(), step.name).into());
        }
    }
    Ok(())
}

/// The decoder's picture queue, set up once the stream's resolution is known.
struct Pictures {
    queue: Queue,
    scaler: Nv12Scaler,
}

pub struct V4l2Transcoder {
    // Buffers come first so they are unmapped before the descriptors close.
    bitstream: Queue,
    pictures: Option<Pictures>,
    raw: Queue,
    encoded: Queue,
    decoder: Device,
    encoder: Device,
    raw_format: u32,
    source_size: (u32, u32),
    crop: Option<oxp_core::model::ViewArea>,
    encoder_layout: Nv12Layout,
    /// Source parameter sets in Annex B, prefixed to key frames.
    header: Vec<u8>,
    /// A decoded picture waiting for a free encoder input buffer: (index, cookie).
    held: Option<(u32, u64)>,
    awaiting_key: bool,
    force_key: bool,
    timestamps: TimestampRing,
    pacer: FramePacer,
    parameters: ParameterSets,
    last_pts: i64,
    unconfigured: usize,
}

impl V4l2Transcoder {
    pub fn open(pair: &DevicePair, config: &TranscodeConfig) -> Result<Self> {
        let source = &config.source;
        let target = config.target;
        let header = annexb::parameter_sets(source.codec, &source.config)?;

        // The encoder goes first: an unsupported target fails before decoder memory is allocated.
        let encoder = Device::open(&pair.encoder.path)
            .map_err(|e| format!("{} open: {e}", pair.encoder.path.display()))?;
        let coded_target = encoder
            .set_format(
                CAPTURE,
                &MplaneFormat {
                    width: target.width,
                    height: target.height,
                    pixelformat: negotiate::codec_fourcc(target.profile.codec),
                    planes: vec![(
                        0,
                        negotiate::encoded_buffer_size(target.width, target.height),
                    )],
                },
            )
            .map_err(context(&encoder, "coded format"))?;
        if coded_target.pixelformat != negotiate::codec_fourcc(target.profile.codec) {
            return Err("encoder replaced requested codec".into());
        }
        let raw_format = encoder
            .set_format(
                OUTPUT,
                &MplaneFormat {
                    width: target.width,
                    height: target.height,
                    pixelformat: pair.raw_format,
                    planes: Vec::new(),
                },
            )
            .map_err(context(&encoder, "raw format"))?;
        if raw_format.pixelformat != pair.raw_format {
            return Err(format!(
                "{} replaced raw format {} with {}",
                encoder.path.display(),
                negotiate::fourcc_name(pair.raw_format),
                negotiate::fourcc_name(raw_format.pixelformat)
            )
            .into());
        }
        // Optional: drivers that align the raw height crop the coded picture to the target.
        let _ = encoder.set_selection(
            BUF_TYPE_VIDEO_OUTPUT,
            SEL_TGT_CROP,
            Rect {
                left: 0,
                top: 0,
                width: target.width,
                height: target.height,
            },
        );
        let _ = encoder.set_frame_rate(OUTPUT, target.fps);
        apply(&encoder, &negotiate::encoder_controls(&target)?)?;
        let encoder_layout = Nv12Layout::new(&raw_format, (target.width, target.height))?;
        let raw = Queue::allocate(
            &encoder,
            OUTPUT,
            negotiate::ENCODER_INPUT_BUFFERS,
            raw_format.planes.len(),
            "raw input",
        )?;
        let mut encoded = Queue::allocate(
            &encoder,
            CAPTURE,
            negotiate::ENCODER_OUTPUT_BUFFERS,
            1,
            "encoded output",
        )?;
        encoded
            .give_all(&encoder)
            .map_err(context(&encoder, "queue encoded output"))?;
        encoder
            .stream(OUTPUT, true)
            .map_err(context(&encoder, "start raw input"))?;
        encoder
            .stream(CAPTURE, true)
            .map_err(context(&encoder, "start encoded output"))?;

        let decoder = Device::open(&pair.decoder.path)
            .map_err(|e| format!("{} open: {e}", pair.decoder.path.display()))?;
        decoder
            .subscribe(EVENT_SOURCE_CHANGE)
            .map_err(context(&decoder, "source change events"))?;
        let fourcc = negotiate::codec_fourcc(source.codec);
        let coded = decoder
            .set_format(
                OUTPUT,
                &MplaneFormat {
                    width: source.width,
                    height: source.height,
                    pixelformat: fourcc,
                    planes: vec![(
                        0,
                        negotiate::bitstream_buffer_size(source.width, source.height),
                    )],
                },
            )
            .map_err(context(&decoder, "coded format"))?;
        if coded.pixelformat != fourcc {
            return Err(format!(
                "{} does not decode {}",
                decoder.path.display(),
                negotiate::fourcc_name(fourcc)
            )
            .into());
        }
        // A hint only; the decoder reports the real picture format after parsing the SPS.
        let _ = decoder.set_format(
            CAPTURE,
            &MplaneFormat {
                width: source.width,
                height: source.height,
                pixelformat: pair.raw_format,
                planes: Vec::new(),
            },
        );
        apply(&decoder, &negotiate::decoder_controls())?;
        let bitstream = Queue::allocate(
            &decoder,
            OUTPUT,
            negotiate::DECODER_INPUT_BUFFERS,
            1,
            "bitstream input",
        )?;
        decoder
            .stream(OUTPUT, true)
            .map_err(context(&decoder, "start bitstream input"))?;
        Ok(Self {
            bitstream,
            pictures: None,
            raw,
            encoded,
            decoder,
            encoder,
            raw_format: pair.raw_format,
            source_size: (source.width, source.height),
            crop: config.crop,
            encoder_layout,
            header,
            held: None,
            awaiting_key: true,
            force_key: false,
            timestamps: TimestampRing::new(64),
            pacer: FramePacer::new(target.fps),
            parameters: ParameterSets::new(target.profile.codec),
            last_pts: 0,
            unconfigured: 0,
        })
    }

    /// Allocates the picture queue once the decoder has parsed the stream's resolution.
    fn configure_pictures(&mut self) -> Result<()> {
        if self.pictures.take().is_some() {
            let _ = self.decoder.stream(CAPTURE, false);
            let _ = self.decoder.request_buffers(CAPTURE, 0);
        }
        self.held = None;
        let mut format = self
            .decoder
            .format(CAPTURE)
            .map_err(context(&self.decoder, "picture format"))?;
        if format.pixelformat != self.raw_format {
            format = self
                .decoder
                .set_format(
                    CAPTURE,
                    &MplaneFormat {
                        pixelformat: self.raw_format,
                        planes: Vec::new(),
                        ..format
                    },
                )
                .map_err(context(&self.decoder, "picture format"))?;
            if format.pixelformat != self.raw_format {
                return Err(format!(
                    "{} decodes to {}, not {}",
                    self.decoder.path.display(),
                    negotiate::fourcc_name(format.pixelformat),
                    negotiate::fourcc_name(self.raw_format)
                )
                .into());
            }
        }
        let visible = self
            .decoder
            .selection(BUF_TYPE_VIDEO_CAPTURE, SEL_TGT_COMPOSE)
            .ok()
            .filter(|r| r.width > 0 && r.height > 0)
            .map_or(self.source_size, |r| (r.width, r.height));
        let layout = Nv12Layout::new(&format, visible)?;
        let minimum = self
            .decoder
            .control(negotiate::CID_MIN_BUFFERS_FOR_CAPTURE)
            .ok();
        let mut queue = Queue::allocate(
            &self.decoder,
            CAPTURE,
            negotiate::capture_buffer_count(minimum),
            format.planes.len(),
            "picture",
        )?;
        queue
            .give_all(&self.decoder)
            .map_err(context(&self.decoder, "queue pictures"))?;
        self.decoder
            .stream(CAPTURE, true)
            .map_err(context(&self.decoder, "start pictures"))?;
        self.pictures = Some(Pictures {
            queue,
            scaler: Nv12Scaler::with_crop(
                layout,
                self.encoder_layout,
                self.crop
                    .unwrap_or_else(|| crate::geometry::crop(layout.width, layout.height, None)),
            )?,
        });
        Ok(())
    }

    fn handle_events(&mut self) -> Result<()> {
        while let Some(kind) = self
            .decoder
            .next_event()
            .map_err(context(&self.decoder, "event"))?
        {
            if kind == EVENT_SOURCE_CHANGE {
                self.configure_pictures()?;
            }
        }
        Ok(())
    }

    /// Moves decoded pictures into free encoder inputs, dropping those above the target rate.
    fn forward_pictures(&mut self) -> Result<()> {
        loop {
            let Some(pictures) = self.pictures.as_mut() else {
                return Ok(());
            };
            if self.held.is_none() {
                let Some(b) = self
                    .decoder
                    .dequeue(CAPTURE, pictures.queue.planes)
                    .map_err(context(&self.decoder, "dequeue picture"))?
                else {
                    return Ok(());
                };
                if b.flags & BUF_FLAG_LAST != 0 && b.bytesused == 0 {
                    // Drained for a resolution change; the source change event follows.
                    continue;
                }
                let paced = self
                    .timestamps
                    .get(b.cookie)
                    .is_some_and(|pts| self.pacer.accept(pts));
                if b.flags & BUF_FLAG_ERROR != 0 || b.bytesused == 0 || !(paced || self.force_key) {
                    pictures
                        .queue
                        .give(&self.decoder, b.index)
                        .map_err(context(&self.decoder, "requeue picture"))?;
                    continue;
                }
                self.held = Some((b.index, b.cookie));
            }
            let (index, cookie) = self.held.unwrap();
            let Some(slot) = self.raw.free.pop() else {
                return Ok(());
            };
            let source = &pictures.queue.buffers[index as usize].planes;
            let target = &mut self.raw.buffers[slot as usize].planes;
            let (first, rest) = target.split_first_mut().ok_or("raw buffer has no planes")?;
            pictures.scaler.convert(
                [
                    source.first().map_or(&[][..], |m| m.bytes()),
                    source.get(1).map_or(&[][..], |m| m.bytes()),
                ],
                [
                    first.bytes_mut(),
                    rest.first_mut()
                        .map_or(Default::default(), |m| m.bytes_mut()),
                ],
            )?;
            if std::mem::take(&mut self.force_key) {
                self.encoder
                    .set_control(negotiate::CID_FORCE_KEY_FRAME, 0)
                    .map_err(context(&self.encoder, "force key frame"))?;
            }
            let sizes = self.raw.sizes(slot, None);
            self.encoder
                .queue(OUTPUT, slot, &sizes[..self.raw.planes], cookie)
                .map_err(context(&self.encoder, "queue raw picture"))?;
            pictures
                .queue
                .give(&self.decoder, index)
                .map_err(context(&self.decoder, "requeue picture"))?;
            self.held = None;
        }
    }

    fn take_encoded(&mut self, out: &mut EncodedFrame) -> Result<bool> {
        loop {
            let Some(Dequeued {
                index,
                flags,
                cookie,
                bytesused,
                data_offset,
            }) = self
                .encoder
                .dequeue(CAPTURE, 1)
                .map_err(context(&self.encoder, "dequeue encoded"))?
            else {
                return Ok(false);
            };
            let split = {
                let data = self.encoded.buffers[index as usize].planes[0].bytes();
                let end = (bytesused as usize).min(data.len());
                let start = (data_offset as usize).min(end);
                (flags & BUF_FLAG_ERROR == 0 && end > start).then(|| {
                    self.parameters
                        .split_access_unit(&data[start..end], &mut out.data)
                })
            };
            self.encoded
                .give(&self.encoder, index)
                .map_err(context(&self.encoder, "requeue encoded"))?;
            let pts = self.timestamps.take(cookie);
            let Some(key) = split.transpose()? else {
                continue;
            };
            if out.data.is_empty() {
                // A header-only buffer: the parameter sets are cached for the next picture.
                continue;
            }
            let Some(config) = self.parameters.config() else {
                self.unconfigured += 1;
                if self.unconfigured > UNCONFIGURED_LIMIT {
                    return Err(
                        format!("{} emitted no SPS/PPS", self.encoder.path.display()).into(),
                    );
                }
                continue;
            };
            self.last_pts = pts.unwrap_or(self.last_pts);
            out.pts_us = self.last_pts;
            out.key_frame = key;
            out.config = Some(config);
            return Ok(true);
        }
    }

    /// Sleeps until either device has work or `timeout` passes.
    fn wait(&self, timeout: Duration) -> Result<()> {
        let mut fds = [
            libc::pollfd {
                fd: self.decoder.raw(),
                events: libc::POLLIN | libc::POLLOUT | libc::POLLPRI,
                revents: 0,
            },
            libc::pollfd {
                fd: self.encoder.raw(),
                events: libc::POLLIN | libc::POLLOUT,
                revents: 0,
            },
        ];
        let ms = timeout.as_millis().clamp(1, 1000) as libc::c_int;
        // SAFETY: `fds` is a live array of two pollfd entries.
        let r = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, ms) };
        if r < 0 {
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(format!("poll: {e}").into());
            }
        }
        // An idle memory-to-memory queue reports POLLERR at once; avoid spinning on it.
        if fds.iter().any(|f| f.revents & libc::POLLERR != 0) {
            std::thread::sleep(timeout.min(Duration::from_millis(1)));
        }
        Ok(())
    }
}

impl Transcoder for V4l2Transcoder {
    fn update_crop(&mut self, crop: Option<oxp_core::model::ViewArea>) -> Result<()> {
        if let Some(pictures) = &mut self.pictures {
            pictures.scaler.update_crop(crop)?;
        }
        self.crop = crop;
        Ok(())
    }

    fn submit(&mut self, data: &[u8], pts_us: i64, key_frame: bool) -> Result<bool> {
        if self.awaiting_key && !key_frame {
            return Ok(true);
        }
        self.bitstream
            .reclaim(&self.decoder)
            .map_err(context(&self.decoder, "reclaim bitstream"))?;
        let Some(index) = self.bitstream.free.pop() else {
            return Ok(false);
        };
        let prefix: &[u8] = if key_frame { &self.header } else { &[] };
        let plane = &mut self.bitstream.buffers[index as usize].planes[0];
        let length = plane.len() as u32;
        let written = match annexb::write_access_unit(prefix, data, plane.bytes_mut()) {
            Ok(n) => n as u32,
            Err(e) => {
                self.bitstream.free.push(index);
                return Err(e);
            }
        };
        let cookie = self.timestamps.insert(pts_us);
        self.decoder
            .queue(OUTPUT, index, &[(written, length)], cookie)
            .map_err(context(&self.decoder, "queue bitstream"))?;
        self.awaiting_key = false;
        Ok(true)
    }
    fn poll(&mut self, out: &mut EncodedFrame, timeout: Duration) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        loop {
            self.handle_events()?;
            self.bitstream
                .reclaim(&self.decoder)
                .map_err(context(&self.decoder, "reclaim bitstream"))?;
            self.raw
                .reclaim(&self.encoder)
                .map_err(context(&self.encoder, "reclaim raw input"))?;
            self.forward_pictures()?;
            if self.take_encoded(out)? {
                return Ok(true);
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(false);
            }
            self.wait(deadline - now)?;
        }
    }
    fn force_key_frame(&mut self) -> Result<()> {
        // Applied to the next picture queued to the encoder, so it takes effect in order.
        self.force_key = true;
        Ok(())
    }
    fn reset(&mut self) -> Result<()> {
        // Stopping the bitstream queue returns every queued buffer; pictures keep flowing and
        // decoding resumes at the next key frame, which carries the parameter sets.
        self.decoder
            .stream(OUTPUT, false)
            .map_err(context(&self.decoder, "stop bitstream input"))?;
        self.bitstream.free = (0..self.bitstream.buffers.len() as u32).rev().collect();
        self.decoder
            .stream(OUTPUT, true)
            .map_err(context(&self.decoder, "restart bitstream input"))?;
        self.awaiting_key = true;
        self.pacer.reset();
        Ok(())
    }
}
impl Drop for V4l2Transcoder {
    fn drop(&mut self) {
        let _ = self.decoder.stream(OUTPUT, false);
        let _ = self.decoder.stream(CAPTURE, false);
        let _ = self.encoder.stream(OUTPUT, false);
        let _ = self.encoder.stream(CAPTURE, false);
    }
}
