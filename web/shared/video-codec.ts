// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-18

import type { Hello, VideoCodecPreference } from '../protocol/json';

export function videoCodecPreference(value: string | null | undefined): VideoCodecPreference {
  return value === 'H264' ? 'H264' : 'AUTO';
}
/** Omit the default so automatic receivers remain compatible with older relays. */
export function withVideoCodec(hello: Hello, videoCodec: VideoCodecPreference): Hello {
  const { videoCodec: _, ...rest } = hello;
  return videoCodec === 'AUTO' ? rest : { ...rest, videoCodec };
}
