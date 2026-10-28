// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

//! NV12 frame copies between decoder and encoder buffers whose strides, plane heights and
//! picture sizes differ.

use crate::{
    negotiate::{MplaneFormat, PIX_FMT_NV12, PIX_FMT_NV12M},
    Result,
};

/// Where one plane's rows start inside a mapped buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaneLayout {
    /// Index of the V4L2 plane holding the rows.
    pub plane: usize,
    pub offset: usize,
    pub stride: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Nv12Layout {
    /// Visible picture size.
    pub width: u32,
    pub height: u32,
    /// Allocated rows of the luma plane; drivers align it, for example to 32 lines.
    pub buffer_height: u32,
    pub luma: PlaneLayout,
    pub chroma: PlaneLayout,
}
impl Nv12Layout {
    /// Single-plane NV12 stores the interleaved chroma right after the luma plane, whose height
    /// is the format height; NV12M keeps it in a second plane.
    pub fn new(format: &MplaneFormat, visible: (u32, u32)) -> Result<Self> {
        let (width, height) = (visible.0.min(format.width), visible.1.min(format.height));
        if width == 0 || height == 0 || format.height == 0 {
            return Err("empty NV12 format".into());
        }
        let rows = format.height as usize;
        let chroma_rows = rows.div_ceil(2);
        let stride = |plane: usize| -> Result<usize> {
            let (bytesperline, _) = *format
                .planes
                .get(plane)
                .ok_or("missing NV12 plane format")?;
            let stride = if bytesperline == 0 {
                format.width
            } else {
                bytesperline
            } as usize;
            if stride < width.next_multiple_of(2) as usize {
                return Err("NV12 stride is narrower than the picture".into());
            }
            Ok(stride)
        };
        let size = |plane: usize| format.planes[plane].1 as usize;
        let layout = match format.pixelformat {
            PIX_FMT_NV12 => {
                let s = stride(0)?;
                if size(0) < s * (rows + chroma_rows) {
                    return Err("NV12 buffer is smaller than its planes".into());
                }
                Self {
                    width,
                    height,
                    buffer_height: format.height,
                    luma: PlaneLayout {
                        plane: 0,
                        offset: 0,
                        stride: s,
                    },
                    chroma: PlaneLayout {
                        plane: 0,
                        offset: s * rows,
                        stride: s,
                    },
                }
            }
            PIX_FMT_NV12M => {
                let (luma, chroma) = (stride(0)?, stride(1)?);
                if size(0) < luma * rows || size(1) < chroma * chroma_rows {
                    return Err("NV12M buffer is smaller than its planes".into());
                }
                Self {
                    width,
                    height,
                    buffer_height: format.height,
                    luma: PlaneLayout {
                        plane: 0,
                        offset: 0,
                        stride: luma,
                    },
                    chroma: PlaneLayout {
                        plane: 1,
                        offset: 0,
                        stride: chroma,
                    },
                }
            }
            _ => return Err("raw format is not NV12".into()),
        };
        Ok(layout)
    }
    fn chroma_rows(&self) -> usize {
        self.height.div_ceil(2) as usize
    }
    fn chroma_buffer_rows(&self) -> usize {
        self.buffer_height.div_ceil(2) as usize
    }
    /// Bytes each plane must hold for a copy to stay in bounds.
    fn required(&self, plane: usize) -> usize {
        let mut need = 0;
        if self.luma.plane == plane {
            need = self.luma.offset + self.luma.stride * self.buffer_height as usize;
        }
        if self.chroma.plane == plane {
            need = need.max(self.chroma.offset + self.chroma.stride * self.chroma_buffer_rows());
        }
        need
    }
}

