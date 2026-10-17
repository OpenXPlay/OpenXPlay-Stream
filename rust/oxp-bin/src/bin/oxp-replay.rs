// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use oxp_bin::*;
use oxp_wire::{Header, Kind, Message, Result};
use serde_json::json;
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};
fn run() -> Result<()> {
    let args = Args::parse(&[
        "--unix",
        "--tcp",
        "--file",
        "--fps",
        "--audio-media",
        "--audio-voice",
        "--audio-voice-every",
        "--mic-rate",
        "--mic-out",
    ])?;
    if args.get("--help").is_some() {
        println!("oxp-replay --file ANNEX_B_H264 [--fps 30] [--stdio | --unix PATH | --tcp PORT] [--audio-media FILE.aac] [--audio-voice FILE.wav] [--audio-voice-every SECONDS] [--mic-rate HZ] [--mic-out FILE.wav]");
        return Ok(());
    }
    let fps = args.get("--fps").unwrap_or("30").parse::<f64>()?;
    if !fps.is_finite() || !(0.001..=1000.0).contains(&fps) {
        return Err("FPS must be between 0.001 and 1000".into());
    }
    let (config, frames) = replay_frames(&std::fs::read(args.need("--file")?)?)?;
    let format = oxp_clink::model::VideoFormat::create(
        0,
        oxp_media::VideoCodec::H264,
        config.into(),
        Some(fps),
    )?;
    let mut audio = replay_audio::ReplayAudio::new(&args)?;
    let mut c = connect(&args, false)?;
    let peer = handshake(&mut c, "producer", "replay")?;
    let stdio = c.stdio;
    oxp_wire::write(
        &mut c.writer,
        &json_message(
            json!({"type":"input-capabilities","codecs":["H264"],"screens":[0],"touch":true,"keys":[0,1,2,3,4,5,6,7,8,9,10],"microphone":audio.mic_rate.is_some(),"keyFrameRequests":true,"reconfigure":"NONE"}),
        ),
    )?;
    oxp_wire::write(
        &mut c.writer,
        &json_message(json!({"type":"session","session":{"id":"1","phase":"ACTIVE"}})),
    )?;
    oxp_wire::write(&mut c.writer, &video_format(&format))?;
    let (tx, rx) = mpsc::sync_channel(256);
    let mut reader = std::mem::replace(&mut c.reader, Box::new(std::io::empty()));
    std::thread::spawn(move || loop {
        let m = oxp_wire::read(&mut reader);
        let done = !matches!(m, Ok(Some(_)));
        if tx.send(m).is_err() || done {
            break;
        }
    });
    let mut index = frames.iter().position(|f| f.1).unwrap();
    let mut seq = 0u32;
    let mut count = 0u64;
    let start = Instant::now();
    audio.start(&mut c.writer)?;
    let mut client = peer["client"].as_str().unwrap().to_owned();
    loop {
        let deadline = start + Duration::from_secs_f64(count as f64 / fps);
        loop {
            let now = start.elapsed();
            audio.tick(
                &mut c.writer,
                now.as_nanos().min(u64::MAX as u128) as u64,
                now.as_micros() as i64,
            )?;
            let wake = deadline.min(
                start
                    .checked_add(Duration::from_nanos(audio.next_deadline()))
                    .unwrap_or(deadline),
            );
            let received = if Instant::now() < deadline {
                match rx.recv_timeout(
                    wake.saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(50)),
                ) {
                    Ok(m) => Some(m),
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => return Ok(()),
                }
            } else {
                match rx.try_recv() {
                    Ok(m) => Some(m),
                    Err(mpsc::TryRecvError::Empty) => None,
                    Err(_) => return Ok(()),
                }
            };
            let Some(m) = received else {
                break;
            };
            let Some(m) = m? else {
                return Ok(());
            };
            let log = match m {
                Message::Json(v) => match v["type"].as_str() {
                    Some("client") => {
                        client = v["client"].as_str().unwrap().into();
                        None
                    }
                    Some("keyframe") => {
                        if v["screen"] == 0 {
                            index = (0..frames.len())
                                .map(|n| (index + n) % frames.len())
                                .find(|i| frames[*i].1)
                                .unwrap();
                        }
                        None
                    }
                    Some("capabilities") => Some(v),
                    Some("stats") => None,
                    Some("error") => return Err(format!("Peer error: {v}").into()),
                    _ => return Err("Unexpected consumer message".into()),
                },
                Message::Binary { header, payload } => match header.kind {
                    Kind::Touch => {
                        let contacts = oxp_wire::decode_touch(&payload)?;
                        Some(
                            json!({"type":"touch","screen":header.stream,"origin":{"outputId":peer["name"],"clientId":client},"contacts":contacts.iter().map(|c|json!({"id":c.id,"x":c.x,"y":c.y,"pressed":c.pressed})).collect::<Vec<_>>()}),
                        )
                    }
                    Kind::Key => {
                        let (code, down) = oxp_wire::decode_key(&payload)?;
                        Some(
                            json!({"type":"key","code":code,"down":down,"origin":{"outputId":peer["name"],"clientId":client}}),
                        )
                    }
                    Kind::Microphone => {
                        if audio.accepts(header.stream, &payload) {
                            audio.receive(
                                header.stream,
                                &payload,
                                peer["name"].as_str().unwrap(),
                                &client,
                            )?;
                        }
                        None
                    }
                    _ => return Err("Unexpected consumer binary kind".into()),
                },
            };
            if let Some(v) = log {
                if stdio {
                    event(v);
                } else {
                    println!("{v}");
                }
            }
        }
        let (payload, key) = &frames[index];
        let pts = (count as f64 * 1_000_000.0 / fps) as i64;
        oxp_wire::write(
            &mut c.writer,
            &Message::Binary {
                header: Header {
                    kind: Kind::Video,
                    stream: 0,
                    flags: if *key { 1 } else { 0 },
                    sequence: seq,
                    pts_us: pts,
                    sender_clock_us: start.elapsed().as_micros() as i64,
                },
                payload: payload.clone(),
            },
        )?;
        seq = seq.wrapping_add(1);
        count += 1;
        index = (index + 1) % frames.len();
    }
}
fn main() {
    if let Err(e) = run() {
        event(json!({"type":"error","message":e.to_string()}));
        std::process::exit(1);
    }
}
