// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

import { expect, it } from 'vitest';
import { frameRates, frameRatePreference, effectiveFrameRate, RestartGate, withFrameRate } from '../receiver/frame-rate';
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
