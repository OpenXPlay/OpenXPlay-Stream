// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

//! Every buffer on the Carlink RTP path stays bounded while its socket is stalled.
use oxp_clink::{
    media::{RtpPacketizer, RtpSender, RtpUnit, RtpWrite},
    Result,
};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
fn ts(n: usize) -> [u8; 188] {
    let mut p = [0u8; 188];
    for (i, b) in p.iter_mut().enumerate() {
        *b = (n + i) as u8;
    }
    p[0] = 0x47;
    p
}
/// The previous list-based packetizer, kept as an independent reference for the streamed layout.
fn reference(ts: &[[u8; 188]], pts_us: i64, first_sequence: u16, ssrc: u32) -> Vec<Vec<u8>> {
    let timestamp = oxp_media::ts::to_90khz(pts_us).unwrap() as u32;
    ts.chunks(7)
        .enumerate()
        .map(|(i, group)| {
            let mut p = ((12 + group.len() * 188) as u16).to_be_bytes().to_vec();
            p.extend_from_slice(&[0x80, 33]);
            p.extend_from_slice(&first_sequence.wrapping_add(i as u16).to_be_bytes());
            p.extend_from_slice(&timestamp.to_be_bytes());
            p.extend_from_slice(&ssrc.to_be_bytes());
            group.iter().for_each(|t| p.extend_from_slice(t));
            p
        })
        .collect()
}
fn fill(rtp: &mut RtpPacketizer, unit: &mut RtpUnit, pts_us: i64, packets: usize) {
    rtp.start(unit, pts_us).unwrap();
    (0..packets).for_each(|n| unit.append(&ts(n)));
    assert!(rtp.finish(unit));
}

#[test]
fn streamed_units_match_the_list_packetizer_and_continue_the_sequence() {
    let mut rtp = RtpPacketizer::new(0x434c4144, 65530);
    let mut unit = RtpUnit::default();
    let mut sequence = 65530u16;
    for call in 0..40usize {
        let packets: Vec<_> = (0..1 + call * 7 % 39).map(|n| ts(call * 41 + n)).collect();
        let pts = call as i64 * 33_333;
        rtp.start(&mut unit, pts).unwrap();
        packets.iter().for_each(|p| unit.append(p));
        assert!(rtp.finish(&unit));
        let expected = reference(&packets, pts, sequence, rtp.ssrc);
        assert_eq!(
            expected,
            unit.framed().map(<[u8]>::to_vec).collect::<Vec<_>>()
        );
        assert_eq!(expected.concat(), unit.as_bytes());
        assert_eq!(expected.len(), unit.packet_count());
        sequence = sequence.wrapping_add(expected.len() as u16);
    }
    let list: Vec<_> = (0..15).map(ts).collect();
    let owned: Vec<Vec<u8>> = list.iter().map(|p| p.to_vec()).collect();
    assert_eq!(
        reference(&list, 1_000_000, sequence, rtp.ssrc)
            .iter()
            .map(|p| p[2..].to_vec())
            .collect::<Vec<_>>(),
        rtp.packetize(&owned, 1_000_000).unwrap()
    );
}

#[test]
fn oversized_unit_is_rejected_without_growing_memory_and_leaves_no_rtp_gap() {
    // 2026-10-15: an hours-long timestamp jump made one muxer call emit hundreds of thousands of PCR
    // packets; a million packets is about eleven hours of 40 ms PCR fill.
    let mut rtp = RtpPacketizer::new(0x434c4144, 100);
    let mut unit = RtpUnit::default();
    rtp.start(&mut unit, 0).unwrap();
    let pcr = ts(0);
    for _ in 0..1_000_000 {
        unit.append(&pcr);
    }
    assert!(unit.overflowed());
    assert!(unit.len() <= RtpUnit::MAX_BYTES && unit.capacity() <= RtpUnit::MAX_BYTES);
    assert!(!rtp.finish(&unit));
    fill(&mut rtp, &mut unit, 40_000, 1);
    assert_eq!(
        100,
        u16::from_be_bytes([unit.as_bytes()[4], unit.as_bytes()[5]]),
        "discarded unit consumed no sequence"
    );
    assert!(
        unit.capacity() < 1024 * 1024,
        "an oversized buffer is released at the next unit"
    );
    // Growth stops at the bound even when it is not a power of two.
    let limit = RtpUnit::FRAMED_PACKET_BYTES * 3 + 100;
    let mut small = RtpUnit::new(limit).unwrap();
    rtp.start(&mut small, 0).unwrap();
    for _ in 0..1000 {
        small.append(&pcr);
    }
    assert!(small.overflowed() && small.len() <= limit && small.capacity() <= limit);
    assert_eq!(3, small.len() / RtpUnit::FRAMED_PACKET_BYTES);
    assert!(RtpUnit::new(RtpUnit::FRAMED_PACKET_BYTES - 1).is_err());
}

