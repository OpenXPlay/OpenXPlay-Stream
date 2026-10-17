// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14.

use oxp_core::audio::*;
fn pcm(samples: impl IntoIterator<Item = i16>) -> Vec<u8> {
    samples.into_iter().flat_map(i16::to_le_bytes).collect()
}
#[test]
fn chunk_boundaries_exact_ratios_and_reset() {
    let data = pcm((0..4801).map(|n| ((n as f64 * 0.14).sin() * 12000.0) as i16));
    let mut whole = PcmResampler::new(48000, 1, 16000, 2).unwrap();
    let mut expected = whole.process(&data).unwrap();
    expected.extend(whole.finish().unwrap());
    let mut split = PcmResampler::new(48000, 1, 16000, 2).unwrap();
    let mut actual = Vec::new();
    for b in data.chunks(37) {
        actual.extend(split.process(b).unwrap());
    }
    actual.extend(split.finish().unwrap());
    assert_eq!(expected, actual);
    assert_eq!(actual.len(), 1601 * 4);
    assert!(split.process(&[]).is_err());
    assert!(split.finish().unwrap().is_empty());
    split.reset();
    assert!(split.process(&[1]).unwrap().is_empty());
    assert!(split.finish().is_err());
}
#[test]
fn stereo_identity_conversion_and_alias_rejection() {
    let mut r = PcmResampler::new(16000, 2, 16000, 1).unwrap();
    assert_eq!(
        r.process(&pcm([1000, -1000, 32767, 32767])).unwrap(),
        pcm([0, 32767])
    );
    let tone = |hz: f64| {
        pcm((0..4800).map(|n| {
            ((n as f64 * 2.0 * std::f64::consts::PI * hz / 48000.0).sin() * 16000.0) as i16
        }))
    };
    let rms = |hz| {
        let mut r = PcmResampler::new(48000, 1, 16000, 1).unwrap();
        let mut b = r.process(&tone(hz)).unwrap();
        b.extend(r.finish().unwrap());
        let s = b
            .as_chunks::<2>()
            .0
            .iter()
            .skip(100)
            .take(1400)
            .map(|&b| i16::from_le_bytes(b) as f64)
            .collect::<Vec<_>>();
        (s.iter().map(|v| v * v).sum::<f64>() / s.len() as f64).sqrt()
    };
    assert!(rms(1000.0) > 10000.0);
    assert!(rms(12000.0) < 100.0);
}
#[test]
fn mixer_saturates_pads_and_retains_filter_tail() {
    let mut m = PcmMixer::default();
    m.add_stream(1, 16000, 1).unwrap();
    m.add_stream(2, 48000, 2).unwrap();
    m.feed(1, &pcm(vec![20000; 320])).unwrap();
    m.feed(2, &pcm(vec![20000; 1920])).unwrap();
    m.end_stream(1).unwrap();
    m.end_stream(2).unwrap();
    let b = m.read_frame().unwrap();
    assert_eq!(b.len(), 640);
    assert!(b.as_chunks::<2>().0.contains(&[255, 127]));
    assert!(m.read_frame().is_none());
    m.add_stream(3, 16000, 1).unwrap();
    assert_eq!(m.read_frame().unwrap(), vec![0; 640]);
    m.feed(3, &pcm([1, 2])).unwrap();
    m.end_stream(3).unwrap();
    let b = m.read_frame().unwrap();
    assert_eq!(&b[..4], &pcm([1, 2]));
    assert_eq!(&b[4..], &vec![0; 636]);
}
#[test]
fn clock_remainders_threshold_overflow_and_reset() {
    let mut c = AudioClock::new(48000).unwrap();
    assert_eq!(c.timestamp(1024, 0).unwrap(), 0);
    assert_eq!(c.timestamp(1024, 121333).unwrap(), 21333); // Exactly 100 ms does not re-anchor.
    assert_eq!(c.timestamp(1024, 142667).unwrap(), 142667);
    c.reset();
    for n in 0..1000 {
        assert_eq!(
            c.timestamp(1024, n * 1024 * 1_000_000 / 48000).unwrap(),
            n * 1024 * 1_000_000 / 48000
        );
    }
    c.reset();
    assert!(c.timestamp(1024, i64::MAX).is_err());
    assert!(c.timestamp(0, 0).is_err());
}

#[test]
fn clock_preserves_one_second_bursts_across_runs() {
    let mut clock = AudioClock::new(16000).unwrap();
    for run in 0..3 {
        let start = run * 3_000_000;
        for n in 0..50 {
            assert_eq!(clock.timestamp(320, start).unwrap(), start + n * 20_000);
        }
        assert_eq!(
            clock.timestamp(320, start + 1_000_000).unwrap(),
            start + 1_000_000
        );
    }
}
#[test]
fn clock_asymmetric_boundaries_and_fraction_reset() {
    let mut clock = AudioClock::new(16000).unwrap();
    assert_eq!(clock.timestamp(320, 0).unwrap(), 0);
    assert_eq!(clock.timestamp(320, 120_000).unwrap(), 20_000);
    assert_eq!(clock.timestamp(320, -1_460_000).unwrap(), 40_000);
    assert_eq!(clock.timestamp(320, -1_440_001).unwrap(), -1_440_001);
    assert_eq!(clock.timestamp(320, 0).unwrap(), 0);
    let mut clock = AudioClock::new(44100).unwrap();
    for (arrival, expected) in [(0, 0), (200000, 200000), (200022, 200022), (200045, 200045)] {
        assert_eq!(clock.timestamp(1, arrival).unwrap(), expected);
    }
    clock.reset();
    assert_eq!(clock.timestamp(1, 42).unwrap(), 42);
}

#[test]
fn replacement_waits_for_ended_pcm_to_drain() {
    let mut mixer = PcmMixer::default();
    mixer.add_stream(1, 16000, 1).unwrap();
    mixer.feed(1, &pcm(vec![1234; 640])).unwrap();
    mixer.end_stream(1).unwrap();
    assert!(!mixer.stream_drained(1));
    assert!(mixer.add_stream(1, 8000, 1).is_err());
    for _ in 0..2 {
        assert_eq!(mixer.read_frame().unwrap(), pcm(vec![1234; 320]));
    }
    assert!(mixer.stream_drained(1));
    mixer.add_stream(1, 8000, 1).unwrap();
    assert!(!mixer.stream_drained(1));
}
