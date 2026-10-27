// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

import { expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { drawerWidth, DrawerStack, drawerKey, horizontalPan, dragOffset, drawerRelease, fallbackSettings, settingsAnchor } from '../receiver/drawer-core';
import { strings, type TextKey } from '../receiver/ui';
import { parseMessage } from '../protocol/json';

it('fits the right drawer to wide, standard and portrait head units', () => {
  for (const [w, h, result] of [[2160, 720, 'half'], [1800, 900, 'half'], [1280, 800, 'two-thirds'], [600, 480, 'two-thirds'], [599, 300, 'full'], [800, 1280, 'full'], [900, 900, 'full']] as const)
    expect(drawerWidth(w, h)).toBe(result);
});
it('limits navigation to a root, menu and picture choice, and resets after close', () => {
  const stack = new DrawerStack(); expect(stack.pop()).toBe(false);
  expect(stack.push('codec')).toBe(false); expect(stack.push('picture')).toBe(true);
  expect(stack.push('sound')).toBe(false); expect(stack.push('frame-rate')).toBe(true);
  expect(stack.depth).toBe(3); expect(stack.previous).toBe('picture');
  expect(stack.push('resolution')).toBe(false); expect(stack.pop()).toBe(true); expect(stack.current).toBe('picture');
  stack.reset(); expect(stack.current).toBe('root'); expect(stack.depth).toBe(1);
  for (const menu of ['plugins', 'about', 'sound'] as const) { expect(stack.push(menu)).toBe(true); expect(stack.push('resolution')).toBe(false); stack.reset(); }
});
it('recognizes head-unit keys and separates deliberate pans from native scrolling', () => {
  for (const key of ['Settings', 'ContextMenu']) expect(drawerKey(key)).toBe('toggle');
  for (const key of ['GoBack', 'Backspace', 'Escape', 'BrowserBack']) expect(drawerKey(key)).toBe('back');
  expect(drawerKey('Enter')).toBeUndefined();
  expect(horizontalPan(10, 0)).toBe(false); expect(horizontalPan(30, 20)).toBe(false);
  expect(horizontalPan(-31, 20)).toBe(true); expect(horizontalPan(40, 1)).toBe(true);
  expect(dragOffset(-200, 700, 20)).toBe(-30); expect(dragOffset(-40, 700, 20)).toBe(-6);
  expect(dragOffset(1000, 700, 20)).toBe(700);
});
it('uses distance or a fresh rightward flick, preserving release momentum', () => {
  expect(drawerRelease(210, 700, .45)).toEqual({ complete: false, duration: 320 });
  expect(drawerRelease(211, 700, 0)).toEqual({ complete: true, duration: 280 });
  expect(drawerRelease(10, 700, 2)).toEqual({ complete: true, duration: 280 });
  expect(drawerRelease(600, 700, 2)).toEqual({ complete: true, duration: 160 });
  expect(drawerRelease(-10, 700, -.8).complete).toBe(false);
});
it('keeps the fallback gear for older relays and provides an invisible plugin anchor', () => {
  for (const features of [undefined, [], ['display-settings']]) expect(fallbackSettings(features)).toBe(true);
  expect(fallbackSettings(['view-area', 'host-ui'])).toBe(false);
  expect(settingsAnchor(1360, 24, 18, 8)).toEqual({ x: 1336, y: 18, w: 0, h: 0, gap: 8, side: 'right', faded: true });
  expect(parseMessage({ type: 'host-ui' }, 'relay-to-client')).toEqual({ type: 'host-ui' });
  expect(() => parseMessage({ type: 'host-ui' }, 'client-to-relay')).toThrow();
  expect(() => parseMessage({ type: 'host-ui', open: true })).toThrow();
});
it('keeps text wrapping and compositor-only motion on the Chromium 94 floor', () => {
  const css = readFileSync(new URL('../receiver/style.css', import.meta.url), 'utf8');
  const html = readFileSync(new URL('../receiver/index.html', import.meta.url), 'utf8');
  expect(css).not.toMatch(/text-overflow\s*:\s*ellipsis|line-clamp|white-space\s*:\s*nowrap|backdrop-filter|\b(?:dvh|svh)\b|:has\(|color-mix\(|@starting-style|transition-behavior/);
  for (const transition of css.matchAll(/transition:\s*([^;}]+)/g)) expect(transition[1]).not.toMatch(/\b(?:all|width|height|top|left|margin|box-shadow|filter|border-radius)\b/);
  expect(html).not.toMatch(/<select|reset-positions|sheet-backdrop/);
  expect(html.match(/data-drawer-page=/g)).toHaveLength(8);
  for (const [, key] of html.matchAll(/data-(?:text|label)="([^"]+)"/g))
    for (const language of ['en', 'zh-CN'] as const) expect(strings[language][key as TextKey]).toBeTruthy();
  expect(html).toContain('viewport-fit=cover'); expect(css).toContain('prefers-reduced-motion');
  const ids = [...html.matchAll(/\bid="([^"]+)"/g)].map(m => m[1]); expect(new Set(ids).size).toBe(ids.length);
});
