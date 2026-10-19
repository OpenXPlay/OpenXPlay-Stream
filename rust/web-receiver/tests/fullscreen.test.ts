// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-19

import { it, expect } from 'vitest';
import { FullscreenIntent, FullscreenExitHistory, autoRestoreFullscreen } from '../receiver/fullscreen';
const storage = () => ({ getItem: () => null, setItem: () => {}, removeItem: () => {} });
it('consumes the restore gesture but allows later touches after failure until the next exit', () => {
  const intent = new FullscreenIntent(true, storage); let prevented = 0, requested = 0;
  const event = { preventDefault: () => prevented++, stopImmediatePropagation: () => prevented++ };
  intent.changed(true); expect(intent.changed(false)).toBe(true);
  expect(intent.pointerDown(event, () => requested++)).toBe(true);
  intent.denied(); expect(intent.failed).toBe(true); expect(intent.pending).toBe(false);
  expect(intent.pointerDown(event, () => requested++)).toBe(false);
  expect(prevented).toBe(2); expect(requested).toBe(1);
  intent.changed(true); intent.changed(false); expect(intent.pending).toBe(true);
  intent.setRestore(false); expect(intent.pending).toBe(false);
});
it('tries twice with a 500 ms retry, checks visibility and returns diagnostics', async () => {
  let calls = 0, enabled = true, hidden = true; const delays: number[] = [];
  const options = { enabled: () => enabled, restored: () => false, hidden: () => hidden,
    visible: async () => { hidden = false; }, request: async () => { calls++; throw Error(); }, delay: async (ms: number) => { delays.push(ms); } };
  expect(await autoRestoreFullscreen(options)).toBe('denied'); expect(calls).toBe(2); expect(delays).toEqual([500]);
  enabled = false; expect(await autoRestoreFullscreen(options)).toBe('off'); expect(calls).toBe(2);
  enabled = true; expect(await autoRestoreFullscreen({ ...options, request: async () => {} })).toBe('restored');
});
it('records only the recent hidden interval', () => {
  const history = new FullscreenExitHistory(); expect(history.take(10)).toBeNull();
  history.visibility(true, 100); history.visibility(false, 1600); expect(history.take(1700)).toBe(1500); expect(history.take(1800)).toBeNull();
  history.visibility(true, 2000); history.visibility(false, 2500); expect(history.take(5001)).toBeNull();
});
