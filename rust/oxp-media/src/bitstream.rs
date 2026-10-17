// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use crate::Result;
pub(crate) struct Cursor<'a> {
    pub data: &'a [u8],
    pub pos: usize,
}
impl<'a> Cursor<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.bytes(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.data.len() - self.pos {
            return Err("Truncated configuration record");
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    pub fn end(&self) -> Result<()> {
        if self.pos == self.data.len() {
            Ok(())
        } else {
            Err("Unexpected configuration bytes")
        }
    }
    pub fn nal(&mut self) -> Result<Vec<u8>> {
        let n = self.u16()? as usize;
        Ok(self.bytes(n)?.to_vec())
    }
}
pub(crate) struct Bits {
    data: Vec<u8>,
    pos: usize,
}
impl Bits {
    pub fn new(data: &[u8], header: usize) -> Result<Self> {
        if data.len() <= header {
            return Err("Truncated parameter set");
        }
        let mut out = Vec::new();
        let mut zeros = 0;
        for i in header..data.len() {
            let b = data[i];
            if zeros >= 2 && b == 3 {
                if i + 1 >= data.len() || data[i + 1] > 3 {
                    return Err("Invalid emulation-prevention byte");
                }
                zeros = 0;
            } else {
                out.push(b);
                zeros = if b == 0 { zeros + 1 } else { 0 };
            }
        }
        Ok(Self { data: out, pos: 0 })
    }
    pub fn bits(&mut self, n: usize) -> Result<u32> {
        if n > 32 || self.pos + n > self.data.len() * 8 {
            return Err("Truncated bitstream");
        }
        let mut v = 0;
        for _ in 0..n {
            v = (v << 1) | ((self.data[self.pos / 8] >> (7 - self.pos % 8)) & 1) as u32;
            self.pos += 1;
        }
        Ok(v)
    }
    pub fn flag(&mut self) -> Result<bool> {
        Ok(self.bits(1)? != 0)
    }
    pub fn skip(&mut self, n: usize) -> Result<()> {
        if n > self.data.len() * 8 - self.pos {
            return Err("Truncated bitstream");
        }
        self.pos += n;
        Ok(())
    }
    pub fn ue(&mut self) -> Result<u32> {
        let mut n = 0;
        while !self.flag()? {
            n += 1;
            if n > 30 {
                return Err("Exp-Golomb value exceeds supported range");
            }
        }
        Ok((1u32 << n) - 1 + self.bits(n)?)
    }
    pub fn se(&mut self) -> Result<i64> {
        let v = self.ue()? as i64;
        Ok(if v & 1 == 0 { -v / 2 } else { v / 2 + 1 })
    }
}
pub(crate) fn dimensions(w: i64, h: i64) -> Result<(u32, u32)> {
    if w <= 0 || h <= 0 || w > i32::MAX as i64 || h > i32::MAX as i64 {
        Err("Invalid picture dimensions or cropping")
    } else {
        Ok((w as u32, h as u32))
    }
}
