// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(super) struct Radio {
    pub device: bool,
    pub usable: bool,
    pub ap_concurrent: bool,
}

fn number_after<'a>(text: &'a str, marker: &str) -> Option<&'a str> {
    text.split_once(marker)?.1.split_whitespace().next()
}
fn combinations(phy: &str) -> Vec<String> {
    let mut result = Vec::<String>::new();
    let mut active = false;
    for line in phy.lines().map(str::trim) {
        if line.eq_ignore_ascii_case("valid interface combinations:") {
            active = true;
        } else if active && line.starts_with("* #{") {
            result.push(line.into());
        } else if active
            && (line.starts_with("total <=")
                || line.starts_with("#{")
                || line.starts_with("#channels <="))
        {
            if let Some(last) = result.last_mut() {
                last.push(' ');
                last.push_str(line);
            }
        } else if active && !line.is_empty() {
            break;
        }
    }
    result
}
fn fits(combination: &str, modes: &BTreeMap<String, usize>, channels: usize) -> bool {
    let mut rest = combination;
    let mut covered = BTreeSet::new();
    while let Some((_, after)) = rest.split_once("#{") {
        let Some((names, after)) = after.split_once('}') else {
            return false;
        };
        let Some(limit) =
            number_after(after, "<=").and_then(|s| s.trim_end_matches(',').parse::<usize>().ok())
        else {
            return false;
        };
        let names: Vec<_> = names.split(',').map(str::trim).collect();
        let count: usize = names
            .iter()
            .map(|name| modes.get(*name).copied().unwrap_or(0))
            .sum();
        if count > limit {
            return false;
        }
        covered.extend(names.into_iter().map(str::to_owned));
        rest = after;
    }
    let limit = |marker| {
        number_after(combination, marker)
            .and_then(|s| s.trim_end_matches(',').parse::<usize>().ok())
            .unwrap_or(0)
    };
    modes.keys().all(|m| covered.contains(m))
        && modes.values().sum::<usize>() <= limit("total <=")
        && channels <= limit("#channels <=")
}

/// Apply iw's actual interface budgets and channel limits, including shared mode pools.
pub(super) fn inspect(phy: &str, devices: &str, wiphy: &str, frequency: i32) -> Radio {
    let mut in_modes = false;
    let mut supported = BTreeSet::new();
    for line in phy.lines().map(str::trim) {
        if line == "Supported interface modes:" {
            in_modes = true;
        } else if in_modes && line.starts_with("* ") {
            supported.insert(line.trim_start_matches("* "));
        } else if in_modes {
            break;
        }
    }
    let device = supported.contains("P2P-device") && supported.contains("P2P-client");
    let mut modes = BTreeMap::<String, usize>::new();
    let mut channels = BTreeSet::new();
    let mut selected = false;
    for line in devices.lines().map(str::trim) {
        if let Some(phy) = line.strip_prefix("phy#") {
            selected = phy == wiphy;
        } else if selected {
            if let Some(mode) = line.strip_prefix("type ") {
                *modes.entry(mode.into()).or_default() += 1;
            }
            if line.starts_with("channel ") {
                if let Some(frequency) = line
                    .split_once('(')
                    .and_then(|(_, s)| s.split_whitespace().next())
                    .and_then(|s| s.parse::<i32>().ok())
                {
                    channels.insert(frequency);
                }
            }
        }
    }
    let busy = modes.contains_key("P2P-client") || modes.contains_key("P2P-GO");
    *modes.entry("P2P-device".into()).or_default() = 1;
    *modes.entry("P2P-client".into()).or_default() += 1;
    channels.insert(frequency);
    let combos = combinations(phy);
    let usable =
        device && !busy && frequency > 0 && combos.iter().any(|c| fits(c, &modes, channels.len()));
    // This also describes support when no AP is currently running, at the requested frequency.
    modes
        .entry("AP".into())
        .and_modify(|n| *n = (*n).max(1))
        .or_insert(1);
    let ap_concurrent = device && combos.iter().any(|c| fits(c, &modes, channels.len()));
    Radio {
        device,
        usable,
        ap_concurrent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const PHY: &str = "Supported interface modes:\n * managed\n * AP\n * P2P-device\n * P2P-client\nBand 1:\nvalid interface combinations:\n * #{ managed } <= 1, #{ AP } <= 1, #{ P2P-client } <= 1, #{ P2P-device } <= 1,\n total <= 4, #channels <= 1\nDevice supports scan flush.\n";
    #[test]
    fn same_channel_ap_only_and_real_mode_support_are_required() {
        let devices = "phy#0\n Interface wlan0\n type AP\n channel 36 (5180 MHz), width: 20 MHz\n";
        let radio = inspect(PHY, devices, "0", 5180);
        assert!(radio.device && radio.usable && radio.ap_concurrent);
        let radio = inspect(PHY, devices, "0", 2412);
        assert!(radio.device && !radio.usable && !radio.ap_concurrent);
        assert!(!inspect(&PHY.replace(" * P2P-device\n", ""), devices, "0", 5180).device);
        assert!(!inspect(&PHY.replace(" * P2P-client\n", ""), devices, "0", 5180).usable);
        assert!(inspect(PHY, devices, "1", 2412).usable);
        assert!(
            !inspect(
                "Supported interface modes:\n * managed\n * AP\n * start_p2p_device",
                devices,
                "0",
                5180
            )
            .device
        );
    }
    #[test]
    fn shared_pools_totals_and_existing_groups_cannot_be_overcommitted() {
        let devices = "phy#0\n Interface wlan0\n type AP\n";
        let shared = PHY.replace(
            "#{ AP } <= 1, #{ P2P-client } <= 1",
            "#{ AP, P2P-client } <= 1",
        );
        assert!(!inspect(&shared, devices, "0", 5180).usable);
        assert!(
            !inspect(
                &PHY.replace("total <= 4", "total <= 2"),
                "phy#0\n type AP\n type managed",
                "0",
                5180
            )
            .usable
        );
        assert!(!inspect(PHY, "phy#0\n type P2P-client", "0", 5180).usable);
        assert!(!inspect(PHY, "phy#0\n type P2P-GO", "0", 5180).usable);
        assert!(
            !inspect(
                &PHY.replace("valid interface combinations:", "unknown:"),
                devices,
                "0",
                5180
            )
            .usable
        );
    }
}