/// Copies or nearest-neighbour scales a picture from one layout into another. Index tables are
/// built once per session; `convert` does not allocate.
pub struct Nv12Scaler {
    src: Nv12Layout,
    dst: Nv12Layout,
    luma_x: Vec<u32>,
    luma_y: Vec<u32>,
    chroma_x: Vec<u32>,
    chroma_y: Vec<u32>,
    same_width: bool,
}
impl Nv12Scaler {
    pub fn new(src: Nv12Layout, dst: Nv12Layout) -> Self {
        Self::with_crop(src, dst, crate::geometry::crop(src.width, src.height, None))
            .expect("full picture crop")
    }
    pub fn with_crop(
        src: Nv12Layout,
        dst: Nv12Layout,
        area: oxp_core::model::ViewArea,
    ) -> Result<Self> {
        area.validate()?;
        if area.canvas_width != src.width
            || area.canvas_height != src.height
            || area.width < 2
            || area.height < 2
        {
            return Err("crop does not match decoded picture".into());
        }
        let map = |to: usize, from: usize| -> Vec<u32> {
            (0..to).map(|i| (i * from / to) as u32).collect()
        };
        let pairs = |w: u32| w.div_ceil(2) as usize;
        Ok(Self {
            luma_x: map(dst.width as usize, area.width as usize)
                .into_iter()
                .map(|x| x + area.x)
                .collect(),
            luma_y: map(dst.height as usize, area.height as usize)
                .into_iter()
                .map(|y| y + area.y)
                .collect(),
            chroma_x: map(pairs(dst.width), pairs(area.width))
                .into_iter()
                .map(|x| x + area.x / 2)
                .collect(),
            chroma_y: map(dst.chroma_rows(), area.height.div_ceil(2) as usize)
                .into_iter()
                .map(|y| y + area.y / 2)
                .collect(),
            same_width: area.x == 0 && area.width == dst.width,
            src,
            dst,
        })
    }
    pub fn update_crop(&mut self, area: Option<oxp_core::model::ViewArea>) -> Result<()> {
        *self = Self::with_crop(
            self.src,
            self.dst,
            area.unwrap_or_else(|| crate::geometry::crop(self.src.width, self.src.height, None)),
        )?;
        Ok(())
    }
    /// `src` and `dst` hold the mapped planes; the second entry is empty for single-plane NV12.
    pub fn convert(&self, src: [&[u8]; 2], dst: [&mut [u8]; 2]) -> Result<()> {
        for plane in 0..2 {
            if src[plane].len() < self.src.required(plane)
                || dst[plane].len() < self.dst.required(plane)
            {
                return Err("NV12 buffer is smaller than its layout".into());
            }
        }
        let (s, d) = (self.src, self.dst);
        // Luma: one byte per pixel.
        let row = d.width as usize;
        rows(
            &src[s.luma.plane][s.luma.offset..],
            s.luma.stride,
            &mut dst[d.luma.plane][d.luma.offset..],
            d.luma.stride,
            row,
            &self.luma_y,
            (!self.same_width).then_some((&self.luma_x[..], 1)),
            d.buffer_height as usize,
        );
        // Chroma: one interleaved Cb/Cr pair per two pixels.
        let row = d.width.div_ceil(2) as usize * 2;
        rows(
            &src[s.chroma.plane][s.chroma.offset..],
            s.chroma.stride,
            &mut dst[d.chroma.plane][d.chroma.offset..],
            d.chroma.stride,
            row,
            &self.chroma_y,
            (!self.same_width).then_some((&self.chroma_x[..], 2)),
            d.chroma_buffer_rows(),
        );
        Ok(())
    }
}

