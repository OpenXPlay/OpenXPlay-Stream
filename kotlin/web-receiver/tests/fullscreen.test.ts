// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-18

import { expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { autoRestoreFullscreen, FullscreenExitHistory, FullscreenIntent } from '../receiver/fullscreen';
import { strings, type Store } from '../receiver/ui';
import type { Hello } from '../protocol/json';
const display = (JSON.parse(readFileSync(new URL('../spec/vectors/web/hello-viewport.json', import.meta.url), 'utf8')) as Hello).displays[0];
function store(): Store {
  const values = new Map<string, string>();
  return { getItem: key => values.get(key) ?? null, setItem: (key, value) => { values.set(key, value); }, removeItem: key => { values.delete(key); } };
}
it('defaults to fullscreen when supported, persists intent and tolerates denied storage', () => {
  const saved = store(), request = vi.fn(), exit = vi.fn();
  const state = new FullscreenIntent(true, () => saved);
  expect(state.pending).toBe(true);
  state.changed(true, { ...display, fullscreen: true });
  expect(saved.getItem('oxp.fullscreen')).toBe('true');
  expect(state.pending).toBe(false);
  state.toggle(request, exit);
  expect(exit).toHaveBeenCalledOnce(); expect(saved.getItem('oxp.fullscreen')).toBe('false');
  expect(new FullscreenIntent(true, () => saved).intent).toBe(false);
  state.changed(false); expect(state.pending).toBe(false);
  state.toggle(request, exit); expect(request).toHaveBeenCalledOnce();
  expect(new FullscreenIntent(true, () => saved).pending).toBe(true);
  const denied = new FullscreenIntent(true, () => { throw Error('denied'); });
  expect(() => { denied.changed(true); denied.toggle(request, exit); }).not.toThrow();
  expect(new FullscreenIntent(false, store).pending).toBe(false);
});
it('consumes the next pointerdown and retries until fullscreen returns', () => {
  const state = new FullscreenIntent(true, store), request = vi.fn();
  const event = { preventDefault: vi.fn(), stopImmediatePropagation: vi.fn() };
  state.changed(true, { ...display, fullscreen: true }); state.changed(false);
  expect(state.pointerDown(event, request)).toBe(true);
  expect(event.preventDefault).toHaveBeenCalledOnce(); expect(event.stopImmediatePropagation).toHaveBeenCalledOnce();
  expect(request).toHaveBeenCalledOnce(); expect(state.pending).toBe(true);
  state.changed(true); expect(state.pending).toBe(false);
  expect(state.pointerDown(event, request)).toBe(false); expect(request).toHaveBeenCalledOnce();
});
it('holds the last fullscreen display, preserves preferences and releases it on restore or explicit off', () => {
  const state = new FullscreenIntent(true, store);
  const full = { ...display, fullscreen: true, viewport: { width: 1920, height: 1080 } };
  const window = { ...display, fullscreen: false, viewport: { width: 1500, height: 800 }, frameRate: 'FPS_30' as const };
  state.changed(true, full); state.changed(false);
  expect(state.display(window)).toEqual({ ...full, frameRate: 'FPS_30' });
  const restored = { ...full, viewport: { width: 2000, height: 1000 } };
  state.changed(true, restored); expect(state.display(restored)).toEqual(restored);
  state.changed(false); state.toggle(vi.fn(), vi.fn()); expect(state.display(window)).toEqual(window);
});
it('wires the bilingual unobtrusive overlay and capture before CarPlay pointer handling', () => {
  const html = readFileSync(new URL('../receiver/index.html', import.meta.url), 'utf8');
  const main = readFileSync(new URL('../receiver/main.ts', import.meta.url), 'utf8');
  expect(html).toContain('id="fullscreen-hint" role="status" data-text="fullscreenRestore" hidden');
  expect(strings.en.fullscreenRestore).toBe('Tap to return to full screen');
  expect(strings['zh-CN'].fullscreenRestore).toBe('轻触屏幕恢复全屏');
  expect(main).toContain('fullscreenHint.hidden = !fullscreen.pending');
  expect(main).toContain('fullscreen.pointerDown(event, requestFullscreen)');
  expect(main).toContain('{ capture: true }');
  expect(main).toContain('fullscreen.display(measuredDisplay())');
});

it('restore defaults on, persists off, passes CarPlay touches and reports real sizes with view-area', () => {
  const saved = store(), state = new FullscreenIntent(true, () => saved);
  const full = { ...display, fullscreen: true, viewport: { width: 1920, height: 1080 } };
  state.changed(true, full);
  expect(state.changed(false)).toBe(true);
  expect(state.display(display)).toEqual({ ...full });
  state.viewAreas = true; expect(state.display(display)).toEqual(display);
  state.setRestore(false); expect(state.pending).toBe(false);
  const event = { preventDefault: vi.fn(), stopImmediatePropagation: vi.fn() }, request = vi.fn();
  expect(state.pointerDown(event, request)).toBe(false); expect(request).not.toHaveBeenCalled();
  expect(new FullscreenIntent(true, () => saved).restore).toBe(false);
  state.setRestore(true); expect(state.pending).toBe(true);
  state.changed(true); state.toggle(vi.fn(), vi.fn()); expect(state.changed(false)).toBe(false);
});
it('associates only the recent hidden interval with the exit and consumes it once', () => {
  const history = new FullscreenExitHistory();
  expect(history.take(10)).toBeNull();
  history.visibility(true, 100); history.visibility(false, 1600);
  expect(history.take(1620)).toBe(1500); expect(history.take(1630)).toBeNull();
  history.visibility(true, 2000); history.visibility(false, 3000);
  expect(history.take(6000)).toBeNull();
});

function automatic() {
  return { enabled: vi.fn(() => true), restored: vi.fn(() => false), hidden: vi.fn(() => false),
    visible: vi.fn(async () => {}), request: vi.fn(async () => {}), delay: vi.fn(async (_ms: number) => {}) };
}
it('restores on the first automatic try without a timer', async () => {
  const options = automatic();
  expect(await autoRestoreFullscreen(options)).toBe('restored');
  expect(options.request).toHaveBeenCalledOnce(); expect(options.delay).not.toHaveBeenCalled();
});
it('retries once after 500 ms and restores on the second try', async () => {
  const options = automatic(); options.request.mockRejectedValueOnce(Error('gesture'));
  expect(await autoRestoreFullscreen(options)).toBe('restored');
  expect(options.request).toHaveBeenCalledTimes(2); expect(options.delay).toHaveBeenCalledExactlyOnceWith(500);
});
it('reports denied after two rejections, including synchronous browser errors', async () => {
  const options = automatic(); options.request.mockImplementation(() => { throw Error('gesture'); });
  expect(await autoRestoreFullscreen(options)).toBe('denied');
  expect(options.request).toHaveBeenCalledTimes(2); expect(options.delay).toHaveBeenCalledOnce();
});
it('does not request when restore is off or intent is gone', async () => {
  for (const off of ['restore', 'intent'] as const) {
    const state = new FullscreenIntent(true, store), options = automatic();
    if (off === 'restore') state.setRestore(false); else state.toggle(vi.fn(), vi.fn());
    options.enabled = () => state.restore && state.intent;
    expect(await autoRestoreFullscreen(options)).toBe('off'); expect(options.request).not.toHaveBeenCalled();
  }
});
it('waits for visibility before either attempt and rechecks intent after the timer', async () => {
  const options = automatic(); let hidden = true, visible!: () => void;
  options.hidden = () => hidden;
  options.visible.mockImplementation(() => new Promise(resolve => { visible = resolve; }));
  options.request.mockRejectedValue(Error('gesture'));
  options.delay.mockImplementation(async () => { hidden = true; });
  const result = autoRestoreFullscreen(options);
  expect(options.request).not.toHaveBeenCalled(); hidden = false; visible();
  await vi.waitFor(() => expect(options.visible).toHaveBeenCalledTimes(2));
  expect(options.request).toHaveBeenCalledOnce();
  options.enabled.mockReturnValue(false); hidden = false; visible();
  expect(await result).toBe('off'); expect(options.request).toHaveBeenCalledOnce();
});
it('does not retry after a manual restoration during the timer', async () => {
  const options = automatic(); options.request.mockRejectedValue(Error('gesture'));
  options.delay.mockImplementation(async () => { options.restored.mockReturnValue(true); });
  expect(await autoRestoreFullscreen(options)).toBe('restored'); expect(options.request).toHaveBeenCalledOnce();
});
