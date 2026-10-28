// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

use oxp_clink::Result;
use std::time::Instant;
use std::{error::Error, fmt};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkFailure {
    pub stage: String,
    pub code: String,
    pub elapsed_ms: u64,
    pub total_elapsed_ms: u64,
    pub attempt: u32,
    pub os_code: Option<i32>,
    pub operation: Option<String>,
    pub status: Option<i32>,
}
impl fmt::Display for LinkFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} failed code={} elapsedMs={} totalElapsedMs={} attempt={}",
            self.stage, self.code, self.elapsed_ms, self.total_elapsed_ms, self.attempt
        )?;
        if let Some(code) = self.os_code {
            write!(f, " osCode={code}")?;
        }
        if let Some(operation) = &self.operation {
            write!(f, " operation={operation}")?;
        }
        if let Some(status) = self.status {
            write!(f, " status={status}")?;
        }
        Ok(())
    }
}
impl Error for LinkFailure {}

#[derive(Debug)]
pub(crate) struct PlatformFailure {
    pub code: &'static str,
    pub os_code: Option<i32>,
    pub operation: Option<String>,
    pub status: Option<i32>,
}
impl fmt::Display for PlatformFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code)
    }
}
impl Error for PlatformFailure {}
#[cfg(any(target_os = "linux", test))]
impl PlatformFailure {
    pub(crate) fn io(code: &'static str, error: std::io::Error) -> Self {
        Self {
            code,
            os_code: error.raw_os_error(),
            operation: None,
            status: None,
        }
    }
}