/// Copies the rows selected by `y_map`, optionally resampling columns in `unit`-byte groups, and
/// repeats the last row into the alignment padding so encoders never read stale lines.
#[allow(clippy::too_many_arguments)]
fn rows(
    src: &[u8],
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    row: usize,
    y_map: &[u32],
    x_map: Option<(&[u32], usize)>,
    buffer_rows: usize,
) {
    for (y, &sy) in y_map.iter().enumerate() {
        let from = &src[sy as usize * src_stride..];
        let to = &mut dst[y * dst_stride..y * dst_stride + row];
        match x_map {
            None => to.copy_from_slice(&from[..row]),
            Some((map, unit)) => {
                for (chunk, &sx) in to.chunks_exact_mut(unit).zip(map) {
                    let at = sx as usize * unit;
                    chunk.copy_from_slice(&from[at..at + unit]);
                }
            }
        }
    }
    if let Some(last) = y_map.len().checked_sub(1) {
        for y in y_map.len()..buffer_rows {
            dst.copy_within(last * dst_stride..last * dst_stride + row, y * dst_stride);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn venus_format(width: u32, height: u32) -> MplaneFormat {
        // Venus aligns NV12 lines to 128 bytes and planes to 32 rows.
        let stride = width.next_multiple_of(128);
        let rows = height.next_multiple_of(32);
        MplaneFormat {
            width,
            height: rows,
            pixelformat: PIX_FMT_NV12,
            planes: vec![(
                stride,
                (stride * rows * 3 / 2 + 8192).next_multiple_of(4096),
            )],
        }
    }
    /// Fills a picture where every luma byte encodes its coordinates and chroma pairs differ.
    fn picture(layout: &Nv12Layout, buffer: &mut [Vec<u8>; 2]) {
        for y in 0..layout.height as usize {
            for x in 0..layout.width as usize {
                buffer[layout.luma.plane][layout.luma.offset + y * layout.luma.stride + x] =
                    (x * 7 + y * 13) as u8;
            }
        }
        for y in 0..layout.height.div_ceil(2) as usize {
            for x in 0..layout.width.div_ceil(2) as usize {
                let at = layout.chroma.offset + y * layout.chroma.stride + x * 2;
                buffer[layout.chroma.plane][at] = (x + y * 3) as u8;
                buffer[layout.chroma.plane][at + 1] = 200u8.wrapping_sub((x + y) as u8);
            }
        }
    }
    fn buffers(format: &MplaneFormat) -> [Vec<u8>; 2] {
        [
            vec![0; format.planes[0].1 as usize],
            vec![0; format.planes.get(1).map_or(0, |p| p.1 as usize)],
        ]
    }
    fn convert(scaler: &Nv12Scaler, src: &[Vec<u8>; 2], dst: &mut [Vec<u8>; 2]) -> Result<()> {
        let [d0, d1] = dst;
        scaler.convert([&src[0], &src[1]], [d0, d1])
    }

    #[test]
    fn venus_layout_places_chroma_after_aligned_rows() {
        let layout = Nv12Layout::new(&venus_format(1280, 720), (1280, 720)).unwrap();
        assert_eq!(
            (layout.width, layout.height, layout.buffer_height),
            (1280, 720, 736)
        );
        assert_eq!(layout.chroma.offset, 1280 * 736);
        let nv12m = MplaneFormat {
            width: 64,
            height: 32,
            pixelformat: PIX_FMT_NV12M,
            planes: vec![(64, 2048), (64, 1024)],
        };
        let layout = Nv12Layout::new(&nv12m, (64, 30)).unwrap();
        assert_eq!(
            layout.chroma,
            PlaneLayout {
                plane: 1,
                offset: 0,
                stride: 64
            }
        );
        let mut short = nv12m.clone();
        short.planes[1].1 = 512;
        assert!(Nv12Layout::new(&short, (64, 32)).is_err());
        let mut narrow = venus_format(1280, 720);
        narrow.planes[0].0 = 640;
        assert!(Nv12Layout::new(&narrow, (1280, 720)).is_err());
        let mut yuyv = venus_format(64, 64);
        yuyv.pixelformat = crate::negotiate::fourcc(b"YUYV");
        assert!(Nv12Layout::new(&yuyv, (64, 64)).is_err());
        assert!(Nv12Layout::new(&MplaneFormat::default(), (0, 0)).is_err());
    }

    #[test]
    fn same_size_copy_handles_strides_and_pads_alignment_rows() {
        let src_format = venus_format(1280, 720);
        let dst_format = MplaneFormat {
            width: 1280,
            height: 720,
            pixelformat: PIX_FMT_NV12M,
            planes: vec![(1408, 1408 * 720), (1408, 1408 * 360)],
        };
        let src_layout = Nv12Layout::new(&src_format, (1280, 720)).unwrap();
        let dst_layout = Nv12Layout::new(&dst_format, (1280, 720)).unwrap();
        let mut src = buffers(&src_format);
        picture(&src_layout, &mut src);
        let mut dst = buffers(&dst_format);
        convert(&Nv12Scaler::new(src_layout, dst_layout), &src, &mut dst).unwrap();
        for (y, x) in [(0, 0), (719, 1279), (300, 641)] {
            assert_eq!(dst[0][y * 1408 + x], (x * 7 + y * 13) as u8);
        }
        assert_eq!(dst[1][359 * 1408 + 2 * 639], (639 + 359 * 3) as u8);
        assert_eq!(
            dst[1][359 * 1408 + 2 * 639 + 1],
            200u8.wrapping_sub((639 + 359) as u8)
        );

        // Back into a Venus encoder buffer: padding rows repeat the last visible line.
        let back_layout = Nv12Layout::new(&src_format, (1280, 720)).unwrap();
        let mut back = buffers(&src_format);
        convert(&Nv12Scaler::new(dst_layout, back_layout), &dst, &mut back).unwrap();
        let luma = |b: &Vec<u8>, y: usize| b[y * 1280..y * 1280 + 1280].to_vec();
        assert_eq!(luma(&back[0], 719), luma(&back[0], 735));
        assert_eq!(luma(&back[0], 10), luma(&src[0], 10));
        let chroma = 1280 * 736;
        assert_eq!(
            back[0][chroma + 367 * 1280..chroma + 368 * 1280],
            back[0][chroma + 359 * 1280..chroma + 360 * 1280]
        );
    }

    #[test]
    fn downscale_samples_nearest_pixels() {
        let src_format = venus_format(1920, 1080);
        let dst_format = venus_format(1280, 720);
        let src_layout = Nv12Layout::new(&src_format, (1920, 1080)).unwrap();
        let dst_layout = Nv12Layout::new(&dst_format, (1280, 720)).unwrap();
        let mut src = buffers(&src_format);
        picture(&src_layout, &mut src);
        let mut dst = buffers(&dst_format);
        convert(&Nv12Scaler::new(src_layout, dst_layout), &src, &mut dst).unwrap();
        // Destination (x, y) samples source (x * 3 / 2, y * 3 / 2).
        for (y, x) in [(0, 0), (1, 1), (719, 1279), (400, 333)] {
            let (sx, sy) = (x * 3 / 2, y * 3 / 2);
            assert_eq!(dst[0][y * 1280 + x], (sx * 7 + sy * 13) as u8, "{x},{y}");
        }
        let chroma = 1280 * 736;
        let (cx, cy) = (100usize, 50usize);
        assert_eq!(
            dst[0][chroma + cy * 1280 + cx * 2],
            (cx * 3 / 2 + cy * 3 / 2 * 3) as u8
        );
        let mut tiny = buffers(&dst_format);
        tiny[0].truncate(1000);
        assert!(convert(&Nv12Scaler::new(src_layout, dst_layout), &src, &mut tiny).is_err());
    }
    #[test]
    fn crop_stretches_luma_and_chroma_without_sampling_the_borders() {
        let sf = venus_format(16, 12);
        let df = venus_format(8, 8);
        let sl = Nv12Layout::new(&sf, (16, 12)).unwrap();
        let dl = Nv12Layout::new(&df, (8, 8)).unwrap();
        let mut src = buffers(&sf);
        picture(&sl, &mut src);
        let mut dst = buffers(&df);
        let area = oxp_core::model::ViewArea {
            x: 4,
            y: 2,
            width: 8,
            height: 4,
            canvas_width: 16,
            canvas_height: 12,
        };
        let mut scaler = Nv12Scaler::with_crop(sl, dl, area).unwrap();
        convert(&scaler, &src, &mut dst).unwrap();
        for (x, y) in [(0, 0), (7, 7), (3, 4)] {
            let (sx, sy) = (4 + x, 2 + y / 2);
            assert_eq!(dst[0][y * dl.luma.stride + x], (sx * 7 + sy * 13) as u8);
        }
        assert_eq!(
            dst[0][dl.chroma.offset],
            src[0][sl.chroma.offset + sl.chroma.stride + 4]
        );
        scaler
            .update_crop(Some(oxp_core::model::ViewArea { x: 2, y: 4, ..area }))
            .unwrap();
        convert(&scaler, &src, &mut dst).unwrap();
        assert_eq!(dst[0][0], (2 * 7 + 4 * 13) as u8);
        assert!(
            Nv12Scaler::with_crop(sl, dl, oxp_core::model::ViewArea { width: 20, ..area }).is_err()
        );
    }
}
