// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-25

import { expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { DrawerStack, drawerWidth, drawerRelease, drawerKey, horizontalPan, dragOffset, fallbackSettings, settingsAnchor } from '../receiver/drawer-core';
import { parseMessage } from '../protocol/json';

it.each([[1920, 720, 'half'], [1600, 600, 'half'], [1280, 720, 'two-thirds'], [1024, 600, 'two-thirds'], [800, 1280, 'full'], [599, 300, 'full'], [600, 400, 'two-thirds'], [600, 300, 'half'], [600, 600, 'full']])('sizes %s × %s as %s', (width, height, result) => {
  expect(drawerWidth(Number(width), Number(height))).toBe(result);
});
it('limits navigation to root, menu and that menu’s choices', () => {
  const stack = new DrawerStack();
  expect(stack.current).toBe('root'); expect(stack.pop()).toBe(false);
  expect(stack.push('resolution')).toBe(false);
  expect(stack.push('picture')).toBe(true); expect(stack.previous).toBe('root');
  expect(stack.push('codec')).toBe(true); expect(stack.depth).toBe(3);
  for (const page of ['root', 'picture', 'sound', 'resolution', 'frame-rate'] as const) expect(stack.push(page)).toBe(false);
  expect(stack.pop()).toBe(true); expect(stack.current).toBe('picture');
  expect(stack.push('frame-rate')).toBe(true);
  stack.close(); expect(stack.depth).toBe(1); expect(stack.current).toBe('root'); expect(stack.previous).toBeUndefined();
  for (const menu of ['sound', 'plugins', 'about'] as const) {
    expect(stack.push(menu)).toBe(true); expect(stack.push('resolution')).toBe(false); stack.close();
  }
});
it('finishes by distance or recent rightward velocity and bounds momentum duration', () => {
  expect(drawerRelease(300, 1000, 0)).toEqual({ complete: false, duration: 320 });
  expect(drawerRelease(301, 1000, 0)).toEqual({ complete: true, duration: 280 });
  expect(drawerRelease(20, 1000, .45).complete).toBe(false);
  expect(drawerRelease(20, 1000, .451).complete).toBe(true);
  expect(drawerRelease(900, 1000, 4)).toEqual({ complete: true, duration: 160 });
  expect(drawerRelease(400, 1000, 2)).toEqual({ complete: true, duration: 280 });
  expect(drawerRelease(700, 1000, 1.5)).toEqual({ complete: true, duration: 200 });
  expect(drawerRelease(-10, 1000, -.5)).toEqual({ complete: false, duration: 320 });
});
it('preserves vertical scrolling, follows rightward drags and caps the left rubber band', () => {
  expect(horizontalPan(10, 0)).toBe(false); expect(horizontalPan(15, 10)).toBe(false);
  expect(horizontalPan(16, 10)).toBe(true); expect(horizontalPan(-16, 10)).toBe(true);
  expect(horizontalPan(20, 100)).toBe(false);
  expect(dragOffset(70, 500, 16)).toBe(70); expect(dragOffset(700, 500, 16)).toBe(500);
  expect(dragOffset(-100, 500, 16)).toBe(-15); expect(dragOffset(-1000, 500, 16)).toBe(-24);
});
it('maps head-unit entry and Back keys without intercepting ordinary keys', () => {
  for (const key of ['ContextMenu', 'Settings']) expect(drawerKey(key)).toBe('toggle');
  for (const key of ['Escape', 'BrowserBack', 'GoBack', 'Backspace']) expect(drawerKey(key)).toBe('back');
  for (const key of ['Tab', 'Enter', 'ArrowRight', 'a']) expect(drawerKey(key)).toBeUndefined();
});
it('uses the fallback only without host-ui and supplies a zero-size right anchor otherwise', () => {
  expect(fallbackSettings()).toBe(true); expect(fallbackSettings([])).toBe(true);
  expect(fallbackSettings(['view-area', 'display-settings'])).toBe(true);
  expect(fallbackSettings(['host-ui'])).toBe(false);
  expect(settingsAnchor(1920, 24, 18, 8)).toEqual({ x: 1896, y: 18, w: 0, h: 0, side: 'right', faded: true, gap: 8 });
});
it('parses the unchanged relay host-ui vector and rejects fields and the wrong direction', () => {
  const vector = readFileSync(new URL('../spec/vectors/web/host-ui.json', import.meta.url), 'utf8');
  expect(parseMessage(vector, 'relay-to-client')).toEqual({ type: 'host-ui' });
  expect(() => parseMessage(vector, 'client-to-relay')).toThrow();
  expect(() => parseMessage({ type: 'host-ui', url: 'anything' })).toThrow();
});
it('keeps animation properties compositor-only and avoids newer CSS features', () => {
  const css = readFileSync(new URL('../receiver/style.css', import.meta.url), 'utf8');
  expect(css).not.toMatch(/backdrop-filter|blur\(|@starting-style|transition-behavior|:has\(|color-mix\(|\b[ds]vh\b/);
  // The drawer moves with interruptible transitions; status-card keyframes may animate only compositor properties.
  for (const [, body] of css.matchAll(/@keyframes [\w-]+ \{((?:[^{}]*\{[^}]*\})*)\s*\}/g)) {
    for (const [, property] of body.matchAll(/([\w-]+)\s*:/g)) expect(property).toMatch(/^(transform|opacity)$/);
  }
  for (const [, transitions] of css.matchAll(/transition:\s*([^;}]+)/g)) {
    if (transitions === 'none' || transitions === 'none !important') continue;
    let depth = 0, start = 0;
    for (let i = 0; i <= transitions.length; i++) {
      if (transitions[i] === '(') depth++;
      if (transitions[i] === ')') depth--;
      if (i === transitions.length || transitions[i] === ',' && depth === 0) {
        expect(transitions.slice(start, i).trim()).toMatch(/^(transform|opacity)\s/); start = i + 1;
      }
    }
  }
  expect(css).toContain('contain: layout paint style');
  expect(css).not.toMatch(/\.(?:panel|page|scrim|root-item)[^{]*\{[^}]*animation:/);
});