pub(crate) fn code(error: &(dyn Error + 'static)) -> String {
    if let Some(failure) = error.downcast_ref::<LinkFailure>() {
        return failure.code.clone();
    }
    if error.is::<std::io::Error>() {
        return "io_error".into();
    }
    if let Some(error) = error.downcast_ref::<super::UnsupportedBand>() {
        return error.to_string();
    }
    #[cfg(any(target_os = "linux", test))]
    if let Some(error) = error.downcast_ref::<super::att::Rejected>() {
        return error.to_string();
    }
    if let Some(error) = error.downcast_ref::<PlatformFailure>() {
        return error.code.into();
    }
    let text = error.to_string();
    // Only exact codes produced by this package cross the diagnostics boundary.
    match text.as_str() {
        "ambiguous_bluetooth_cccd"
        | "ambiguous_bluetooth_characteristic"
        | "bluetooth_adapter_unavailable"
        | "bluetooth_bind_failed"
        | "bluetooth_bus_invalid"
        | "bluetooth_cccd_missing"
        | "bluetooth_cccd_timeout"
        | "bluetooth_connect_failed"
        | "bluetooth_control_unavailable"
        | "bluetooth_discovery_limit"
        | "bluetooth_handoff_closed"
        | "bluetooth_handoff_service_missing"
        | "bluetooth_handoff_timeout"
        | "bluetooth_monitor_closed"
        | "bluetooth_monitor_failed"
        | "bluetooth_monitor_not_ready"
        | "bluetooth_monitor_unavailable"
        | "bluetooth_packet_limit"
        | "bluetooth_partial_write"
        | "bluetooth_phone_characteristic_missing"
        | "bluetooth_read_failed"
        | "bluetooth_request_limit"
        | "bluetooth_scan_failed"
        | "bluetooth_scan_invalid_signal"
        | "bluetooth_scan_overflow"
        | "bluetooth_scan_start_timeout"
        | "bluetooth_socket_failed"
        | "bluetooth_vehicle_characteristic_missing"
        | "bluetooth_write_echo_mismatch"
        | "bluetooth_write_failed"
        | "command_timeout"
        | "duplicate_network_bootstrap"
        | "head_unit_rejected_request"
        | "invalid_ble_peers"
        | "invalid_bluetooth_address"
        | "invalid_bluetooth_attribute"
        | "invalid_bluetooth_attribute_range"
        | "invalid_bluetooth_configuration"
        | "invalid_bluetooth_discovery"
        | "invalid_bluetooth_mtu"
        | "invalid_bluetooth_notification"
        | "invalid_bluetooth_response"
        | "invalid_bluetooth_write_response"
        | "invalid_link_configuration"
        | "invalid_network_bootstrap"
        | "invalid_phone_bluetooth_address"
        | "invalid_service_changed"
        | "link_already_connected"
        | "link_connect_failed"
        | "link_lock_poisoned"
        | "link_monitor_failed"
        | "linux_link_unavailable"
        | "missing_network_bootstrap"
        | "multiple_vehicles_select_address"
        | "network_bootstrap_limit"
        | "network_dhcp_server_unavailable"
        | "network_ipv4_invalid"
        | "network_ipv4_unavailable"
        | "network_link_lost"
        | "phone_bootstrap_limit"
        | "p2p_bus_invalid"
        | "p2p_device_unavailable"
        | "p2p_dhcp_failed"
        | "p2p_dhcp_invalid"
        | "p2p_dhcp_stopped"
        | "p2p_group_mismatch"
        | "p2p_join_timeout"
        | "p2p_phy_unavailable"
        | "selected_vehicle_not_discovered"
        | "shutdown_requested"
        | "system_tool_failed"
        | "system_tool_unavailable"
        | "unexpected_bluetooth_notification"
        | "unexpected_bluetooth_response"
        | "vehicle_not_discovered"
        | "wifi_join_p2p_cleanup_failed"
        | "wifi_join_p2p_unsupported" => text,
        _ => "unknown_error".into(),
    }
}
pub(crate) fn diagnosed<T>(
    stage: &str,
    attempt: u32,
    start: Instant,
    emit: &mut dyn FnMut(crate::LinkEvent),
    action: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let at = Instant::now();
    action().map_err(|error| {
        if error.is::<LinkFailure>() {
            return error;
        }
        let failure = LinkFailure {
            stage: stage.into(),
            code: code(error.as_ref()),
            elapsed_ms: millis(at.elapsed()),
            total_elapsed_ms: millis(start.elapsed()),
            attempt,
            os_code: error
                .downcast_ref::<std::io::Error>()
                .and_then(|e| e.raw_os_error())
                .or_else(|| {
                    error
                        .downcast_ref::<PlatformFailure>()
                        .and_then(|e| e.os_code)
                }),
            operation: error
                .downcast_ref::<PlatformFailure>()
                .and_then(|e| e.operation.clone()),
            status: error
                .downcast_ref::<PlatformFailure>()
                .and_then(|e| e.status),
        };
        emit(crate::LinkEvent::Failure(failure.clone()));
        Box::new(failure) as Box<dyn Error + Send + Sync>
    })
}
pub(crate) fn millis(duration: std::time::Duration) -> u64 {
    duration.as_millis().min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_diagnosis_keeps_the_original_stage_and_tool_status() {
        let mut events = vec![];
        let start = Instant::now();
        let result = diagnosed::<()>("gatt_connect", 3, start, &mut |e| events.push(e), || {
            Err(PlatformFailure {
                code: "system_tool_failed",
                operation: Some("nmcli".into()),
                status: Some(7),
                os_code: None,
            }
            .into())
        });
        let result = diagnosed("ble_exchange", 3, start, &mut |e| events.push(e), || result);
        let failure = result.unwrap_err().downcast::<LinkFailure>().unwrap();
        assert_eq!(failure.stage, "gatt_connect");
        assert_eq!(failure.status, Some(7));
        assert_eq!(failure.operation.as_deref(), Some("nmcli"));
        assert_eq!(events.len(), 1);
    }
    #[test]
    fn stages_codes_timing_and_errno_survive_without_platform_messages() {
        let start = Instant::now() - std::time::Duration::from_secs(2);
        for (error, expected, os_code) in [
            (
                "bluetooth_cccd_timeout".into(),
                "bluetooth_cccd_timeout",
                None,
            ),
            ("secret-password".into(), "unknown_error", None),
            (
                Box::new(std::io::Error::from_raw_os_error(13)) as Box<dyn Error + Send + Sync>,
                "io_error",
                Some(13),
            ),
        ] {
            let mut events = vec![];
            let result = diagnosed::<()>("ble_exchange", 2, start, &mut |e| events.push(e), || {
                Err(error)
            });
            let failure = result.unwrap_err().downcast::<LinkFailure>().unwrap();
            assert_eq!(failure.code, expected);
            assert_eq!(failure.stage, "ble_exchange");
            assert_eq!(failure.attempt, 2);
            assert_eq!(failure.os_code, os_code);
            assert!(failure.total_elapsed_ms >= 2000);
            assert!(!failure.to_string().contains("secret-password"));
            assert!(matches!(&events[..], [crate::LinkEvent::Failure(f)] if f == failure.as_ref()));
        }
    }
}