#[test]
fn units_reuse_one_buffer_through_the_sender() {
    let sender = Arc::new(RtpSender::new(Duration::from_secs(5)).unwrap());
    let active = Arc::new(AtomicBool::new(true));
    let writer = {
        let (sender, active) = (sender.clone(), active.clone());
        thread::spawn(move || sender.run(|| active.load(Ordering::Acquire)))
    };
    let written = Arc::new(AtomicUsize::new(0));
    let count = written.clone();
    let write: RtpWrite = Arc::new(move |p: &[u8]| -> Result<bool> {
        count.fetch_add(p.len(), Ordering::Relaxed);
        Ok(true)
    });
    let mut rtp = RtpPacketizer::default();
    let mut unit = RtpUnit::default();
    fill(&mut rtp, &mut unit, 0, 300);
    let buffer = unit.as_bytes().as_ptr();
    let capacity = unit.capacity();
    let mut expected = 0;
    for n in 0..200 {
        fill(&mut rtp, &mut unit, n * 33_333, 1 + (n as usize * 13) % 300);
        expected += unit.len();
        unit = sender.submit(unit, write.clone()).unwrap().unwrap();
        assert_eq!(
            (buffer, capacity),
            (unit.as_bytes().as_ptr(), unit.capacity())
        );
    }
    assert_eq!(expected, written.load(Ordering::Relaxed));
    assert_eq!((0, false), sender.retained());
    active.store(false, Ordering::Release);
    writer.join().unwrap();
}

#[test]
fn stalled_write_releases_the_media_worker_after_the_bounded_wait_and_recovers() {
    let sender = Arc::new(RtpSender::new(Duration::from_millis(50)).unwrap());
    let active = Arc::new(AtomicBool::new(true));
    let writer = {
        let (sender, active) = (sender.clone(), active.clone());
        thread::spawn(move || sender.run(|| active.load(Ordering::Acquire)))
    };
    let (release, gate) = mpsc::channel::<()>();
    let gate = Mutex::new(gate);
    let written = Arc::new(Mutex::new(Vec::new()));
    let sink = written.clone();
    let stalled: RtpWrite = Arc::new(move |p: &[u8]| -> Result<bool> {
        gate.lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| "never released")?;
        sink.lock().unwrap().extend_from_slice(p);
        Ok(true)
    });
    let mut rtp = RtpPacketizer::default();
    let mut unit = RtpUnit::default();
    fill(&mut rtp, &mut unit, 0, 20);
    let expected = unit.as_bytes().to_vec();
    let capacity = unit.capacity();
    let started = Instant::now();
    assert!(
        sender.submit(unit, stalled.clone()).unwrap().is_none(),
        "the write is still in flight"
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the media worker waited only boundedly"
    );
    // Minutes of media-loop iterations during the stall retain nothing beyond the one unit.
    for _ in 0..100_000 {
        assert!(sender.reclaim().unwrap().is_none());
    }
    assert_eq!((capacity, true), sender.retained());
    assert!(sender.take_recovered().is_none());
    let mut other = RtpUnit::default();
    fill(&mut rtp, &mut other, 1, 1);
    assert!(
        sender.submit(other, stalled.clone()).is_err(),
        "one write at a time"
    );
    for _ in 0..3 {
        release.send(()).unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut unit = loop {
        if let Some(unit) = sender.reclaim().unwrap() {
            break unit;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    };
    assert!(sender.take_recovered().unwrap() >= Duration::from_millis(50));
    assert_eq!(expected, *written.lock().unwrap());
    fill(&mut rtp, &mut unit, 33_333, 1);
    release.send(()).unwrap();
    let mut unit = sender.submit(unit, stalled).unwrap().unwrap();
    assert_eq!(expected.len() + 14 + 188, written.lock().unwrap().len());
    // A declined packet ends the unit, as the session does while not playing.
    fill(&mut rtp, &mut unit, 66_666, 15);
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let declining: RtpWrite = Arc::new(move |_: &[u8]| -> Result<bool> {
        Ok(counted.fetch_add(1, Ordering::Relaxed) < 1)
    });
    let mut unit = sender.submit(unit, declining).unwrap().unwrap();
    assert_eq!(2, calls.load(Ordering::Relaxed));
    // A socket failure on the writer surfaces on the media worker.
    fill(&mut rtp, &mut unit, 99_999, 1);
    let failing: RtpWrite = Arc::new(|_: &[u8]| -> Result<bool> { Err("closed".into()) });
    assert_eq!(
        "closed",
        sender.submit(unit, failing).err().unwrap().to_string()
    );
    assert!(sender.reclaim().unwrap().is_some());
    active.store(false, Ordering::Release);
    writer.join().unwrap();
}
