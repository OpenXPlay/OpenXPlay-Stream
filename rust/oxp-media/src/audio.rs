// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14.

use crate::Result;
const RATES: [u32; 13] = [
    96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
];
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioSpecificConfig {
    pub object_type: u8,
    pub sample_rate: u32,
    pub channel_configuration: u8,
    pub frequency_index: u8,
}
impl AudioSpecificConfig {
    pub fn frequency_index(rate: u32) -> u8 {
        RATES
            .iter()
            .position(|r| *r == rate)
            .map_or(15, |i| i as u8)
    }
    pub fn rate_for_index(index: u8) -> Result<u32> {
        RATES
            .get(index as usize)
            .copied()
            .ok_or("Reserved or explicit AAC frequency index")
    }
    pub fn new(object_type: u8, sample_rate: u32, channel_configuration: u8) -> Result<Self> {
        Self::with_index(
            object_type,
            sample_rate,
            channel_configuration,
            Self::frequency_index(sample_rate),
        )
    }
    fn with_index(
        object_type: u8,
        sample_rate: u32,
        channel_configuration: u8,
        frequency_index: u8,
    ) -> Result<Self> {
        if !(1..=4).contains(&object_type)
            || !(1..=0xffffff).contains(&sample_rate)
            || !(1..=7).contains(&channel_configuration)
            || (frequency_index != 15 && Self::rate_for_index(frequency_index)? != sample_rate)
        {
            return Err("Unsupported AAC configuration");
        }
        Ok(Self {
            object_type,
            sample_rate,
            channel_configuration,
            frequency_index,
        })
    }
    pub fn build(&self) -> Result<Vec<u8>> {
        Self::with_index(
            self.object_type,
            self.sample_rate,
            self.channel_configuration,
            self.frequency_index,
        )?;
        let mut bits = Vec::new();
        for (v, n) in [
            (self.object_type as u32, 5),
            (self.frequency_index as u32, 4),
        ] {
            for i in (0..n).rev() {
                bits.push((v >> i) & 1);
            }
        }
        if self.frequency_index == 15 {
            for i in (0..24).rev() {
                bits.push((self.sample_rate >> i) & 1);
            }
        }
        for i in (0..4).rev() {
            bits.push((self.channel_configuration as u32 >> i) & 1);
        }
        bits.extend([0, 0, 0]);
        let mut out = vec![0; bits.len() / 8];
        for (i, b) in bits.iter().enumerate() {
            out[i / 8] |= (*b as u8) << (7 - i % 8);
        }
        Ok(out)
    }
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let mut pos = 0;
        let mut read = |n: usize| -> Result<u32> {
            if pos + n > bytes.len() * 8 {
                return Err("Truncated AudioSpecificConfig");
            }
            let mut v = 0;
            for _ in 0..n {
                v = (v << 1) | ((bytes[pos / 8] >> (7 - pos % 8)) & 1) as u32;
                pos += 1;
            }
            Ok(v)
        };
        let object = read(5)? as u8;
        let index = read(4)? as u8;
        let rate = if index == 15 {
            read(24)?
        } else {
            Self::rate_for_index(index)?
        };
        let channels = read(4)? as u8;
        if read(3)? != 0 || pos != bytes.len() * 8 {
            return Err("Unsupported AAC flags or trailing bytes");
        }
        Self::with_index(object, rate, channels, index)
    }
}
#[derive(Clone, Debug)]
pub struct AdtsHeader {
    pub config: AudioSpecificConfig,
    pub frame_length: usize,
}
#[derive(Clone, Debug)]
pub struct AccessUnit {
    pub config: AudioSpecificConfig,
    pub data: Vec<u8>,
}
pub struct Adts;
impl Adts {
    pub fn build_header(config: &AudioSpecificConfig, size: usize) -> Result<[u8; 7]> {
        config.build()?;
        let index = AudioSpecificConfig::frequency_index(config.sample_rate);
        if !(1..=8184).contains(&size) || index == 15 {
            return Err("Invalid ADTS size or explicit rate");
        }
        let c = config.channel_configuration;
        let n = size + 7;
        Ok([
            255,
            0xf1,
            ((config.object_type - 1) << 6) | (index << 2) | (c >> 2),
            ((c & 3) << 6) | (n >> 11) as u8,
            (n >> 3) as u8,
            ((n & 7) << 5) as u8 | 0x1f,
            0xfc,
        ])
    }
    pub fn parse_header(bytes: &[u8]) -> Result<AdtsHeader> {
        if bytes.len() < 7 || bytes[0] != 255 || bytes[1] & 0xf6 != 0xf0 {
            return Err("Invalid ADTS sync or layer");
        }
        if bytes[1] & 1 == 0 || bytes[6] & 3 != 0 {
            return Err("Unsupported ADTS CRC or multiple blocks");
        }
        let index = (bytes[2] >> 2) & 15;
        let object = (bytes[2] >> 6) + 1;
        let c = ((bytes[2] & 1) << 2) | (bytes[3] >> 6);
        let config = AudioSpecificConfig::with_index(
            object,
            AudioSpecificConfig::rate_for_index(index)?,
            c,
            index,
        )?;
        let n = (((bytes[3] & 3) as usize) << 11)
            | ((bytes[4] as usize) << 3)
            | (bytes[5] as usize >> 5);
        if (bytes[1] & 8 != 0 && object == 4) || n <= 7 || n > bytes.len() {
            return Err("Invalid or truncated ADTS frame");
        }
        Ok(AdtsHeader {
            config,
            frame_length: n,
        })
    }
    pub fn validate_frame(bytes: &[u8]) -> Result<AdtsHeader> {
        let h = Self::parse_header(bytes)?;
        if h.frame_length != bytes.len() {
            return Err("Expected exactly one ADTS frame");
        }
        Ok(h)
    }
    pub fn parse(mut bytes: &[u8]) -> Result<Vec<AccessUnit>> {
        let mut out = Vec::new();
        while !bytes.is_empty() {
            let h = Self::parse_header(bytes)?;
            out.push(AccessUnit {
                config: h.config,
                data: bytes[7..h.frame_length].to_vec(),
            });
            bytes = &bytes[h.frame_length..];
        }
        Ok(out)
    }
}
