// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use oxp_clink::Result;
use std::{
    io::Read,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub(crate) struct Process(pub Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
// Discard stderr: nmcli and D-Bus errors may contain credentials.
pub(crate) fn run(program: &str, args: &[&str], deadline: Instant) -> Result<String> {
    run_with_cancel(program, args, deadline, true)
}

pub(crate) fn run_cleanup(program: &str, args: &[&str], deadline: Instant) -> Result<String> {
    run_with_cancel(program, args, deadline, false)
}

fn run_with_cancel(
    program: &str,
    args: &[&str],
    deadline: Instant,
    cancel: bool,
) -> Result<String> {
    #[cfg(not(target_os = "linux"))]
    let _ = cancel;
    if Instant::now() >= deadline {
        return Err("command_timeout".into());
    }
    let mut child = Process(
        Command::new(program)
            .args(args)
            // C.UTF-8 keeps tool messages stable but preserves non-ASCII SSIDs (C prints them as "?").
            .env("LC_ALL", "C.UTF-8")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| crate::diagnostics::PlatformFailure {
                operation: Some(program.into()),
                ..crate::diagnostics::PlatformFailure::io("system_tool_unavailable", error)
            })?,
    );
    let mut pipe = child.0.stdout.take().ok_or("system_tool_failed")?;
    let reader = thread::spawn(move || {
        let mut out = String::new();
        pipe.by_ref()
            .take(1024 * 1024 + 1)
            .read_to_string(&mut out)
            .map(|_| out)
    });
    loop {
        #[cfg(target_os = "linux")]
        if cancel {
            crate::signals::check()?;
        }
        if let Some(status) = child.0.try_wait()? {
            let out = reader.join().map_err(|_| "system_tool_failed")??;
            if !status.success() || out.len() > 1024 * 1024 {
                return Err(crate::diagnostics::PlatformFailure {
                    code: "system_tool_failed",
                    operation: Some(program.into()),
                    status: status.code(),
                    os_code: None,
                }
                .into());
            }
            return Ok(out);
        }
        if Instant::now() >= deadline {
            return Err("command_timeout".into());
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tool_exit_status_is_kept_but_output_is_not_exposed() {
        let error = run(
            "sh",
            &["-c", "echo private-credential >&2; exit 7"],
            Instant::now() + Duration::from_secs(3),
        )
        .unwrap_err();
        let failure = error
            .downcast_ref::<crate::diagnostics::PlatformFailure>()
            .unwrap();
        assert_eq!(failure.operation.as_deref(), Some("sh"));
        assert_eq!(failure.status, Some(7));
        assert_eq!(failure.to_string(), "system_tool_failed");
    }
}
