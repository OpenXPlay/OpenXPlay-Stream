// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-19

import { it, expect } from 'vitest';
import { DisplayDebounce, DisplaySettingsGate, pictureSize } from '../receiver/display';
import { parseMessage, type DisplayInfo } from '../protocol/json';
import { readFileSync } from 'node:fs';
const hello = JSON.parse(readFileSync(new URL('../spec/vectors/web/hello-fullscreen-preferred.json', import.meta.url), 'utf8'));
const display: DisplayInfo = hello.displays[0];
it('debounces changed geometry, viewport and fullscreen intent for 500 ms', () => {
  const debounce = new DisplayDebounce(); debounce.seed(display);
  const changed = { ...display, viewport: { width: 800, height: 600 } };
  expect(debounce.observe(changed, 0)).toBe(false);
  expect(debounce.observe(changed, 300)).toBe(false);
  expect(debounce.observe(changed, 500)).toBe(true);
  expect(debounce.observe(changed, 1000)).toBe(false);
  expect(debounce.observe({ ...changed, fullscreenPreferred: false }, 1100)).toBe(false);
  expect(debounce.observe({ ...changed, fullscreenPreferred: false }, 1600)).toBe(true);
});
it('requires feature negotiation and sends every settings request without throttling', () => {
  const gate = new DisplaySettingsGate(); expect(gate.message('auto', 200, 'auto')).toBeUndefined();
  gate.welcome(['display-settings']);
  for (const resolution of ['auto', 'native', '1080p', '720p', 'screen'] as const)
    for (const screenWidth of [125, 200, 250, 300] as const)
      for (const videoCodec of ['auto', 'h265', 'h264'] as const)
        expect(parseMessage(gate.message(resolution, screenWidth, videoCodec), 'client-to-relay')).toEqual({ type: 'display-settings', resolution, screenWidth, videoCodec });
  gate.welcome(); expect(gate.supported).toBe(false);
});
it('rejects malformed optional display fields and message directions', () => {
  for (const fields of [{ viewport: null }, { viewport: { width: 0, height: 10 } }, { viewport: { width: 1.5, height: 10 } }, { fullscreen: null }, { fullscreenPreferred: 'true' }])
    expect(() => parseMessage({ ...hello, displays: [{ ...display, ...fields }] })).toThrow();
  for (const fields of [{ resolution: 'bad' }, { screenWidth: 150 }, { videoCodec: 'H265' }, { videoCodec: undefined }])
    expect(() => parseMessage({ type: 'display-settings', resolution: 'auto', screenWidth: 200, videoCodec: 'auto', ...fields }, 'relay-to-client')).toThrow();
  const area = { type: 'view-area', x: 100, y: 10, width: 300, height: 200, canvasWidth: 800, canvasHeight: 400 };
  expect(() => parseMessage(area, 'client-to-relay')).toThrow();
  for (const fields of [{ width: 0 }, { x: -1 }, { x: 501 }, { height: 391 }, { canvasWidth: 0 }]) expect(() => parseMessage({ ...area, ...fields })).toThrow();
  const exit = { type: 'fullscreen-exit', hiddenMs: null, display };
  expect(parseMessage(exit, 'client-to-relay')).toEqual(exit);
  for (const fields of [{ hiddenMs: -1 }, { autoRestore: 'bad' }, { display: null }]) expect(() => parseMessage({ ...exit, ...fields })).toThrow();
  expect(() => parseMessage(exit, 'relay-to-client')).toThrow();
});

it('describes picture dimensions with a two-decimal aspect ratio', () => {
  expect(pictureSize(1440, 900)).toBe('1440×900 (1.60:1)');
  expect(pictureSize(1080, 1920)).toBe('1080×1920 (0.56:1)');
  expect(pictureSize(0, 900)).toBe('—'); expect(pictureSize(1440, 0)).toBe('—');
});
