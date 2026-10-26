// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-15

import { expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { applyReducedMotion, microphoneStatus, statusCard, strings, type StatusState } from '../receiver/ui';
import { sheetKey } from '../receiver/sheet';
import { version } from '../receiver/version';

it('injects the package version, and the build commit when known, into the module used by About', () => {
  const pkg = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'));
  expect(version === pkg.version || new RegExp(`^${pkg.version.replaceAll('.', '\\.')} \\([0-9a-f]{7}\\)$`).test(version)).toBe(true);
  expect(version).not.toBe('0.0.0');
});
it('maps microphone lifecycle to honest status, guidance and only useful actions', () => {
  for (const state of ['idle', 'disabled', 'requesting'] as const) expect(microphoneStatus(state).label).toBe('notInUse');
  expect(microphoneStatus('requesting').hint).toBe('micRequestHint');
  expect(microphoneStatus('capturing')).toEqual({ label: 'listening', hint: 'micListeningHint', retry: false });
  expect(microphoneStatus('denied')).toEqual({ label: 'blocked', hint: 'micBlockedHint', retry: true });
  expect(microphoneStatus('unavailable')).toEqual({ label: 'blocked', hint: 'micUnavailableHint', retry: false });
});
it('supplies plain bilingual copy for every receiver state and microphone status', () => {
  const states: StatusState[] = ['connecting', 'waiting', 'playing', 'reconnecting', 'error', 'unsupported', 'soundBlocked', 'microphoneDenied'];
  for (const language of ['en', 'zh-CN'] as const) {
    for (const state of states) {
      const card = statusCard(state);
      expect(strings[language][card.headline].length).toBeGreaterThan(2);
      expect(strings[language][card.subline].length).toBeGreaterThan(2);
    }
    for (const state of ['idle', 'capturing', 'denied', 'unavailable', 'requesting'] as const) {
      const status = microphoneStatus(state);
      expect(strings[language][status.label]).toBeTruthy(); expect(strings[language][status.hint]).toBeTruthy();
    }
    expect(strings[language].attempt).toContain('{attempt}'); expect(strings[language].attempt).toContain('{seconds}');
  }
  expect(strings.en.error).toBe('Relay unreachable');
  expect(strings['zh-CN'].microphoneDenied).toBe('麦克风已被阻止');
});
it('applies and removes the reduced-motion class on preference changes', () => {
  const classes = new Set<string>();
  const target = { classList: { toggle: (name: string, force: boolean) => force ? classes.add(name) : classes.delete(name) } };
  applyReducedMotion(target, true); expect([...classes]).toEqual(['reduced-motion']);
  applyReducedMotion(target, false); expect(classes.size).toBe(0);
});
function control(checkbox = false) {
  return { focus: vi.fn(), click: vi.fn(), matches: () => checkbox, closest: () => null } as unknown as HTMLElement;
}
function key(key: string, shiftKey = false) { return { key, shiftKey, preventDefault: vi.fn() }; }
it('traps Tab and Shift+Tab at sheet boundaries, preserves internal order and closes on Back/Escape', () => {
  const controls = [control(), control(true), control(), control()]; const close = vi.fn();
  let event = key('Tab'); sheetKey(event, controls, controls[3], close); expect(controls[0].focus).toHaveBeenCalledOnce(); expect(event.preventDefault).toHaveBeenCalledOnce();
  event = key('Tab', true); sheetKey(event, controls, controls[0], close); expect(controls[3].focus).toHaveBeenCalledOnce();
  event = key('Tab'); sheetKey(event, controls, controls[1], close); expect(event.preventDefault).toHaveBeenCalledOnce(); expect(controls[2].focus).toHaveBeenCalledOnce();
  sheetKey(key('Enter'), controls, controls[1], close); expect(controls[1].click).toHaveBeenCalledOnce();
  for (const name of ['Escape', 'BrowserBack', 'GoBack', 'Backspace']) sheetKey(key(name), controls, controls[0], close);
  expect(close).toHaveBeenCalledTimes(4);
});
it('moves arrows in order, wraps, and confines horizontal arrows to the picture-fit group', () => {
  const controls = [control(), control(), control(), control()]; const close = vi.fn();
  sheetKey(key('ArrowDown'), controls, controls[0], close); expect(controls[1].focus).toHaveBeenCalledOnce();
  sheetKey(key('ArrowUp'), controls, controls[0], close); expect(controls[3].focus).toHaveBeenCalledOnce();
  const group = { contains: (node: HTMLElement) => controls.slice(1, 3).includes(node) };
  controls[1].closest = (() => group) as HTMLElement['closest']; controls[2].closest = (() => group) as HTMLElement['closest'];
  sheetKey(key('ArrowRight'), controls, controls[2], close); expect(controls[1].focus).toHaveBeenCalledTimes(2);
  sheetKey(key('ArrowLeft'), controls, controls[1], close); expect(controls[2].focus).toHaveBeenCalledOnce();
});
it.each(['Escape', 'BrowserBack', 'GoBack', 'Backspace'])('handles %s through the shared drawer Back-key classification', name => {
  const close = vi.fn(), event = key(name), target = control();
  sheetKey(event, [target], target, close);
  expect(close).toHaveBeenCalledOnce(); expect(event.preventDefault).toHaveBeenCalledOnce();
});

it('keeps the preview out of production inputs and builds all drawer pages once', () => {
  const config = readFileSync(new URL('../vite.config.ts', import.meta.url), 'utf8');
  expect(config).toContain("receiver: 'receiver/index.html', probe: 'probe/index.html'");
  const html = readFileSync(new URL('../receiver/index.html', import.meta.url), 'utf8');
  expect(html.match(/<dialog/g)).toHaveLength(1);
  expect(html).not.toContain('<select');
  expect(html).not.toContain('reset-positions');
  for (const page of ['root', 'picture', 'resolution', 'frame-rate', 'codec', 'sound', 'plugins', 'about']) expect(html).toContain(`data-drawer-page="${page}"`);
});
