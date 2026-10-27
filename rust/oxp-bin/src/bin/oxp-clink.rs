// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use oxp_bin::clink_support::*;
use oxp_bin::*;
#[cfg(test)]
use oxp_clink::model::*;
use oxp_clink::*;
use oxp_wire::Result;
#[cfg(test)]
use oxp_wire::{Kind, Message};
use serde_json::json;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{sync_channel, RecvTimeoutError},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
fn run() -> Result<()> {
    let args = Args::parse(&[
        "--unix",
        "--tcp",
        "--link",
        "--head-unit",
        "--local",
        "--pin",
        "--pin-file",
        "--state-dir",
        "--port",
    ])?;
    if args.get("--help").is_some() {
        println!(
            "oxp-clink [--stdio | --unix PATH | --tcp PORT] --link static --head-unit IPV4 --local IPV4 [--pin DIGITS | --pin-file PATH] --state-dir DIR [--port auth=N,control=N,media=N,cert=N,rtsp=N,uibc=N,rtp=N]\nPIN entry confirms pairing automatically (auto_confirm)."
        );
        return Ok(());
    }
    if args.need("--link")? != "static" {
        return Err("Only --link static is supported".into());
    }
    if args.get("--pin").is_some() && args.get("--pin-file").is_some() {
        return Err("Choose --pin or --pin-file".into());
    }
    if let Some(pin) = args.get("--pin") {
        if pin.is_empty() || !pin.bytes().all(|b| b.is_ascii_digit()) {
            return Err("PIN must contain digits".into());
        }
    }
    let ports = parse_ports(args.get("--port"))?;
    let link = StaticClinkLink(ClinkAddresses::new(
        args.need("--head-unit")?.parse()?,
        args.need("--local")?.parse()?,
    ));
    let pin_stopped = Arc::new(AtomicBool::new(false));
    let output = ClinkOutput::new(
        Arc::new(link),
        Arc::new(Pin {
            digits: args.get("--pin").map(str::to_owned),
            file: args.get("--pin-file").map(str::to_owned),
            stopped: pin_stopped.clone(),
            auto_confirm: true,
        }),
        Arc::new(FileTrust::new(std::path::Path::new(
            args.need("--state-dir")?,
        ))?),
    )?
    .with_options(oxp_clink::session::SessionOptions {
        ports,
        ..Default::default()
    })?
    .with_listener(Arc::new(Listener));
    let mut c = connect(&args, true)?;
    handshake(&mut c, "consumer", "clink")?;
    let (tx, rx) = sync_channel(256);
    let mut writer = std::mem::replace(&mut c.writer, Box::new(std::io::sink()));
    let stopped = Arc::new(AtomicBool::new(false));
    let failed = Arc::new(AtomicBool::new(false));
    let writer_stopped = stopped.clone();
    let writer_failed = failed.clone();
    let shutdown = c.shutdown.clone();
    let worker = std::thread::spawn(move || loop {
        let m = match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(m) => m,
            Err(RecvTimeoutError::Timeout) if !writer_stopped.load(Ordering::Acquire) => {
                continue;
            }
            Err(_) => break,
        };
        if let Err(e) = oxp_wire::write(&mut writer, &m) {
            event(json!({"type":"error","message":e.to_string()}));
            writer_failed.store(true, Ordering::Release);
            shutdown();
            break;
        }
    });
    let sink = Arc::new(Sink {
        tx: tx.clone(),
        state: Mutex::new(("default".into(), [0; 256])),
        clock: Instant::now(),
        shutdown: c.shutdown.clone(),
        failed: failed.clone(),
        log: EventLog::global().clone(),
    });
    if let Err(e) = output.open(sink) {
        stopped.store(true, Ordering::Release);
        (c.shutdown)();
        let _ = worker.join();
        return Err(e);
    }
    let mut consumer = Consumer::default();
    let result = (|| {
        while let Some(m) = oxp_wire::read(&mut c.reader)? {
            consumer.receive(m, &output)?;
        }
        Ok(())
    })();
    if let Err(e) = &result {
        let _ = tx.try_send(json_message(
            json!({"type":"error","code":"WIRE","message":format!("{e}")}),
        ));
    }
    pin_stopped.store(true, Ordering::Release);
    output.close();
    output.await_closed(Duration::from_secs(3));
    drop(output);
    drop(tx);
    stopped.store(true, Ordering::Release);
    let _ = worker.join();
    (c.shutdown)();
    if failed.load(Ordering::Acquire) {
        return Err("Wire writer failed or callback queue overflowed".into());
    }
    result
}
fn main() {
    let result = run();
    // Events queued by callbacks precede the final error, as when they were written directly.
    EventLog::flush_global(Duration::from_millis(500));
    if let Err(e) = result {
        event(json!({"type":"error","message":e.to_string()}));
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn port_overrides_are_validated_and_defaults_preserved() {
        let defaults = parse_ports(None).unwrap();
        assert_eq!(defaults.auth, 57209);
        assert_eq!(defaults.rtp, 15550);
        let ports = parse_ports(Some(
            "auth=20001,control=20002,media=20003,cert=20004,rtsp=20005,uibc=20006,rtp=20007",
        ))
        .unwrap();
        assert_eq!(
            [
                ports.auth,
                ports.control,
                ports.media,
                ports.cert,
                ports.rtsp,
                ports.uibc,
                ports.rtp
            ],
            [20001, 20002, 20003, 20004, 20005, 20006, 20007]
        );
        assert_eq!(parse_ports(Some("rtp=20007")).unwrap().auth, defaults.auth);
        for invalid in [
            "",
            "auth",
            "wat=123",
            "auth=0",
            "auth=65536",
            "auth=1,auth=2",
            "auth=7236",
            "auth=2,rtp=2",
        ] {
            assert!(parse_ports(Some(invalid)).is_err(), "{invalid}");
        }
    }
    #[test]
    fn callbacks_send_client_controls_and_stats_in_order() {
        let (tx, rx) = sync_channel(16);
        let sink = Sink {
            tx,
            state: Mutex::new(("default".into(), [0; 256])),
            clock: Instant::now(),
            shutdown: Arc::new(|| {}),
            failed: Arc::new(AtomicBool::new(false)),
            log: EventLog::global().clone(),
        };
        let origin = Origin {
            output_id: "clink".into(),
            client_id: "head-unit".into(),
        };
        sink.control(ControlEvent::Touch {
            screen: 0,
            contacts: vec![TouchContact::new(0, 0.5, 0.25, true).unwrap()],
            origin: origin.clone(),
        });
        sink.control(ControlEvent::Key {
            code: KeyCode::Back,
            down: true,
            origin: origin.clone(),
        });
        sink.control(ControlEvent::KeyFrameRequest {
            screen: 0,
            origin: origin.clone(),
        });
        sink.control(ControlEvent::MicPacket {
            stream: 2,
            data: vec![7; 640].into(),
            pts_us: 2345,
            origin: origin.clone(),
        });
        sink.stats(OutputStats {
            session: None,
            origin,
            screen: 0,
            decode_time_us: Some(3200),
            decode_queue: 1,
            dropped: 2,
            rendered: 120,
            end_to_end_latency_us: Some(16700),
            audio_buffered_us: Some(60000),
            audio_underruns: 2,
        });
        let messages: Vec<_> = rx.try_iter().collect();
        assert_eq!(messages.len(), 6);
        assert!(
            matches!(&messages[0],Message::Json(v) if v["type"]=="client" && v["client"]=="head-unit")
        );
        assert!(
            matches!(&messages[1],Message::Binary{header,payload} if header.kind==Kind::Touch && header.sequence==0 && oxp_wire::decode_touch(payload).unwrap().len()==1)
        );
        assert!(
            matches!(&messages[2],Message::Binary{header,payload} if header.kind==Kind::Key && header.sequence==1 && oxp_wire::decode_key(payload).unwrap()==(1,true))
        );
        assert!(matches!(&messages[3],Message::Json(v) if v["type"]=="keyframe"));
        assert!(
            matches!(&messages[5],Message::Json(v) if v["decodeMs"]==3.2 && v["renderedLatencyMs"]==16.7 && v["audioBufferedMs"]==60.0 && v["audioUnderruns"]==2)
        );
        assert!(
            matches!(&messages[4],Message::Binary {header,payload} if header.kind==Kind::Microphone && header.pts_us==2345 && header.stream==2 && payload==&vec![7;640])
        );
        for m in messages {
            oxp_wire::encode(&m).unwrap();
        }
    }
}
