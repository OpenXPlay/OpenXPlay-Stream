// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-25

import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { DisplaySettings, Hello } from '../protocol/json';
import type { PageCommand, WorkerNotice } from '../receiver/messages';

vi.mock('../shared/capabilities', () => ({
  codecCandidates: [], probeDecoders: async () => [{ supported: true, codecString: 'avc1.640028' }],
  estimateRefreshHz: async () => 60, audioCapabilities: () => ({ audio: [], microphone: false }),
}));
vi.mock('../shared/receiver-info', () => ({ collectReceiverInfo: async () => ({}), codecPowerEfficient: async () => false }));
const drawerOpen = vi.hoisted(() => vi.fn());
vi.mock('../receiver/drawer', () => ({ bindDrawer: () => ({ open: drawerOpen, close() {} }), syncChoices() {} }));
vi.mock('../receiver/plugin-slot', () => ({ PluginSlot: class {
  preference = false;
  refresh() {} receiverState() {} configure() {} visibility() {} enable() {} resetPositions() {}
} }));
vi.mock('../receiver/version', () => ({ version: 'test', shouldReload: () => false }));

class Element {
  value = ''; hidden = false; disabled = false; checked = false; textContent = ''; open = false;
  dataset: Record<string, string> = {};
  style = { setProperty() {} };
  offsetHeight = 48;
  onchange?: (event: { target: Element }) => void;
  onclick?: () => void;
  private classes = new Set<string>();
  classList = {
    add: (name: string) => { this.classes.add(name); }, remove: (name: string) => { this.classes.delete(name); },
    contains: (name: string) => this.classes.has(name),
    toggle: (name: string, on: boolean) => { if (on) this.classes.add(name); else this.classes.delete(name); },
  };
  addEventListener() {} setAttribute() {} toggleAttribute() {}
  getBoundingClientRect() { return { x: 0, y: 0, left: 0, top: 0, width: 800, height: 480 }; }
  transferControlToOffscreen() { return {}; }
}
class Worker {
  static current: Worker;
  commands: PageCommand[] = [];
  onmessage?: (event: { data: WorkerNotice }) => void;
  constructor() { Worker.current = this; }
  postMessage(command: PageCommand) {
    this.commands.push(structuredClone(command));
    if (command.type === 'audio-probe') this.receive({ type: 'audio-support', aac: false });
  }
  receive(data: WorkerNotice) { this.onmessage?.({ data }); }
  terminate() {}
}
let elements: Map<string, Element>, values: Map<string, string>, now: number;
const element = (id: string) => {
  if (!elements.has(id)) elements.set(id, new Element());
  return elements.get(id)!;
};
const settings = (frameRate?: DisplaySettings['frameRate']): DisplaySettings => ({
  type: 'display-settings', resolution: '720p', screenWidth: 250, videoCodec: 'h264',
  ...(frameRate !== undefined ? { frameRate } : {}),
});
const choose = (id: string, value: string) => {
  const choice = element(id); choice.value = value; choice.onchange!({ target: choice });
};
function hello(): Hello {
  const command = Worker.current.commands.find(command => command.type === 'init');
  if (command?.type !== 'init') throw Error('Receiver did not initialize');
  return command.hello;
}
beforeEach(() => {
  vi.resetModules(); drawerOpen.mockClear(); vi.useFakeTimers(); now = 0;
  vi.spyOn(performance, 'now').mockImplementation(() => now);
  elements = new Map(); values = new Map();
  vi.stubGlobal('localStorage', {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => { values.set(key, value); }, removeItem: (key: string) => { values.delete(key); },
  });
  vi.stubGlobal('document', {
    documentElement: new Element(), hidden: false, fullscreenEnabled: false,
    getElementById: element, querySelector: () => element('video'), querySelectorAll: () => [],
    addEventListener() {}, getAnimations: () => [],
  });
  vi.stubGlobal('window', { addEventListener() {} });
  vi.stubGlobal('navigator', { languages: ['en'], maxTouchPoints: 1 });
  vi.stubGlobal('location', { hash: '', protocol: 'https:', host: 'localhost' });
  vi.stubGlobal('screen', { width: 800, height: 480 });
  vi.stubGlobal('innerWidth', 800); vi.stubGlobal('innerHeight', 480); vi.stubGlobal('devicePixelRatio', 1);
  vi.stubGlobal('isSecureContext', true); vi.stubGlobal('Worker', Worker); vi.stubGlobal('VideoDecoder', class {});
  vi.stubGlobal('matchMedia', () => ({ matches: true, addEventListener() {} }));
  vi.stubGlobal('getComputedStyle', () => ({ marginTop: '0', marginRight: '0', marginBottom: '0', marginLeft: '0', fontSize: '16px', getPropertyValue: () => '0' }));
  vi.stubGlobal('ResizeObserver', class { observe() {} });
  vi.stubGlobal('PerformanceObserver', class { observe() {} });
});
afterEach(() => { vi.clearAllTimers(); vi.restoreAllMocks(); vi.useRealTimers(); vi.unstubAllGlobals(); });
async function start(rate = 'FPS_35') {
  values.set('oxp.frameRate', rate);
  await import('../receiver/main'); await vi.advanceTimersByTimeAsync(0);
  expect(hello().displays[0].frameRate).toBe(rate === 'AUTO' ? undefined : rate);
  Worker.current.receive({ type: 'welcome', features: ['display-settings'], sourceUrl: 'https://example.org/source' });
}

