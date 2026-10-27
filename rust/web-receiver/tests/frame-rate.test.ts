// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

import { expect, it } from 'vitest';
import { FrameRateSync, frameRates, frameRatePreference, effectiveFrameRate, RestartGate, withFrameRate } from '../receiver/frame-rate';
import { DisplaySettingsGate } from '../receiver/display';
import { ReceiverPreferences } from '../receiver/ui';
import { parseMessage, type Hello } from '../protocol/json';
import { readFileSync } from 'node:fs';
const hello = JSON.parse(readFileSync(new URL('../spec/vectors/web/hello.json', import.meta.url), 'utf8')) as Hello;
it('persists every rate, omits the default from hello and validates every fixed rate', () => {
  const values = new Map<string, string>();
  const storage = new ReceiverPreferences(() => ({ getItem: key => values.get(key) ?? null, setItem: (key, value) => { values.set(key, value); }, removeItem: key => { values.delete(key); } }));
  for (const rate of frameRates) {
    const preferences = storage.load(new URLSearchParams());
    preferences.frameRate = rate; storage.save(preferences);
    expect(storage.load(new URLSearchParams()).frameRate).toBe(rate);
    const updated = withFrameRate(hello, rate);
    expect(parseMessage(updated, 'client-to-relay')).toEqual(updated);
    expect(updated.displays[0].frameRate).toBe(rate === 'AUTO' ? undefined : rate);
    expect(withFrameRate(updated, 'AUTO').displays[0].frameRate).toBeUndefined();
  }
  for (const value of [null, '', 'FPS_42', '60']) expect(frameRatePreference(value)).toBe('AUTO');
});
it('uses safe defaults when browser storage is blocked', () => {
  const storage = new ReceiverPreferences(() => { throw Error('blocked'); });
  const preferences = storage.load(new URLSearchParams());
  expect(preferences.frameRate).toBe('AUTO'); preferences.frameRate = 'FPS_45';
  expect(() => storage.save(preferences)).not.toThrow();
});
it('allows one restart per ten seconds including the boundary', () => {
  const gate = new RestartGate();
  expect(gate.allow(0)).toBe(true);
  for (const now of [0, 1, 9999]) expect(gate.allow(now)).toBe(false);
  expect(gate.allow(10_000)).toBe(true); expect(gate.allow(19_999)).toBe(false);
  expect(gate.allow(20_000)).toBe(true);
});

it('maps every fixed rate independently of measurements and keeps the automatic threshold', () => {
  for (const rate of frameRates.slice(1)) expect(effectiveFrameRate({ ...hello.displays[0], frameRate: rate, refreshHz: 24, sustainedFps: 10 })).toBe(Number(rate.slice(4)));
  expect(effectiveFrameRate(undefined)).toBe(60);
  for (const [refreshHz, sustainedFps, expected] of [[49.9, 60, 30], [60, 49.9, 30], [50, 50, 60]])
    expect(effectiveFrameRate({ ...hello.displays[0], frameRate: 'AUTO', refreshHz, sustainedFps })).toBe(expected);
});

it('follows the relay echo, persists its last value and removes stale hello rates', () => {
  const values = new Map<string, string>(), storage = new ReceiverPreferences(() => ({ getItem: key => values.get(key) ?? null, setItem: (key, value) => { values.set(key, value); }, removeItem: key => { values.delete(key); } }));
  const sync = new FrameRateSync('FPS_55', new RestartGate());
  const initial = sync.hello(hello); expect(initial.displays[0].frameRate).toBe('FPS_55');
  sync.receive({ type: 'display-settings', resolution: '720p', screenWidth: 250, videoCodec: 'h265', frameRate: 'FPS_35' });
  const preferences = storage.load(new URLSearchParams()); preferences.frameRate = sync.preference; storage.save(preferences);
  expect(storage.load(new URLSearchParams()).frameRate).toBe('FPS_35');
  const clean = sync.hello(initial); expect(clean.displays[0]).not.toHaveProperty('frameRate'); expect(sync.hello(clean)).toEqual(clean);
  const edit = sync.change('FPS_50', clean, true, true, 0);
  expect(edit).toEqual({ displaySettings: { type: 'display-settings', resolution: '720p', screenWidth: 250, videoCodec: 'h265', frameRate: 'FPS_50' } });
  expect(sync.preference).toBe('FPS_35');
  expect(sync.change('FPS_45', clean, true, true, 9999)).toEqual({});
  sync.receive(edit.displaySettings!); expect(sync.preference).toBe('FPS_50');
  sync.disconnect(); expect(sync.synced).toBe(false); expect(sync.hello(clean).displays[0].frameRate).toBe('FPS_50');
});
it('keeps legacy hello and restart behavior, including edits before initialization', () => {
  const gate = new RestartGate(), sync = new FrameRateSync('AUTO', gate);
  sync.receive({ type: 'display-settings', resolution: 'auto', screenWidth: 200, videoCodec: 'auto' });
  expect(sync.synced).toBe(false);
  expect(sync.change('FPS_40', hello, false, false, 0)).toEqual({}); expect(sync.preference).toBe('FPS_40');
  expect(sync.change('FPS_45', hello, true, false, 0)).toEqual({ hello: withFrameRate(hello, 'FPS_45'), restart: true });
  expect(sync.change('FPS_60', hello, true, false, 9000)).toEqual({}); expect(sync.preference).toBe('FPS_45');
  expect(sync.change('AUTO', hello, true, false, 10000)).toEqual({ hello: withFrameRate(hello, 'AUTO'), restart: true });
});
it('shares the display restart gate and never sends a synced rate without its capability', () => {
  const gate = new RestartGate(), sync = new FrameRateSync('FPS_60', gate), display = new DisplaySettingsGate();
  sync.receive({ type: 'display-settings', resolution: 'native', screenWidth: 300, videoCodec: 'h264', frameRate: 'AUTO' });
  expect(sync.change('FPS_30', hello, true, false, 0)).toEqual({});
  expect(gate.allow(0)).toBe(true); expect(sync.change('FPS_30', hello, true, true, 500)).toEqual({});
  display.welcome(['display-settings']);
  expect(display.message('screen', 125, 'auto', 'FPS_55')).toEqual({ type: 'display-settings', resolution: 'screen', screenWidth: 125, videoCodec: 'auto', frameRate: 'FPS_55' });
  expect(display.message('720p', 200, 'h265')).not.toHaveProperty('frameRate');
  expect(sync.change('FPS_30', hello, true, true, 10000)).toHaveProperty('displaySettings.frameRate', 'FPS_30');
});
