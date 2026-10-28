// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

//! Transcodes an H.264 Annex B file with the host's V4L2 codecs, to check a board without a head
//! unit: `transcode_file INPUT.h264 OUTPUT.h264 [FPS] [SINK_CODEC]` (defaults 60, avc1.42E01F).

#[cfg(target_os = "linux")]
fn main() -> oxp_transcode_linux::Result<()> {
    use oxp_core::model::{VideoCodec, VideoFormat};
    use oxp_media::{
        nal::{self, Format},
        transcode_policy::TranscodeDecision,
        transcode_policy::{decide, describe, CodecProfile, SinkVideo, SourceVideo},
        AvcConfig,
    };
    use oxp_transcode_linux::{
        v4l2::V4l2Factory, EncodedFrame, TranscodeConfig, TranscoderFactory,
    };
    use std::{
        io::Write,
        sync::Arc,
        time::{Duration, Instant},
    };

    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: transcode_file INPUT.h264 OUTPUT.h264 [FPS] [SINK_CODEC]");
        std::process::exit(2);
    }
    let fps: f64 = args.get(3).map(|v| v.parse()).transpose()?.unwrap_or(60.0);
    let sink = SinkVideo::from_codec_strings([args.get(4).map_or("avc1.42E01F", String::as_str)]);
    let input = std::fs::read(&args[1])?;

    // A slice whose first_mb_in_slice is zero (leading bit set) starts a new picture.
    let (mut sps, mut pps) = (None, None);
    let mut pictures: Vec<(Vec<u8>, bool)> = Vec::new();
    for unit in nal::iter(&input, Format::AnnexB) {
        let unit = unit?;
        match unit[0] & 31 {
            7 => sps = Some(unit.to_vec()),
            8 => pps = Some(unit.to_vec()),
            kind @ (1 | 5) => {
                if pictures.is_empty() || unit.get(1).is_some_and(|b| b & 0x80 != 0) {
                    pictures.push((Vec::new(), false));
                }
                let (data, key) = pictures.last_mut().unwrap();
                data.extend_from_slice(&(unit.len() as u32).to_be_bytes());
                data.extend_from_slice(unit);
                *key |= kind == 5;
            }
            _ => {}
        }
    }
    let config = AvcConfig::from_parameter_sets(
        sps.as_deref().ok_or("input has no SPS")?,
        pps.as_deref().ok_or("input has no PPS")?,
    )?;
    let source = Arc::new(VideoFormat::create(
        0,
        VideoCodec::H264,
        config.into(),
        Some(fps),
    )?);
    let factory = V4l2Factory::default();
    eprintln!("{}", factory.describe());
    let video = SourceVideo {
        profile: CodecProfile::parse(&source.codec_string).ok_or("unknown input profile")?,
        width: source.width,
        height: source.height,
        fps: Some(fps),
    };
    let decision = decide(&video, &sink, factory.support().as_ref());
    eprintln!("{}", describe(&video, &decision));
    let TranscodeDecision::Transcode { target, .. } = decision else {
        return Err("the policy does not transcode this input for this sink".into());
    };
    let started = Instant::now();
    let mut transcoder = factory.open(&TranscodeConfig {
        source,
        target,
        crop: None,
    })?;
    eprintln!("opened in {} ms", started.elapsed().as_millis());

    let mut output = std::io::BufWriter::new(std::fs::File::create(&args[2])?);
    let mut frame = EncodedFrame::default();
    let (mut received, mut keys, mut first, mut profile) = (0usize, 0usize, None, None);
    let mut write = |frame: &EncodedFrame| -> oxp_transcode_linux::Result<()> {
        if frame.key_frame {
            let c = AvcConfig::parse(frame.config.as_deref().ok_or("frame without config")?)?;
            profile = Some(c.codec_string.clone());
            for set in c.sps.iter().chain(&c.pps) {
                output.write_all(&[0, 0, 0, 1])?;
                output.write_all(set)?;
            }
        }
        for unit in nal::iter(&frame.data, Format::Avcc) {
            output.write_all(&[0, 0, 0, 1])?;
            output.write_all(unit?)?;
        }
        Ok(())
    };
    let mut on_frame = |frame: &EncodedFrame| -> oxp_transcode_linux::Result<()> {
        received += 1;
        keys += frame.key_frame as usize;
        first.get_or_insert_with(|| started.elapsed());
        write(frame)
    };
    let interval = 1_000_000.0 / fps;
    let begin = Instant::now();
    for (i, (data, key)) in pictures.iter().enumerate() {
        let pts = (i as f64 * interval) as i64;
        while !transcoder.submit(data, pts, *key)? {
            if transcoder.poll(&mut frame, Duration::from_millis(20))? {
                on_frame(&frame)?;
            }
        }
        while transcoder.poll(&mut frame, Duration::ZERO)? {
            on_frame(&frame)?;
        }
        if i == pictures.len() / 2 {
            transcoder.force_key_frame()?;
        }
    }
    let mut idle = Instant::now();
    while idle.elapsed() < Duration::from_millis(1500) {
        if transcoder.poll(&mut frame, Duration::from_millis(50))? {
            on_frame(&frame)?;
            idle = Instant::now();
        }
    }
    output.flush()?;
    let elapsed = begin.elapsed().as_secs_f64();
    eprintln!(
        "input {} pictures, output {received} frames ({keys} key) in {elapsed:.2} s = {:.1} input fps; first output after {} ms; output {}",
        pictures.len(),
        pictures.len() as f64 / elapsed,
        first.map_or(0, |d| d.as_millis()),
        profile.unwrap_or_else(|| "none".into())
    );
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("transcode_file needs Linux V4L2 codecs");
    std::process::exit(1);
}
