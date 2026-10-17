// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

import { expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { QUALITY_BUFFER_MS, SmartDepth, paceDue, playbackMode, playbackModes, playbackParameters, withPlayback } from '../shared/playback';
import { ReceiverPreferences, playbackHint, strings } from '../receiver/ui';
import { dropPolicy, renderQueuePolicy } from '../shared/math';
import { parseMessage, type Hello } from '../protocol/json';
const hello = JSON.parse(readFileSync(new URL('../../spec/vectors/web/hello.json', import.meta.url), 'utf8')) as Hello;

it('persists every mode, omits the Smart default from hello and validates the others', () => {
  const values = new Map<string, string>();
  const storage = new ReceiverPreferences(() => ({ getItem: key => values.get(key) ?? null, setItem: (key, value) => { values.set(key, value); }, removeItem: key => { values.delete(key); } }));
  expect(storage.load(new URLSearchParams()).playback).toBe('SMART');
  for (const mode of playbackModes) {
    const preferences = storage.load(new URLSearchParams());
    preferences.playback = mode; storage.save(preferences);
    expect(values.get('oxp.playback')).toBe(mode);
    expect(storage.load(new URLSearchParams()).playback).toBe(mode);
    const updated = withPlayback(hello, mode);
    expect(parseMessage(updated, 'client-to-relay')).toEqual(updated);
    expect(updated.playback).toBe(mode === 'SMART' ? undefined : mode);
    expect('playback' in withPlayback(updated, 'SMART')).toBe(false);
  }
  for (const value of [null, undefined, '', 'smart', 'AUTO']) expect(playbackMode(value)).toBe('SMART');
});
it('falls back to Smart when browser storage is blocked', () => {
  const storage = new ReceiverPreferences(() => { throw Error('blocked'); });
  const preferences = storage.load(new URLSearchParams());
  expect(preferences.playback).toBe('SMART'); preferences.playback = 'QUALITY';
  expect(() => storage.save(preferences)).not.toThrow();
});
it('names and explains every mode in both languages', () => {
  for (const language of ['en', 'zh-CN'] as const) {
    for (const key of ['playback', 'smart', 'quality', 'speed'] as const) expect(strings[language][key]).toBeTruthy();
    for (const mode of playbackModes) expect(strings[language][playbackHint(mode)].length).toBeGreaterThan(4);
  }
  expect([strings.en.smart, strings.en.quality, strings.en.speed]).toEqual(['Smart', 'Quality', 'Speed']);
  expect(new Set(playbackModes.map(playbackHint)).size).toBe(3);
});
it('keeps the documented parameters for each mode', () => {
  expect(playbackParameters.SPEED).toEqual({ minDepth: 0, maxDepth: 0, lateBudgetMs: 50, maxDecodeQueue: 2, paced: false });
  expect(playbackParameters.SMART).toEqual({ minDepth: 0, maxDepth: 2, lateBudgetMs: 80, maxDecodeQueue: 3, paced: false });
  expect(playbackParameters.QUALITY).toEqual({ minDepth: 3, maxDepth: 3, lateBudgetMs: 1000, maxDecodeQueue: 60, paced: true });
  expect(QUALITY_BUFFER_MS).toBe(150);
});
it('bounds the decode queue per mode', () => {
  expect(dropPolicy(false, false, 2, false, false, 2).decode).toBe(true);
  expect(dropPolicy(false, false, 3, false, false, 2)).toEqual({ waiting: true, decode: false, reset: false, requestKeyframe: true });
  expect(dropPolicy(false, false, 60, false, false, 60).decode).toBe(true);
  expect(dropPolicy(false, false, 61, false, false, 60).decode).toBe(false);
  expect(dropPolicy(true, true, 61, false, false, 60).decode).toBe(true);
});
it('keeps the newest decoded frames up to the current depth', () => {
  expect(renderQueuePolicy([1, 2], 3, 2)).toEqual({ frames: [1, 2, 3], skipped: [] });
  expect(renderQueuePolicy([1, 2, 3], 4, 2)).toEqual({ frames: [2, 3, 4], skipped: [1] });
  expect(renderQueuePolicy([1, 2, 3], 4, 3)).toEqual({ frames: [1, 2, 3, 4], skipped: [] });
  expect(renderQueuePolicy([1, 2], 3, 2, item => item === 2)).toEqual({ frames: [3], skipped: [1, 2] });
});
it('paces Quality by timestamp spacing and catches up once the backlog exceeds the buffer', () => {
  const last = { at: 1000, ptsUs: 0 };
  expect(paceDue(1000, null, 0, 0)).toBe(true);
  expect(paceDue(1012, last, 16_667, 0)).toBe(false);
  expect(paceDue(1013, last, 16_667, 0)).toBe(true);
  expect(paceDue(1020, last, 33_333, 0)).toBe(false);
  expect(paceDue(1001, last, 33_333, QUALITY_BUFFER_MS)).toBe(false);
  expect(paceDue(1001, last, 33_333, QUALITY_BUFFER_MS + 1)).toBe(true);
  // Timestamp jumps are discontinuities rather than a cadence to wait for.
  for (const pts of [0, -5000, 300_000]) expect(paceDue(1001, last, pts, 0)).toBe(true);
});

/** One second of 60 fps arrivals; `offset` displaces each arrival from its timestamp. */
function second(depth: SmartDepth, index: number, offset: (frame: number) => number, frames = 60): number {
  for (let frame = 0; frame < frames; frame++) {
    const at = index * 1000 + frame * 1000 / frames;
    depth.arrival(at + offset(frame), Math.round(at * 1000));
  }
  return depth.evaluate();
}
const smooth = () => 0;
const jitter = (ms: number) => (frame: number) => frame % 2 ? ms : 0;
it('lowers Smart depth after five smooth seconds and raises it after two jittery ones', () => {
  const depth = new SmartDepth(1);
  expect([0, 1, 2, 3, 4].map(index => second(depth, index, smooth))).toEqual([1, 1, 1, 1, 0]);
  // Jitter of 0.6 frame intervals asks for one frame, 1.8 intervals for two; never more than two.
  expect([5, 6].map(index => second(depth, index, jitter(10)))).toEqual([0, 1]);
  expect([7, 8, 9, 10].map(index => second(depth, index, jitter(30)))).toEqual([1, 2, 2, 2]);
  // A single smooth second between jittery ones does not lower the depth.
  expect([11, 12, 13].map(index => second(depth, index, (frame) => index === 12 ? 0 : jitter(30)(frame)))).toEqual([2, 2, 2]);
});
it('backs off on late frames at once and holds raises for five seconds', () => {
  const depth = new SmartDepth(2);
  for (let frame = 0; frame < 3; frame++) depth.lateFrame();
  expect(second(depth, 0, jitter(30))).toBe(1);
  expect([1, 2, 3, 4, 5, 6].map(index => second(depth, index, jitter(30)))).toEqual([1, 1, 1, 1, 1, 2]);
  // Late frames must be both at least three and at least 5 % of the window.
  const steady = new SmartDepth(1);
  for (let frame = 0; frame < 3; frame++) steady.lateFrame();
  expect(second(steady, 0, jitter(10), 100)).toBe(1);
  steady.lateFrame(); steady.lateFrame();
  expect(second(steady, 1, jitter(10))).toBe(1);
});
it('keeps Smart depth without enough samples and clamps the initial depth', () => {
  const depth = new SmartDepth(1);
  for (let index = 0; index < 10; index++) expect(second(depth, index, jitter(400), 5)).toBe(1);
  expect(new SmartDepth(5).depth).toBe(2); expect(new SmartDepth(-1).depth).toBe(0);
  // A restart does not compare arrivals across a decoder reset.
  const restarted = new SmartDepth(0);
  restarted.arrival(0, 0); restarted.restart(); restarted.arrival(500, 16_667);
  expect(restarted.evaluate()).toBe(0);
});
it('reports the render depth as an optional numeric stats diagnostic', () => {
  const stats = JSON.parse(readFileSync(new URL('../../spec/vectors/web/stats-diagnostics.json', import.meta.url), 'utf8'));
  expect(parseMessage(stats, 'client-to-relay')).toMatchObject({ renderDepth: 1 });
  const { renderDepth: _, ...old } = stats; expect(parseMessage(old)).toEqual(old);
  for (const renderDepth of [-1, '1', null]) expect(() => parseMessage({ ...stats, renderDepth })).toThrow();
});
