// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

import type { VideoFormat } from '../protocol/json';

/** Preserve the codec string and decoder record (avcC or hvcC) from Stream. */
export function videoDecoderConfig(format: VideoFormat): VideoDecoderConfig {
  return { codec: format.codecString,
    description: Uint8Array.from(atob(format.config), c => c.charCodeAt(0)),
    codedWidth: format.width, codedHeight: format.height,
    hardwareAcceleration: 'prefer-hardware', optimizeForLatency: true };
}
