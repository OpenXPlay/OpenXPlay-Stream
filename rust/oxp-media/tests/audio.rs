// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14.

use oxp_media::{Adts, AudioSpecificConfig};
#[test]
fn asc_standard_explicit_and_strict_features() {
    let c = AudioSpecificConfig::new(2, 48000, 2).unwrap();
    assert_eq!(c.build().unwrap(), [0x11, 0x90]);
    assert_eq!(AudioSpecificConfig::parse(&c.build().unwrap()).unwrap(), c);
    let c = AudioSpecificConfig::new(2, 12345, 1).unwrap();
    assert_eq!(c.build().unwrap().len(), 5);
    assert_eq!(AudioSpecificConfig::parse(&c.build().unwrap()).unwrap(), c);
    assert!(Adts::build_header(&c, 4).is_err());
    for bytes in [
        &[0x11][..],
        &[0x11, 0x91],
        &[0x11, 0x90, 0],
        &[0x29, 0x90],
        &[0x16, 0x90],
        &[0x11, 0x80],
    ] {
        assert!(AudioSpecificConfig::parse(bytes).is_err());
    }
}
#[test]
fn adts_split_mux_validation_and_rejections() {
    let c = AudioSpecificConfig::new(2, 48000, 2).unwrap();
    let mut b = Adts::build_header(&c, 4).unwrap().to_vec();
    b.extend([1, 2, 3, 4]);
    assert_eq!(Adts::validate_frame(&b).unwrap().config, c);
    let mut twice = b.clone();
    twice.extend(&b);
    assert_eq!(Adts::parse(&twice).unwrap().len(), 2);
    assert!(Adts::validate_frame(&twice).is_err());
    assert!(Adts::build_header(&c, 0).is_err());
    assert!(Adts::build_header(&c, 8185).is_err());
    for (i, value) in [(0, 0), (1, 0xf0), (6, 0xfd), (2, 0x7c)] {
        let mut invalid = b.clone();
        invalid[i] = value;
        assert!(Adts::parse(&invalid).is_err());
    }
    for size in 0..b.len() {
        assert!(Adts::parse_header(&b[..size]).is_err());
    }
}

#[test]
fn normalized_aac_pes_on_video_transport_clock() {
    let config = include_str!("../../../spec/vectors/avc-baseline-640x480.hex")
        .split_whitespace()
        .map(|v| u8::from_str_radix(v, 16).unwrap())
        .collect::<Vec<_>>();
    let config = oxp_media::AvcConfig::parse(&config).unwrap();
    let mut packets = Vec::new();
    let aac = AudioSpecificConfig::new(2, 48000, 2).unwrap();
    let au = [0x21, 0x10, 0x56, 0x78];
    let mut adts = Adts::build_header(&aac, au.len()).unwrap().to_vec();
    adts.extend(au);
    {
        let mut mux = oxp_media::TsMuxer::with_options(
            |p: &[u8; 188]| packets.push(*p),
            oxp_media::ts::TsOptions {
                audio_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        mux.write_video(&[0, 0, 0, 2, 0x65, 0x80], &config, 1_000_000, true)
            .unwrap();
        mux.write_audio(&adts, 1_021_333).unwrap();
    }
    let pid = |p: &[u8; 188]| ((p[1] as u16 & 31) << 8) | p[2] as u16;
    let p = packets
        .iter()
        .find(|p| pid(p) == 0x102 && p[1] & 0x40 != 0)
        .unwrap();
    let offset = if p[3] & 0x20 != 0 {
        5 + p[4] as usize
    } else {
        4
    };
    let pes = &p[offset..];
    assert_eq!(&pes[..4], &[0, 0, 1, 0xc0]);
    let n = u16::from_be_bytes([pes[4], pes[5]]) as usize;
    let t = &pes[9..14];
    let pts = ((t[0] as u64 >> 1 & 7) << 30)
        | ((t[1] as u64) << 22)
        | ((t[2] as u64 >> 1) << 15)
        | ((t[3] as u64) << 7)
        | (t[4] as u64 >> 1);
    assert_eq!(pts, oxp_media::ts::to_90khz(1_021_333).unwrap());
    assert!(pts - 90_000 <= 4500);
    assert_eq!(Adts::parse(&pes[14..6 + n]).unwrap()[0].data, au);
    let pmt = packets.iter().find(|p| pid(p) == 0x100).unwrap();
    assert!(pmt.windows(5).any(|b| b == [0x0f, 0xe1, 0x02, 0xf0, 0]));
}
