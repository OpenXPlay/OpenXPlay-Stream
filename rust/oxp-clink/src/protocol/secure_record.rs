// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use super::frame::MAX_BYTES;
use crate::{require, Result};
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes128Gcm,
};
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
/// Per-direction, per-channel state; retain the last 4096 authenticated records.
pub struct SecureRecord {
    key: [u8; 16],
    buffer: Vec<u8>,
    seen: VecDeque<[u8; 32]>,
}
impl SecureRecord {
    pub fn new(key: &[u8]) -> Result<Self> {
        Ok(Self {
            key: key.try_into().map_err(|_| "invalid_session_key")?,
            buffer: Vec::new(),
            seen: VecDeque::new(),
        })
    }
    pub fn pending_bytes(&self) -> usize {
        self.buffer.len()
    }
    pub fn seal(&self, plain: &[u8]) -> Result<Vec<u8>> {
        let mut iv = [0; 12];
        rand::rngs::OsRng.fill_bytes(&mut iv);
        self.seal_with_iv(plain, &iv)
    }
    pub fn seal_with_iv(&self, plain: &[u8], iv: &[u8; 12]) -> Result<Vec<u8>> {
        require(plain.len() <= MAX_BYTES - 40, "invalid_cipher_record")?;
        let encrypted = Aes128Gcm::new_from_slice(&self.key)
            .unwrap()
            .encrypt(iv.into(), plain)
            .map_err(|_| "cipher_failed")?;
        let mut out = Vec::new();
        out.extend_from_slice(&12u32.to_be_bytes());
        out.extend_from_slice(iv);
        out.extend_from_slice(&(encrypted.len() as u32).to_be_bytes());
        out.extend_from_slice(&encrypted);
        Ok(out)
    }
    fn decrypt(&mut self, b: &[u8]) -> Result<Vec<u8>> {
        let n = u32::from_be_bytes(b[..4].try_into()?) as usize;
        let fingerprint: [u8; 32] = Sha256::digest(&b[4..]).into();
        require(!self.seen.contains(&fingerprint), "replayed_cipher_record")?;
        // Java accepts IV lengths 12..32; use the corresponding GCM nonce type.
        use aes_gcm::{aead::consts::*, aes::Aes128, AesGcm};
        macro_rules! open {
            ($size:ty) => {
                AesGcm::<Aes128, $size>::new_from_slice(&self.key)
                    .unwrap()
                    .decrypt(
                        aes_gcm::Nonce::<$size>::from_slice(&b[4..4 + n]),
                        &b[n + 8..],
                    )
                    .map_err(|_| "cipher_authentication_failed")?
            };
        }
        let plain = match n {
            12 => open!(U12),
            13 => open!(U13),
            14 => open!(U14),
            15 => open!(U15),
            16 => open!(U16),
            17 => open!(U17),
            18 => open!(U18),
            19 => open!(U19),
            20 => open!(U20),
            21 => open!(U21),
            22 => open!(U22),
            23 => open!(U23),
            24 => open!(U24),
            25 => open!(U25),
            26 => open!(U26),
            27 => open!(U27),
            28 => open!(U28),
            29 => open!(U29),
            30 => open!(U30),
            31 => open!(U31),
            32 => open!(U32),
            _ => return Err("invalid_cipher_record".into()),
        };
        if self.seen.len() == 4096 {
            self.seen.pop_front();
        }
        self.seen.push_back(fingerprint);
        Ok(plain)
    }
    pub fn open(&mut self, b: &[u8]) -> Result<Vec<u8>> {
        require(self.buffer.is_empty(), "partial_record_pending")?;
        require(b.len() >= 8, "invalid_cipher_record")?;
        let n = u32::from_be_bytes(b[..4].try_into()?) as usize;
        require(
            (12..=32).contains(&n) && b.len() >= n + 8,
            "invalid_cipher_record",
        )?;
        let size = u32::from_be_bytes(b[n + 4..n + 8].try_into()?) as usize;
        require(
            (16..=MAX_BYTES).contains(&size) && b.len() == n + 8 + size,
            "invalid_cipher_record",
        )?;
        self.decrypt(b)
    }
    pub fn feed(&mut self, b: &[u8]) -> Result<Vec<Vec<u8>>> {
        require(
            self.buffer.len() + b.len() <= MAX_BYTES + 65576,
            "invalid_cipher_record",
        )?;
        self.buffer.extend_from_slice(b);
        let mut out = Vec::new();
        let mut pos = 0;
        while self.buffer.len() - pos >= 4 {
            let n = u32::from_be_bytes(self.buffer[pos..pos + 4].try_into()?) as usize;
            require((12..=32).contains(&n), "invalid_cipher_record")?;
            if self.buffer.len() - pos < n + 8 {
                break;
            }
            let size =
                u32::from_be_bytes(self.buffer[pos + n + 4..pos + n + 8].try_into()?) as usize;
            require((16..=MAX_BYTES).contains(&size), "invalid_cipher_record")?;
            let total = n + 8 + size;
            if self.buffer.len() - pos < total {
                break;
            }
            let record = self.buffer[pos..pos + total].to_vec();
            out.push(self.decrypt(&record)?);
            pos += total;
        }
        self.buffer.drain(..pos);
        Ok(out)
    }
}