it('shows and stores the relay value, removes stale hello preferences once and waits for its echo', async () => {
  await start();
  Worker.current.commands = [];
  Worker.current.receive(settings('FPS_60'));
  expect(element('frame-rate').value).toBe('FPS_60');
  expect(values.get('oxp.frameRate')).toBe('FPS_60');
  expect(Worker.current.commands.map(command => command.type)).toEqual(['hello']);
  expect(helloFromCommands().displays.every(display => display.frameRate === undefined)).toBe(true);
  Worker.current.receive(settings('FPS_55'));
  expect(Worker.current.commands).toHaveLength(1);
  Worker.current.commands = [];
  choose('frame-rate', 'FPS_45');
  expect(Worker.current.commands).toEqual([settings('FPS_45')]);
  expect(element('frame-rate').value).toBe('FPS_55');
  expect(values.get('oxp.frameRate')).toBe('FPS_55');
  expect(element('frame-rate-status').hidden).toBe(false);
  Worker.current.receive({ type: 'playing' });
  expect(element('frame-rate-status').hidden).toBe(false);
  Worker.current.receive(settings('FPS_45'));
  expect(element('frame-rate').value).toBe('FPS_45');
  expect(values.get('oxp.frameRate')).toBe('FPS_45');
  expect(element('frame-rate-status').hidden).toBe(true);
  expect(Worker.current.commands).toEqual([settings('FPS_45')]);
});
function helloFromCommands(): Hello {
  const command = Worker.current.commands.find(command => command.type === 'hello');
  if (command?.type !== 'hello') throw Error('Expected a hello update');
  return command.hello;
}

it('shares the ten-second gate with the other settings and preserves the relay fields', async () => {
  await start('AUTO'); Worker.current.receive(settings('AUTO')); Worker.current.commands = [];
  choose('resolution', 'native');
  expect(Worker.current.commands).toEqual([{ ...settings('AUTO'), resolution: 'native' }]);
  choose('frame-rate', 'FPS_30');
  expect(Worker.current.commands).toHaveLength(1); expect(element('frame-rate').value).toBe('AUTO');
  await vi.advanceTimersByTimeAsync(10_000); now = 10_000; Worker.current.commands = []; choose('frame-rate', 'FPS_30');
  expect(Worker.current.commands.at(-1)).toEqual(settings('FPS_30'));
  choose('screen-width', '300');
  expect(Worker.current.commands).toHaveLength(1); expect(element('screen-width').value).toBe('250');
});

it('keeps hello and restart behavior when the relay omits frame rate', async () => {
  await start(); Worker.current.receive(settings()); Worker.current.commands = [];
  choose('frame-rate', 'FPS_40');
  expect(Worker.current.commands.map(command => command.type)).toEqual(['hello', 'restart']);
  expect(helloFromCommands().displays[0].frameRate).toBe('FPS_40');
  expect(element('frame-rate').value).toBe('FPS_40'); expect(values.get('oxp.frameRate')).toBe('FPS_40');
  expect(element('frame-rate').disabled).toBe(true);
  Worker.current.receive({ type: 'playing' }); expect(element('frame-rate-status').hidden).toBe(true);
  await vi.advanceTimersByTimeAsync(10_000); expect(element('frame-rate').disabled).toBe(false);
});

it('restores legacy preference reporting after disconnect and adopts the next relay value', async () => {
  await start(); Worker.current.receive(settings('FPS_60')); Worker.current.commands = [];
  Worker.current.receive({ type: 'display-disconnected' });
  expect(helloFromCommands().displays[0].frameRate).toBe('FPS_60');
  Worker.current.receive({ type: 'welcome', features: ['display-settings'], sourceUrl: 'https://example.org/source' });
  Worker.current.receive(settings()); Worker.current.commands = [];
  choose('frame-rate', 'FPS_30');
  expect(Worker.current.commands.map(command => command.type)).toEqual(['hello', 'restart']);
  Worker.current.receive({ type: 'display-disconnected' });
  Worker.current.receive({ type: 'welcome', features: ['display-settings'], sourceUrl: 'https://example.org/source' });
  Worker.current.receive(settings('FPS_50'));
  expect(element('frame-rate').value).toBe('FPS_50'); expect(values.get('oxp.frameRate')).toBe('FPS_50');
  expect(helloFromCommands().displays[0].frameRate).toBe('FPS_30');
  const update = Worker.current.commands.at(-1);
  expect(update?.type === 'hello' && update.hello.displays[0].frameRate).toBeUndefined();
});

it('opens from the host event and shows the fallback only without the feature', async () => {
  await start();
  Worker.current.receive({ type: 'welcome', features: ['host-ui'], sourceUrl: 'https://example.org/source' });
  expect(element('gear').hidden).toBe(true);
  Worker.current.receive({ type: 'host-ui' }); expect(drawerOpen).toHaveBeenCalledOnce();
  Worker.current.receive({ type: 'welcome', sourceUrl: 'https://example.org/source' });
  expect(element('gear').hidden).toBe(false);
});
