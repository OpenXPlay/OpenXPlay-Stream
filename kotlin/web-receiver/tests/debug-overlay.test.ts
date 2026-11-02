// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-11-01

import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';

vi.mock('../receiver/drawer', () => ({ bindDrawer: () => ({ open() {} }), syncChoices() {} }));
vi.mock('../receiver/drawer-settings', () => ({ bindQuickTiles() {}, syncSettings() {}, syncPlugins() {}, syncPictureValue() {}, syncFullscreen() {} }));
vi.mock('../receiver/plugin-slot', () => ({ PluginSlot: class { preference = false; receiverState() {} refresh() {} } }));
vi.mock('../shared/capabilities', () => ({ codecCandidates: [], probeDecoders: async () => [], estimateRefreshHz: async () => 60,
  audioCapabilities: () => ({ audio: [], microphone: false }) }));

class Element {
  hidden = false; textContent = ''; dataset: Record<string, string> = {};
  children: Element[] = [];
  classList = { contains: () => false, toggle() {} };
  addEventListener() {}
  setAttribute() {}
  append(...children: Element[]) { this.children.push(...children); }
  replaceChildren(...children: Element[]) { this.children = children; }
  getBoundingClientRect() { return { width: 800, height: 480 }; }
  transferControlToOffscreen() { return {}; }
}
const nodes = new Map<string, Element>();
const node = (id: string) => { if (!nodes.has(id)) nodes.set(id, new Element()); return nodes.get(id)!; };
class Worker {
  static current: Worker;
  onmessage = (_event: { data: unknown }) => {};
  constructor() { Worker.current = this; }
  postMessage(message: { type: string }) { if (message.type === 'audio-probe') this.receive({ type: 'audio-support', aac: false }); }
  receive(data: unknown) { this.onmessage({ data }); }
}
beforeEach(async () => {
  vi.resetModules(); vi.useFakeTimers(); nodes.clear();
  const storage = { getItem: () => null, setItem() {}, removeItem() {} };
  vi.stubGlobal('localStorage', storage); vi.stubGlobal('sessionStorage', storage);
  vi.stubGlobal('document', { documentElement: new Element(), querySelector: (selector: string) => node(selector.slice(1)),
    querySelectorAll: () => [], getElementById: node, createElement: () => new Element(), addEventListener() {} });
  vi.stubGlobal('window', { addEventListener() {} });
  vi.stubGlobal('navigator', { languages: ['en'], maxTouchPoints: 0 });
  vi.stubGlobal('location', { hash: '', protocol: 'https:', host: 'localhost' });
  vi.stubGlobal('screen', { width: 800, height: 480 }); vi.stubGlobal('devicePixelRatio', 1);
  vi.stubGlobal('matchMedia', () => ({ matches: false, addEventListener() {} }));
  vi.stubGlobal('getComputedStyle', () => ({ getPropertyValue: () => '0' }));
  vi.stubGlobal('ResizeObserver', class { observe() {} });
  vi.stubGlobal('isSecureContext', true); vi.stubGlobal('VideoDecoder', class {}); vi.stubGlobal('Worker', Worker);
  await import('../receiver/main'); await vi.advanceTimersByTimeAsync(0);
});
afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); vi.unstubAllGlobals(); });

it('shows decoded and displayed rates and each drop reason in the debug overlay', () => {
  node('debug').hidden = false;
  const stats = JSON.parse(readFileSync(new URL('../spec/vectors/web/stats.json', import.meta.url), 'utf8'));
  const report = { type: 'stats', stats: { ...stats, decoderOutputPerSecond: 60, renderedFps: 59,
    renderSupersededFrames: 1, renderLateFrames: 2, renderResetFrames: 3, renderInactiveFrames: 4 },
    skipped: 10, renderQueue: 1, queueDepth: 1, playback: 'SMART', audio: [], audioBuffers: [] };
  Worker.current.receive(report);
  const rows = () => node('statistics').children[0].children.map(child => child.textContent);
  expect(rows()).toContain('Decoded / shown (fps)'); expect(rows()).toContain('60.0 / 59.0');
  expect(rows()).toContain('Decoded drops: superseded / late / reset / inactive'); expect(rows()).toContain('1 / 2 / 3 / 4');
  Worker.current.receive({ ...report, stats });
  expect(rows()).toContain('— / —'); expect(rows()).toContain('0 / 0 / 0 / 0');
  node('debug').hidden = true;
  Worker.current.receive(report);
  expect(rows()).toContain('0 / 0 / 0 / 0');
});
