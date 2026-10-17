// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use crate::{require, Result};
use std::collections::BTreeMap;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldValue {
    Varint(u64),
    Bytes(Vec<u8>),
    Fixed64([u8; 8]),
    Fixed32([u8; 4]),
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fields(pub BTreeMap<u32, FieldValue>);
impl Fields {
    pub fn number(&self, id: u32) -> Result<u64> {
        match self.0.get(&id) {
            None => Ok(0),
            Some(FieldValue::Varint(n)) => Ok(*n),
            _ => Err("invalid_fields".into()),
        }
    }
    pub fn bytes(&self, id: u32) -> Result<&[u8]> {
        match self.0.get(&id) {
            Some(FieldValue::Bytes(n)) => Ok(n),
            Some(FieldValue::Fixed64(n)) => Ok(n),
            Some(FieldValue::Fixed32(n)) => Ok(n),
            _ => Err("missing_byte_field".into()),
        }
    }
    pub fn sized_bytes(&self, id: u32, n: usize) -> Result<&[u8]> {
        let b = self.bytes(id)?;
        require(b.len() == n, "invalid_field_size")?;
        Ok(b)
    }
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        for (&id, v) in &self.0 {
            require(id > 0 && id < 1 << 29, "invalid_field_number")?;
            match v {
                FieldValue::Varint(n) => {
                    varint(&mut out, (id as u64) << 3);
                    varint(&mut out, *n);
                }
                FieldValue::Bytes(b) => {
                    varint(&mut out, ((id as u64) << 3) | 2);
                    varint(&mut out, b.len() as u64);
                    out.extend_from_slice(b);
                }
                FieldValue::Fixed64(b) => {
                    varint(&mut out, ((id as u64) << 3) | 1);
                    out.extend_from_slice(b);
                }
                FieldValue::Fixed32(b) => {
                    varint(&mut out, ((id as u64) << 3) | 5);
                    out.extend_from_slice(b);
                }
            }
        }
        Ok(out)
    }
    pub fn decode(b: &[u8]) -> Result<Self> {
        require(b.len() <= super::frame::MAX_BYTES, "invalid_fields")?;
        let mut c = 0;
        let mut map = BTreeMap::new();
        while c < b.len() {
            let key = read_varint(b, &mut c)?;
            let id = key >> 3;
            require(id > 0 && id < 1 << 29, "invalid_field_number")?;
            let id = id as u32;
            require(!map.contains_key(&id), "duplicate_field")?;
            let wire = key & 7;
            let v = if wire == 0 {
                FieldValue::Varint(read_varint(b, &mut c)?)
            } else {
                let n = match wire {
                    1 => 8,
                    5 => 4,
                    2 => read_varint(b, &mut c)?,
                    _ => return Err("unsupported_wire_type".into()),
                };
                require(n <= (b.len() - c) as u64, "truncated_field")?;
                let s = &b[c..c + n as usize];
                c += n as usize;
                match wire {
                    1 => FieldValue::Fixed64(s.try_into()?),
                    5 => FieldValue::Fixed32(s.try_into()?),
                    _ => FieldValue::Bytes(s.to_vec()),
                }
            };
            map.insert(id, v);
        }
        Ok(Self(map))
    }
}
fn varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 128 {
        out.push((v as u8 & 127) | 128);
        v >>= 7;
    }
    out.push(v as u8);
}
fn read_varint(b: &[u8], pos: &mut usize) -> Result<u64> {
    let mut n = 0;
    for i in 0..10 {
        let v = *b.get(*pos).ok_or("truncated_varint")?;
        *pos += 1;
        require(i < 9 || v <= 1, "varint_overflow")?;
        n |= ((v & 127) as u64) << (7 * i);
        if v & 128 == 0 {
            return Ok(n);
        }
    }
    Err("varint_overflow".into())
}
