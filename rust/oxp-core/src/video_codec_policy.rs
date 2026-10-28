// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

use crate::model::VideoCodec;

pub fn is_hevc(codec: &str) -> bool {
    codec.starts_with("hvc1.") || codec.starts_with("hev1.")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoCodecDecision {
    pub codec: VideoCodec,
    pub reason: &'static str,
}
impl Default for VideoCodecDecision {
    fn default() -> Self {
        resolve(&[], false)
    }
}

/// Each entry represents one active main-screen output's hardware HEVC support.
pub fn resolve(outputs: &[bool], requires_avc: bool) -> VideoCodecDecision {
    resolve_with_preferences(outputs, requires_avc, false)
}

/// The preference is aggregated from active main-screen outputs only.
pub fn resolve_with_preferences(
    outputs: &[bool],
    requires_avc: bool,
    prefers_h264: bool,
) -> VideoCodecDecision {
    let reason = if requires_avc {
        "output-requires-avc"
    } else if outputs.is_empty() {
        "no-main-screen-output"
    } else if prefers_h264 {
        "main-screen-output-prefers-h264"
    } else if outputs.iter().any(|hardware| !hardware) {
        "hardware-hevc-unavailable"
    } else {
        "all-main-screen-outputs-hardware-hevc"
    };
    VideoCodecDecision {
        codec: if reason == "all-main-screen-outputs-hardware-hevc" {
            VideoCodec::H265
        } else {
            VideoCodec::H264
        },
        reason,
    }
}

/// Whether a receiver's codec string accepts a source profile, level and tier.
pub fn decoder_accepts(decoder: &str, source: &str) -> bool {
    use oxp_media::transcode_policy::CodecProfile;
    match (CodecProfile::parse(decoder), CodecProfile::parse(source)) {
        (Some(decoder), Some(source)) => decoder.accepts(&source),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preference_precedence() {
        for (outputs, avc, preference, reason) in [
            (
                vec![true, true],
                false,
                true,
                "main-screen-output-prefers-h264",
            ),
            (vec![false], false, true, "main-screen-output-prefers-h264"),
            (vec![true], true, true, "output-requires-avc"),
            (vec![], false, true, "no-main-screen-output"),
        ] {
            let decision = resolve_with_preferences(&outputs, avc, preference);
            assert_eq!(decision.codec, VideoCodec::H264);
            assert_eq!(decision.reason, reason);
        }
    }
    #[test]
    fn requires_every_active_receiver_and_respects_avc_outputs() {
        for (outputs, avc, expected) in [
            (vec![], false, VideoCodec::H264),
            (vec![true], false, VideoCodec::H265),
            (vec![true, true], false, VideoCodec::H265),
            (vec![true, false], false, VideoCodec::H264),
            (vec![false], false, VideoCodec::H264),
            (vec![true], true, VideoCodec::H264),
        ] {
            assert_eq!(resolve(&outputs, avc).codec, expected);
        }
    }
}
