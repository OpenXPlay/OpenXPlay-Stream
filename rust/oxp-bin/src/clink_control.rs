// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-22

use crate::clink_trace::TraceRecorder;
use serde_json::{json, Value};
use std::io::BufRead;

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Clear,
    Start(u64),
    Stop,
}

fn parse(line: &[u8]) -> Result<Command, &'static str> {
    let value: Value = serde_json::from_slice(line).map_err(|_| "malformed_json")?;
    match (value["type"].as_str(), value["action"].as_str()) {
        (Some("clink-trace"), Some("clear")) => Ok(Command::Clear),
        (Some("clink-capture"), Some("start")) => {
            let seconds = match value.get("seconds") {
                None => 60,
                Some(value) => value
                    .as_u64()
                    .filter(|n| (1..=300).contains(n))
                    .ok_or("invalid_seconds")?,
            };
            Ok(Command::Start(seconds))
        }
        (Some("clink-capture"), Some("stop")) => Ok(Command::Stop),
        _ => Err("unknown_command"),
    }
}

/// Keeps at most 4 KiB while draining oversized lines through the next newline.
pub fn read_control(
    mut input: impl BufRead,
    trace: Option<&TraceRecorder>,
    mut emit: impl FnMut(Value),
) {
    let mut line = Vec::with_capacity(4096);
    let mut oversized = false;
    loop {
        let (consumed, newline, eof) = match input.fill_buf() {
            Ok(bytes) => {
                let count = bytes.iter().position(|b| *b == b'\n');
                let end = count.unwrap_or(bytes.len());
                if !oversized {
                    if end > 4096 - line.len() {
                        oversized = true;
                        line.clear();
                    } else {
                        line.extend_from_slice(&bytes[..end]);
                    }
                }
                (
                    end + usize::from(count.is_some()),
                    count.is_some(),
                    bytes.is_empty(),
                )
            }
            Err(_) => {
                emit(json!({"type":"diagnostic","message":"clink control: read_error"}));
                break;
            }
        };
        input.consume(consumed);
        if newline || eof && (!line.is_empty() || oversized) {
            let command = if oversized {
                Err("line_too_long")
            } else {
                parse(&line)
            };
            match command {
                Err(reason) => {
                    emit(json!({"type":"diagnostic","message":format!("clink control: {reason}")}))
                }
                Ok(_) if trace.is_none() => emit(json!({"type":"clink-trace","error":"trace_off"})),
                Ok(command) => {
                    if let Some(trace) = trace {
                        match command {
                            Command::Clear => trace.clear(),
                            Command::Start(seconds) => trace.capture().start(seconds),
                            Command::Stop => trace.capture().stop(),
                        }
                    }
                }
            }
            line.clear();
            oversized = false;
        }
        if eof {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};

    #[test]
    fn commands_and_seconds_validation() {
        for (line, command) in [
            (r#"{"type":"clink-trace","action":"clear"}"#, Command::Clear),
            (
                r#"{"type":"clink-capture","action":"start"}"#,
                Command::Start(60),
            ),
            (
                r#"{"type":"clink-capture","action":"start","seconds":1}"#,
                Command::Start(1),
            ),
            (
                r#"{"type":"clink-capture","action":"start","seconds":300}"#,
                Command::Start(300),
            ),
            (r#"{"type":"clink-capture","action":"stop"}"#, Command::Stop),
        ] {
            assert_eq!(parse(line.as_bytes()), Ok(command));
        }
        for seconds in ["0", "301", "-1", "1.5", "null", "true", "\"60\""] {
            let line =
                format!(r#"{{"type":"clink-capture","action":"start","seconds":{seconds}}}"#);
            assert_eq!(parse(line.as_bytes()), Err("invalid_seconds"));
        }
        assert_eq!(parse(b"garbage"), Err("malformed_json"));
        assert_eq!(parse(b"{}"), Err("unknown_command"));
    }

    #[test]
    fn oversized_lines_are_bounded_rejected_once_and_resynchronised() {
        let mut input = vec![b'x'; 100_000];
        input.extend_from_slice(b"\n{\"type\":\"clink-capture\",\"action\":\"stop\"}\r\n");
        let mut events = Vec::new();
        read_control(
            BufReader::with_capacity(17, Cursor::new(input)),
            None,
            |v| events.push(v),
        );
        assert_eq!(
            events,
            vec![
                json!({"type":"diagnostic","message":"clink control: line_too_long"}),
                json!({"type":"clink-trace","error":"trace_off"}),
            ]
        );
    }

    #[test]
    fn eof_is_quiet_and_each_bad_line_has_one_diagnostic() {
        let mut events = Vec::new();
        read_control(Cursor::new(b""), None, |v| events.push(v));
        assert!(events.is_empty());
        read_control(
            Cursor::new(b"no\n{}\n{\"type\":\"clink-trace\",\"action\":\"clear\"}"),
            None,
            |v| events.push(v),
        );
        assert_eq!(events.len(), 3);
        assert_eq!(events[0]["type"], "diagnostic");
        assert_eq!(events[1]["type"], "diagnostic");
        assert_eq!(events[2], json!({"type":"clink-trace","error":"trace_off"}));
    }

    #[test]
    fn exact_limit_is_accepted_and_long_eof_is_rejected() {
        let command = r#"{"type":"clink-trace","action":"clear"}"#;
        let input = format!("{command:4096}\n{}", "x".repeat(4097));
        let mut events = Vec::new();
        read_control(Cursor::new(input), None, |v| events.push(v));
        assert_eq!(
            events,
            vec![
                json!({"type":"clink-trace","error":"trace_off"}),
                json!({"type":"diagnostic","message":"clink control: line_too_long"}),
            ]
        );
    }
}
