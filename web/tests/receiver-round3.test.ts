// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-15

import { expect, it } from 'vitest';
import { soundPrompt } from '../receiver/ui';
import { previewOptions, previewSizes, previewStates } from '../preview-options';

it('chooses exactly one blocked audio action and clears it when audio resumes or sound is disabled', () => {
  for (const playing of [false, true]) {
    expect(soundPrompt(playing, true, 'suspended')).toBe(playing ? 'pill' : 'card');
    for (const state of ['running', 'closed', undefined]) expect(soundPrompt(playing, true, state)).toBeNull();
    expect(soundPrompt(playing, false, 'suspended')).toBeNull();
    expect(soundPrompt(playing, true, 'suspended', false)).toBeNull();
  }
  // A video starting or ending moves the prompt rather than duplicating it.
  expect([false, true, false].map(playing => soundPrompt(playing, true, 'suspended'))).toEqual(['card', 'pill', 'card']);
});
it('defaults to the complete viewport matrix and validates screenshot parameters', () => {
  expect(previewOptions(new URLSearchParams())).toEqual({ state: 'code', language: 'en', gear: { side: 'right', y: 0 }, sheet: false, large: false, motion: false, sizes: previewSizes });
  expect(previewOptions(new URLSearchParams('state=playingSoundBlocked&lang=zh-CN&sheet=true&large=1&motion=reduce&size=1920x720'))).toEqual({ state: 'playingSoundBlocked', language: 'zh-CN', gear: { side: 'right', y: 0 }, sheet: true, large: true, motion: true, sizes: [[1920, 720]] });
  for (const state of previewStates) expect(previewOptions(new URLSearchParams({ state })).state).toBe(state);
  expect(previewOptions(new URLSearchParams('state=unknown&lang=fr&sheet=0&large=false&motion=off')).state).toBe('code');
  expect(previewOptions(new URLSearchParams('sheet=0&large=false&motion=off'))).toMatchObject({ sheet: false, large: false, motion: false });
  for (const size of ['0x480', '800x0', '-800x480', '800x480x2', '9999x720', 'all']) expect(previewOptions(new URLSearchParams({ size })).sizes).toEqual(previewSizes);
  expect(previewOptions(new URLSearchParams('size=800X480&sheet&large&motion')).sizes).toEqual([[800, 480]]);
});

it('accepts a moved playing gear and falls back for invalid preview positions', () => {
  expect(previewOptions(new URLSearchParams('state=playing&gear=left:0.3')).gear).toEqual({ side: 'left', y: .3 });
  for (const gear of ['left:', 'top:0.3', 'left:-1', 'right:2', 'left:NaN']) expect(previewOptions(new URLSearchParams({ gear })).gear).toEqual({ side: 'right', y: 0 });
});
