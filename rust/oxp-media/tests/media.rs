// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use oxp_media::{
    nal::{self, Format},
    ts::{self, TsOptions},
    AvcConfig, HevcConfig, TsMuxer, VideoCodec,
};
fn hex(s: &str) -> Vec<u8> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}
fn vector(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../spec/vectors")
        .join(format!("{name}.hex"));
    hex(&std::fs::read_to_string(path).unwrap())
}
fn config() -> AvcConfig {
    AvcConfig::parse(&vector("avc-baseline-640x480")).unwrap()
}
fn frame(size: usize, kind: u8) -> Vec<u8> {
    let mut out = (size as u32).to_be_bytes().to_vec();
    let mut n = vec![0x55; size];
    n[0] = 0x60 | kind;
    out.extend(n);
    out
}
fn pid(p: &[u8]) -> u16 {
    u16::from_be_bytes([p[1], p[2]]) & 0x1fff
}
fn control(p: &[u8]) -> u8 {
    (p[3] >> 4) & 3
}
fn start(p: &[u8]) -> bool {
    p[1] & 0x40 != 0
}
fn payload(p: &[u8]) -> &[u8] {
    &p[if control(p) & 2 == 0 {
        4
    } else {
        5 + p[4] as usize
    }..]
}
fn section(p: &[u8]) -> &[u8] {
    let b = payload(p);
    let s = 1 + b[0] as usize;
    let n = 3 + (u16::from_be_bytes([b[s + 1], b[s + 2]]) & 0xfff) as usize;
    &b[s..s + n]
}
fn pes(packets: &[Vec<u8>], p: u16) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = Vec::new();
    for b in packets
        .iter()
        .filter(|b| pid(b) == p && control(b) & 1 != 0)
    {
        if start(b) {
            out.push(Vec::new());
        }
        out.last_mut().unwrap().extend_from_slice(payload(b));
    }
    out
}
fn has_pcr(p: &[u8]) -> bool {
    control(p) & 2 != 0 && p[4] >= 7 && p[5] & 0x10 != 0
}
fn pcr(p: &[u8]) -> u64 {
    let base = ((p[6] as u64) << 25)
        | ((p[7] as u64) << 17)
        | ((p[8] as u64) << 9)
        | ((p[9] as u64) << 1)
        | (p[10] as u64 >> 7);
    base * 300 + (((p[10] & 1) as u64) << 8) + p[11] as u64
}
fn pts(p: &[u8]) -> u64 {
    (((p[9] >> 1 & 7) as u64) << 30)
        | ((p[10] as u64) << 22)
        | ((p[11] as u64 >> 1) << 15)
        | ((p[12] as u64) << 7)
        | (p[13] as u64 >> 1)
}
#[test]
fn nal_roundtrip_and_invalid_framing() {
    let bad = hex("000000016500");
    let mut lazy = nal::iter(&bad, Format::Avcc);
    assert_eq!(&[0x65], lazy.next().unwrap().unwrap());
    assert!(lazy.next().unwrap().is_err());
    assert!(lazy.next().is_none());
    let annex = hex("00000000016742c01e00000168ce06e20000000165abcdef0000");
    let avcc = nal::annex_b_to_avcc(&annex).unwrap();
    assert_eq!(
        nal::units(&avcc, Format::Avcc).unwrap(),
        vec![hex("6742c01e"), hex("68ce06e2"), hex("65abcdef")]
    );
    assert!(nal::is_key_frame(&avcc, VideoCodec::H264, Format::Avcc).unwrap());
    assert_eq!(
        nal::annex_b_to_avcc(&nal::avcc_to_annex_b(&avcc).unwrap()).unwrap(),
        avcc
    );
    for bad in [
        "000000",
        "00000000",
        "ffffffff65",
        "0000000265",
        "000000016500",
    ] {
        assert!(nal::units(&hex(bad), Format::Avcc).is_err());
    }
    for bad in ["00", "010203", "1200000165", "000001", "00000100000165"] {
        assert!(nal::units(&hex(bad), Format::AnnexB).is_err());
    }
    assert!(nal::units(&[], Format::Avcc).unwrap().is_empty());
    assert!(nal::units(&[], Format::AnnexB).unwrap().is_empty());
    for kind in 0..64 {
        let a = vec![0, 0, 0, 2, kind << 1, 1];
        assert_eq!(
            (16..=23).contains(&kind),
            nal::is_key_frame(&a, VideoCodec::H265, Format::Avcc).unwrap()
        );
    }
    assert!(nal::is_key_frame(&hex("0000000140"), VideoCodec::H265, Format::Avcc).is_err());
    assert!(nal::is_key_frame(&hex("000000016500"), VideoCodec::H264, Format::Avcc).is_err());
}
#[test]
fn configs_and_all_truncated_prefixes() {
    let a = config();
    assert_eq!(
        (640, 480, 4, 66, 0xc0, 30),
        (
            a.width,
            a.height,
            a.nal_length_size,
            a.profile,
            a.compatibility,
            a.level
        )
    );
    assert_eq!("avc1.42C01E", a.codec_string);
    assert_eq!(1, a.sps.len());
    assert_eq!(1, a.pps.len());
    let a = AvcConfig::parse(&vector("avc-high-1920x1080")).unwrap();
    assert_eq!((1920, 1080), (a.width, a.height));
    assert_eq!("avc1.640028", a.codec_string);
    assert!(a.sps_extensions.is_empty());
    let h = HevcConfig::parse(&vector("hevc-main-1920x1080")).unwrap();
    assert_eq!((1920, 1080, 4), (h.width, h.height, h.nal_length_size));
    assert_eq!("hvc1.1.6.L120.90", h.codec_string);
    assert_eq!(hex("900000000000"), h.constraints);
    for name in [
        "avc-baseline-640x480",
        "avc-high-1920x1080",
        "hevc-main-1920x1080",
    ] {
        let bytes = vector(name);
        for n in 0..bytes.len() {
            // The optional AVC extension may be absent or incomplete; decoders ignore it.
            if name == "avc-high-1920x1080" && n >= bytes.len() - 4 {
                continue;
            }
            let valid = if name.starts_with("avc") {
                AvcConfig::parse(&bytes[..n]).is_ok()
            } else {
                HevcConfig::parse(&bytes[..n]).is_ok()
            };
            assert!(!valid, "{name} truncated at {n}");
        }
    }
    let mut h = vector("hevc-main-1920x1080");
    h[1] = 0xe2;
    h[7] = 10;
    h[8] = 0;
    h[9] = 1;
    assert_eq!(
        "hvc1.C2.6.H120.90.0A.00.01",
        HevcConfig::parse(&h).unwrap().codec_string
    );
    h[6..12].fill(0);
    assert_eq!(
        "hvc1.C2.6.H120",
        HevcConfig::parse(&h).unwrap().codec_string
    );
    // A zero SPS count fails; reserved bits around it do not matter.
    for (offset, value) in [(0, 2), (4, 0xfe), (5, 0xe0), (8, 0x68)] {
        let mut b = vector("avc-baseline-640x480");
        b[offset] = value;
        assert!(AvcConfig::parse(&b).is_err());
    }
    for bad in ["6700000304", "67000003", "6742001e0000000000"] {
        assert!(AvcConfig::parse_sps(&hex(bad)).is_err());
    }
}
#[test]
fn ts_sizes_stuffing_and_pes_payload() {
    let mut packets = Vec::new();
    let mut pointers = Vec::new();
    {
        let mut m = TsMuxer::with_options(
            |p: &[u8; 188]| {
                pointers.push(p.as_ptr() as usize);
                packets.push(p.to_vec());
            },
            TsOptions {
                insert_aud: false,
                ..Default::default()
            },
        )
        .unwrap();
        for size in [163, 164, 165, 166, 167, 168, 169, 1000] {
            m.write_video(&frame(size, 1), &config(), 0, false).unwrap();
        }
    }
    assert!(pointers.iter().all(|p| *p == pointers[0]));
    for p in &packets {
        assert_eq!(188, p.len());
        assert_eq!(0x47, p[0]);
        assert_eq!(0, p[1] & 128);
        if control(p) == 3 && p[4] > 0 {
            let s = if p[5] & 16 != 0 { 12 } else { 6 };
            let end = 188 - payload(p).len();
            assert!(p[s..end].iter().all(|v| *v == 255));
        }
    }
    for (actual, n) in pes(&packets, 0x101)
        .iter()
        .zip([163, 164, 165, 166, 167, 168, 169, 1000])
    {
        assert_eq!(nal::avcc_to_annex_b(&frame(n, 1)).unwrap(), actual[14..]);
    }
    assert!(packets.iter().any(|p| control(p) == 3 && p[4] == 0));
}
#[test]
fn ts_tables_crc_and_configured_pids() {
    assert_eq!(0x0376e6e7, ts::crc32_mpeg2(b"123456789"));
    for audio in [false, true] {
        let mut packets = Vec::new();
        TsMuxer::with_options(
            |p: &[u8; 188]| packets.push(p.to_vec()),
            TsOptions {
                pmt_pid: 0x120,
                video_pid: 0x121,
                audio_pid: 0x122,
                program_number: 42,
                audio_enabled: audio,
                ..Default::default()
            },
        )
        .unwrap()
        .write_video(&frame(2, 5), &config(), 0, true)
        .unwrap();
        let pat = section(packets.iter().find(|p| pid(p) == 0).unwrap());
        let pmt = section(packets.iter().find(|p| pid(p) == 0x120).unwrap());
        assert_eq!(&[0, 42, 0xe1, 0x20], &pat[8..12]);
        assert_eq!(if audio { 26 } else { 21 }, pmt.len());
        assert_eq!(0x1b, pmt[12]);
        if audio {
            assert_eq!(0xf, pmt[17]);
        }
        for s in [pat, pmt] {
            assert_eq!(0, ts::crc32_mpeg2(s));
            assert_eq!(
                s.len() - 3,
                (u16::from_be_bytes([s[1], s[2]]) & 0xfff) as usize
            );
        }
    }
}
#[test]
fn ts_pts_markers_rounding_and_wrap() {
    for t in [0, 1, 12_345_678, 95_443_717_700, i64::MAX] {
        let mut packets = Vec::new();
        TsMuxer::new(|p: &[u8; 188]| packets.push(p.to_vec()))
            .write_video(&frame(2, 1), &config(), t, false)
            .unwrap();
        let p = pes(&packets, 0x101).remove(0);
        assert_eq!(hex("000001e00000808005"), p[..9]);
        let expected = ((t as u128 * 90_000 / 1_000_000) & 0x1ffffffff) as u64;
        assert_eq!(expected, pts(&p));
        assert_eq!(expected, ts::to_90khz(t).unwrap());
        assert_eq!(
            expected,
            pcr(packets.iter().find(|p| has_pcr(p)).unwrap()) / 300
        );
        for n in [9, 11, 13] {
            assert_eq!(1, p[n] & 1);
        }
    }
}
#[test]
fn ts_aud_parameter_sets_psi_and_continuity() {
    let mut packets = Vec::new();
    {
        let mut muxer = TsMuxer::new(|p: &[u8; 188]| packets.push(p.to_vec()));
        for i in 0..18 {
            let key = [0, 7, 16].contains(&i);
            muxer
                .write_video(
                    &frame(2, if key { 5 } else { 1 }),
                    &config(),
                    i * 33_333,
                    key,
                )
                .unwrap();
        }
    }
    for (i, p) in pes(&packets, 0x101).iter().enumerate() {
        let n = nal::units(&p[14..], Format::AnnexB).unwrap();
        let kinds: Vec<_> = n.iter().map(|n| n[0] & 31).collect();
        assert_eq!(
            if [0, 7, 16].contains(&i) {
                vec![9, 7, 8, 5]
            } else {
                vec![9, 1]
            },
            kinds
        );
    }
    let mut table_frames = Vec::new();
    let mut index = 0;
    for p in &packets {
        if pid(p) == 0 {
            table_frames.push(index);
        }
        if pid(p) == 0x101 && start(p) {
            index += 1;
        }
    }
    assert_eq!(vec![0, 7, 15, 16], table_frames);
    let mut packets = Vec::new();
    {
        let mut muxer = TsMuxer::with_options(
            |p: &[u8; 188]| packets.push(p.to_vec()),
            TsOptions {
                audio_enabled: true,
                psi_interval_frames: 1,
                ..Default::default()
            },
        )
        .unwrap();
        for i in 0..40 {
            muxer
                .write_video(&frame(1500, 1), &config(), i * 100_000, i % 10 == 0)
                .unwrap();
            muxer
                .write_audio(&hex("fff15080013ffc1122"), i * 100_000)
                .unwrap();
        }
    }
    let mut counters = std::collections::BTreeMap::new();
    for p in &packets {
        let n = counters.entry(pid(p)).or_insert(0u8);
        if control(p) & 1 != 0 {
            assert_eq!(*n, p[3] & 15);
            *n = (*n + 1) & 15;
        } else {
            assert_eq!((*n + 15) & 15, p[3] & 15);
        }
    }
    assert_eq!(
        vec![0, 0x100, 0x101, 0x102],
        counters.keys().copied().collect::<Vec<_>>()
    );
    assert_eq!(40, packets.iter().filter(|p| pid(p) == 0).count());
}
#[test]
fn ts_timestamp_jump_is_not_gap_filled() {
    let mut packets = Vec::new();
    {
        let mut m =
            TsMuxer::with_options(|p: &[u8; 188]| packets.push(p.to_vec()), Default::default())
                .unwrap();
        m.write_video(&frame(2, 1), &config(), 0, true).unwrap();
        m.write_video(&frame(2, 1), &config(), 3 * 3_600_000_000, false)
            .unwrap();
        m.write_video(&frame(2, 1), &config(), 3 * 3_600_000_000 + 100_000, false)
            .unwrap();
    }
    let clocks: Vec<_> = packets
        .iter()
        .filter(|p| has_pcr(p))
        .map(|p| pcr(p) / 27)
        .collect();
    let jump = 3 * 3_600_000_000u64;
    assert_eq!(vec![0, jump, jump + 40_000, jump + 80_000], clocks);
}
#[test]
fn ts_pcr_gap_fill_audio_and_invalid_inputs() {
    let mut packets = Vec::new();
    {
        let mut m = TsMuxer::with_options(
            |p: &[u8; 188]| packets.push(p.to_vec()),
            TsOptions {
                audio_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        for (t, key) in [(0, true), (33_333, false), (66_666, false), (100_001, true)] {
            m.write_video(&frame(2, 1), &config(), t, key).unwrap();
        }
        m.write_audio(&hex("fff15080013ffc1122"), 140_001).unwrap();
        m.write_video(&frame(2, 1), &config(), 300_001, false)
            .unwrap();
    }
    let clocks: Vec<_> = packets
        .iter()
        .filter(|p| has_pcr(p))
        .map(|p| pcr(p))
        .collect();
    assert_eq!(
        vec![0, 40_000, 80_000, 100_001, 140_001, 180_001, 220_001, 260_001, 300_001]
            .into_iter()
            .map(|t| t * 27)
            .collect::<Vec<_>>(),
        clocks
    );
    let mut packets = Vec::new();
    {
        let mut m = TsMuxer::with_options(
            |p: &[u8; 188]| packets.push(p.to_vec()),
            TsOptions {
                audio_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        m.write_audio(&hex("fff15080013ffc1122"), 1_000_000)
            .unwrap();
        assert!(m.write_audio(&[1], 0).is_err());
        assert!(m.write_audio(&hex("fff15080013ffc1122")[..8], 0).is_err());
        assert!(m
            .write_video(&hex("0000000865"), &config(), 0, true)
            .is_err());
        assert!(m.write_video(&frame(2, 1), &config(), -1, true).is_err());
    }
    let p = pes(&packets, 0x102).remove(0);
    assert_eq!(0xc0, p[3]);
    assert_eq!(90_000, pts(&p));
    assert_eq!(hex("fff15080013ffc1122"), p[14..]);
    assert!(TsMuxer::new(|_| {})
        .write_audio(&hex("fff15080013ffc1122"), 0)
        .is_err());
    for options in [
        TsOptions {
            pmt_pid: 0,
            ..Default::default()
        },
        TsOptions {
            video_pid: 0x100,
            ..Default::default()
        },
        TsOptions {
            psi_interval_frames: 0,
            ..Default::default()
        },
    ] {
        assert!(TsMuxer::with_options(|_| {}, options).is_err());
    }
}
#[test]
fn baseline_clip_all_frames_decode_with_ffprobe() {
    let ffprobe = std::path::Path::new("/opt/homebrew/bin/ffprobe");
    if !ffprobe.exists() {
        eprintln!("skipped: ffprobe absent");
        return;
    }
    let bytes =
        include_bytes!("../../../kotlin/core/src/test/resources/media/baseline-640x360.h264");
    let units = nal::units(bytes, Format::AnnexB).unwrap();
    let sps = units.iter().find(|n| n[0] & 31 == 7).unwrap();
    let pps = units.iter().find(|n| n[0] & 31 == 8).unwrap();
    let config = AvcConfig::parse(&AvcConfig::from_parameter_sets(sps, pps).unwrap()).unwrap();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    for n in units {
        let t = n[0] & 31;
        if t == 9 {
            frames.push(Vec::new());
        } else if t != 7 && t != 8 {
            let f = frames.last_mut().unwrap();
            f.extend_from_slice(&(n.len() as u32).to_be_bytes());
            f.extend_from_slice(n);
        }
    }
    assert_eq!(60, frames.len());
    let mut ts = Vec::new();
    {
        let mut muxer = TsMuxer::new(|p: &[u8; 188]| ts.extend_from_slice(p));
        for (i, f) in frames.iter().enumerate() {
            muxer
                .write_video(
                    f,
                    &config,
                    i as i64 * 1_000_000 / 30,
                    nal::is_key_frame(f, VideoCodec::H264, Format::Avcc).unwrap(),
                )
                .unwrap();
        }
    }
    let path = std::env::temp_dir().join(format!(
        "oxp-ts-{}-{}.ts",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path, ts).unwrap();
    let report = path.with_extension("report");
    let file = std::fs::File::create(&report).unwrap();
    let mut process = std::process::Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-count_frames",
            "-show_entries",
            "stream=codec_name,width,height,nb_read_frames",
            "-of",
            "default=noprint_wrappers=1",
        ])
        .arg(&path)
        .stdout(file)
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let status = loop {
        if let Some(s) = process.try_wait().unwrap() {
            break s;
        }
        if std::time::Instant::now() > deadline {
            process.kill().unwrap();
            process.wait().unwrap();
            panic!("ffprobe timeout");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    let text = std::fs::read_to_string(&report).unwrap();
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(report);
    assert!(status.success(), "{text}");
    let actual: std::collections::BTreeSet<_> = text.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(
        [
            "codec_name=h264",
            "width=640",
            "height=360",
            "nb_read_frames=60"
        ]
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>(),
        actual,
        "{text}"
    );
}

#[derive(Default)]
struct TestBits(Vec<bool>);
impl TestBits {
    fn bits(mut self, v: u32, n: usize) -> Self {
        for i in (0..n).rev() {
            self.0.push((v >> i) & 1 != 0);
        }
        self
    }
    fn ue(self, v: u32) -> Self {
        let code = v + 1;
        let n = (32 - code.leading_zeros()) as usize;
        self.bits(0, n - 1).bits(code, n)
    }
    fn se(self, v: i32) -> Self {
        self.ue(if v <= 0 {
            (-v as u32) * 2
        } else {
            v as u32 * 2 - 1
        })
    }
    fn nal(self, header: &[u8]) -> Vec<u8> {
        let mut out = header.to_vec();
        let mut zeros = 0;
        for chunk in self.0.chunks(8) {
            let mut v = 0;
            for (i, b) in chunk.iter().enumerate() {
                if *b {
                    v |= 1 << (7 - i);
                }
            }
            if zeros >= 2 && v <= 3 {
                out.push(3);
                zeros = 0;
            }
            out.push(v);
            zeros = if v == 0 { zeros + 1 } else { 0 };
        }
        out
    }
}
#[test]
fn avc_interlaced_monochrome_and_hevc_sublayer_separate_plane() {
    let sps = TestBits::default()
        .bits(100, 8)
        .bits(0, 8)
        .bits(40, 8)
        .ue(0)
        .ue(0)
        .ue(0)
        .ue(0)
        .bits(0, 1)
        .bits(0, 1)
        .ue(0)
        .ue(1)
        .bits(0, 1)
        .se(-1)
        .se(1)
        .ue(2)
        .se(-2)
        .se(2)
        .ue(1)
        .bits(0, 1)
        .ue(39)
        .ue(14)
        .bits(0, 1)
        .bits(1, 1)
        .bits(1, 1)
        .bits(1, 1)
        .ue(1)
        .ue(2)
        .ue(3)
        .ue(4)
        .bits(0, 1)
        .nal(&[0x67]);
    assert_eq!((637, 466), AvcConfig::parse_sps(&sps).unwrap());
    let mut bits = TestBits::default().bits(0, 4).bits(1, 3).bits(1, 1);
    for _ in 0..3 {
        bits = bits.bits(0, 32);
    }
    bits = bits.bits(1, 1).bits(1, 1);
    for _ in 0..7 {
        bits = bits.bits(0, 2);
    }
    for _ in 0..2 {
        bits = bits.bits(0, 32);
    }
    let sps = bits
        .bits(0, 24)
        .bits(30, 8)
        .ue(0)
        .ue(3)
        .bits(1, 1)
        .ue(640)
        .ue(480)
        .bits(1, 1)
        .ue(1)
        .ue(2)
        .ue(3)
        .ue(4)
        .nal(&[0x42, 1]);
    assert_eq!((637, 473), HevcConfig::parse_sps(&sps).unwrap());
}
#[test]
fn invalid_sps_dimensions_cropping_and_emulation_prevention() {
    let avc = |width, crop| {
        TestBits::default()
            .bits(66, 8)
            .bits(0, 8)
            .bits(30, 8)
            .ue(0)
            .ue(0)
            .ue(0)
            .ue(0)
            .ue(0)
            .bits(0, 1)
            .ue(width)
            .ue(0)
            .bits(1, 1)
            .bits(1, 1)
            .bits(1, 1)
            .ue(crop)
            .ue(0)
            .ue(0)
            .ue(0)
            .bits(0, 1)
            .nal(&[0x67])
    };
    assert!(AvcConfig::parse_sps(&avc(0, 8)).is_err());
    assert!(AvcConfig::parse_sps(&avc(i32::MAX as u32 - 1, 0)).is_err());
    let hevc = |width, crop| {
        let mut b = TestBits::default().bits(0, 4).bits(0, 3).bits(1, 1);
        for _ in 0..3 {
            b = b.bits(0, 32);
        }
        b.ue(0)
            .ue(1)
            .ue(width)
            .ue(480)
            .bits(1, 1)
            .ue(crop)
            .ue(0)
            .ue(0)
            .ue(0)
            .nal(&[0x42, 1])
    };
    for (w, c) in [(0, 0), (640, 321), (i32::MAX as u32, 0)] {
        assert!(HevcConfig::parse_sps(&hevc(w, c)).is_err());
    }
    let annex = hex("000000016700000301800000016588");
    let avcc = hex("00000006670000030180000000026588");
    assert_eq!(avcc, nal::annex_b_to_avcc(&annex).unwrap());
    assert_eq!(
        avcc,
        nal::annex_b_to_avcc(&nal::avcc_to_annex_b(&avcc).unwrap()).unwrap()
    );
}
