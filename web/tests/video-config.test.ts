// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

import { expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { videoDecoderConfig } from '../receiver/video-config';

it('passes hvc1 and the complete hvcC vector to the worker decoder configuration', () => {
  const hex = readFileSync(new URL('../../spec/vectors/hevc-main-1920x1080.hex', import.meta.url), 'utf8');
  const bytes = Uint8Array.from(hex.trim().split(/\s+/).map(v => parseInt(v, 16)));
  const config = videoDecoderConfig({ codec: 'H265', codecString: 'hvc1.1.6.L120.90',
    width: 1920, height: 1080, config: Buffer.from(bytes).toString('base64') });
  expect(config.codec).toBe('hvc1.1.6.L120.90');
  expect(config.description).toEqual(bytes);
  expect(config.codedWidth).toBe(1920);
  expect(config.codedHeight).toBe(1080);
  expect(config.hardwareAcceleration).toBe('prefer-hardware');
});
