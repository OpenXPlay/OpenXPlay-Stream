// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import type { DisplaySettings } from '../protocol/json';
import type { PageCommand, WorkerNotice } from '../receiver/messages';

vi.mock('../shared/capabilities', () => ({
  codecCandidates: [], probeDecoders: async () => [{ codecString: 'avc1.4D4028', supported: true }],
  estimateRefreshHz: async () => 59, audioCapabilities: () => ({ audio: [], microphone: false }),
}));
vi.mock('../shared/receiver-info', () => ({ collectReceiverInfo: async () => ({}), codecPowerEfficient: async () => false }));
const bindings = vi.hoisted(() => ({ drawer: undefined as any, plugins: undefined as any }));
vi.mock('../receiver/drawer', () => ({
  bindDrawer(dialog: any, _reduced: unknown, visibility: (open: boolean) => void, before: () => void) {
    const open = vi.fn(() => { before(); dialog.open = true; visibility(true); });
    const close = () => { dialog.open = false; visibility(false); };
    bindings.drawer = { open, close }; return bindings.drawer;
  },
  syncChoices() {},
}));
vi.mock('../receiver/plugin-slot', () => ({ PluginSlot: class {
  preference = false; cover = 'none';
  constructor(public geometry: () => any, _diagnostic: unknown, public wake: () => void, private changed: () => void) { bindings.plugins = this; }
  layout(cover: string) { this.cover = cover; this.changed(); }
  collapse = vi.fn(() => { if (this.cover === 'none') return false; this.layout('none'); return true; });
  visibility = vi.fn(); configure = vi.fn(); enable = vi.fn(); resetPositions = vi.fn();
  refresh = vi.fn(); receiverState = vi.fn();
} }));

type Listener = (event: any) => void;
const markup = readFileSync(new URL('../receiver/index.html', import.meta.url), 'utf8');
const inputTypes = new Map([...markup.matchAll(/<\w+\b([^>]*\bid="([^"]+)"[^>]*)>/g)].map(([, attributes, id]) => [id, /\btype="([^"]+)"/.exec(attributes)?.[1]]));
class Events {
  private listeners = new Map<string, { callback: Listener; capture: boolean }[]>();
  addEventListener(type: string, callback: Listener, options?: boolean | { capture?: boolean }) {
    const capture = typeof options === 'boolean' ? options : !!options?.capture;
    this.listeners.set(type, [...this.listeners.get(type) ?? [], { callback, capture }]);
  }
  removeEventListener(type: string, callback: Listener) {
    this.listeners.set(type, (this.listeners.get(type) ?? []).filter(listener => listener.callback !== callback));
  }
  emit(type: string, properties: object = {}) {
    const event = { target: undefined as any, stopped: false, preventDefault: vi.fn(),
      stopImmediatePropagation() { this.stopped = true; }, ...properties };
    for (const listener of [...this.listeners.get(type) ?? []].sort((a, b) => Number(b.capture) - Number(a.capture))) {
      if (!event.stopped) listener.callback(event);
    }
    return event;
  }
}
class PageElement extends Events {
  value = ''; textContent = ''; hidden = false; disabled = false; checked = false; open = false;
  width = 0; height = 0; offsetHeight = 52; children: PageElement[] = [];
  dataset: Record<string, string> = {}; attributes = new Map<string, string>(); classes = new Set<string>();
  style = { setProperty: vi.fn() };
  onchange?: Listener; onclick?: Listener; oninput?: Listener; onsubmit?: Listener;
  classList = {
    add: (...names: string[]) => names.forEach(name => this.classes.add(name)),
    remove: (...names: string[]) => names.forEach(name => this.classes.delete(name)),
    contains: (name: string) => this.classes.has(name),
    toggle: (name: string, on: boolean) => on ? this.classes.add(name) : this.classes.delete(name),
  };
  constructor(public id = '') { super(); }
  setAttribute(key: string, value: string) { this.attributes.set(key, value); }
  toggleAttribute() {} querySelectorAll() { return []; }
  getBoundingClientRect() {
    const size = this.id === 'video' ? viewport : { width: 52, height: 52 };
    return { ...size, x: 0, y: 0, left: 0, top: 0 };
  }
  contains(node: unknown) { return node === this; }
  click() {
    if (this.disabled) return;
    if (inputTypes.get(this.id) === 'checkbox') { this.checked = !this.checked; this.onchange?.({ target: this }); }
    this.onclick?.({ target: this });
  }
  focus() {} setPointerCapture() {} releasePointerCapture() {} hasPointerCapture() { return false; }
  transferControlToOffscreen() { return { width: this.width, height: this.height }; }
  requestFullscreen = vi.fn(async () => {});
}
class PageWorker {
  static current: PageWorker;
  commands: PageCommand[] = [];
  onmessage?: (event: { data: WorkerNotice }) => void;
  constructor() { PageWorker.current = this; }
  terminate() {}
  postMessage(command: PageCommand) {
    this.commands.push(structuredClone(command));
    if (command.type === 'audio-probe') this.receive({ type: 'audio-support', aac: false });
  }
  receive(data: WorkerNotice) { this.onmessage?.({ data }); }
}
let nodes: Map<string, PageElement>, saved: Map<string, string>;
let page: Events & { documentElement: PageElement; hidden: boolean; fullscreenEnabled: boolean; fullscreenElement?: PageElement };
let viewport: { width: number; height: number }, insets: Record<string, number>;
let resizeObserved: () => void;
const node = (id: string) => {
  if (!inputTypes.has(id)) throw Error(`Unknown receiver element: ${id}`);
  if (!nodes.has(id)) nodes.set(id, new PageElement(id));
  return nodes.get(id)!;
};
const worker = () => PageWorker.current;
const sent = <T extends PageCommand['type']>(type: T) => worker().commands.filter((command): command is Extract<PageCommand, { type: T }> => command.type === type);
const change = (id: string, value: string) => { node(id).value = value; node(id).onchange!({ target: node(id) }); };
const relaySettings = (rate?: DisplaySettings['frameRate']): DisplaySettings => ({
  type: 'display-settings', resolution: 'native', screenWidth: 300, videoCodec: 'h265', ...(rate === undefined ? {} : { frameRate: rate }),
});
const welcome = (features?: string[]) => worker().receive({ type: 'welcome', token: '', sourceUrl: 'https://example.org/linux-source', features });
const frame = () => worker().receive({ type: 'format', format: { codec: 'H264', codecString: 'avc1.4D4028', width: 1440, height: 900, config: '' } });
function pointer(type: string, target = node('video'), pointerId = 7) {
  const event = page.emit(type, { target, pointerId, pointerType: 'touch', button: 0, clientX: 400, clientY: 250, pressure: .5 });
  if (!event.stopped) target.emit(type, event);
  return event;
}
beforeEach(() => {
  vi.resetModules(); vi.useFakeTimers(); vi.setSystemTime(0);
  vi.spyOn(performance, 'now').mockImplementation(() => Date.now());
  nodes = new Map(); saved = new Map(); viewport = { width: 912, height: 540 }; insets = { top: 2, right: 4, bottom: 6, left: 8 };
  const store = { getItem: (key: string) => saved.get(key) ?? null, setItem: (key: string, value: string) => { saved.set(key, value); }, removeItem: (key: string) => { saved.delete(key); } };
  vi.stubGlobal('localStorage', store); vi.stubGlobal('sessionStorage', store);
  page = Object.assign(new Events(), {
    documentElement: new PageElement(), hidden: false, fullscreenEnabled: false,
    getElementById: node, querySelector: (selector: string) => node(selector.slice(1)), querySelectorAll: () => [], getAnimations: () => [],
  });
  vi.stubGlobal('document', page); vi.stubGlobal('window', new Events()); vi.stubGlobal('Node', PageElement);
  vi.stubGlobal('navigator', { languages: ['en'], maxTouchPoints: 2 });
  vi.stubGlobal('location', { protocol: 'https:', host: 'head-unit.test', pathname: '/', search: '', hash: '', reload: vi.fn() });
  vi.stubGlobal('history', { replaceState: vi.fn() });
  vi.stubGlobal('screen', { width: 1366, height: 768 });
  vi.stubGlobal('innerWidth', 912); vi.stubGlobal('innerHeight', 540); vi.stubGlobal('devicePixelRatio', 1.25);
  vi.stubGlobal('isSecureContext', true); vi.stubGlobal('Worker', PageWorker); vi.stubGlobal('VideoDecoder', class {});
  vi.stubGlobal('matchMedia', () => ({ matches: true, addEventListener() {} }));
  vi.stubGlobal('requestAnimationFrame', (callback: () => void) => setTimeout(callback, 16));
  vi.stubGlobal('getComputedStyle', () => ({ fontSize: '16px', right: '14px', top: '10px', getPropertyValue: (key: string) => String(insets[key.replace('--safe-', '')] ?? 0) }));
  vi.stubGlobal('ResizeObserver', class { constructor(callback: () => void) { resizeObserved = callback; } observe() {} });
  vi.stubGlobal('PerformanceObserver', class { observe() {} });
});
afterEach(() => { vi.clearAllTimers(); vi.restoreAllMocks(); vi.useRealTimers(); vi.unstubAllGlobals(); });
async function start(rate = 'FPS_50') {
  saved.set('oxp.frameRate', rate);
  await import('../receiver/main'); await vi.advanceTimersByTimeAsync(0);
  expect(sent('init')).toHaveLength(1); worker().commands = [];
}
const applying = (on: boolean) => {
  expect(node('frame-rate').disabled).toBe(on);
  for (const id of ['frame-rate-status', 'frame-rate-choice-status']) expect(node(id).hidden).toBe(!on);
};

it('initializes physical screen, viewport, safe-area and canvas sizes with no stale saved maximum', async () => {
  saved.set('oxp-screen-max', JSON.stringify({ width: 8000, height: 4000, dpr: 1.25 }));
  await import('../receiver/main'); await vi.advanceTimersByTimeAsync(0);
  const init = sent('init')[0];
  expect(init.url).toBe('wss://head-unit.test/oxp/v0');
  expect(init.hello.displays[0]).toMatchObject({ width: 1708, height: 960, dpr: 1.25, refreshHz: 59,
    viewport: { width: 1140, height: 675 }, safeArea: { top: 2.5, right: 5, bottom: 7.5, left: 10 } });
  expect(init.canvas).toEqual({ width: 1140, height: 675 });
});
it('reports a DPR-only change after 500 ms stability and resizes the worker canvas once', async () => {
  await start(); vi.stubGlobal('devicePixelRatio', 2);
  await vi.advanceTimersByTimeAsync(599); expect(sent('hello')).toEqual([]); expect(sent('resize')).toEqual([]);
  await vi.advanceTimersByTimeAsync(1);
  expect(sent('hello')).toHaveLength(1); expect(sent('resize')).toEqual([{ type: 'resize', width: 1824, height: 1080 }]);
  expect(sent('hello')[0].hello.displays[0]).toMatchObject({ width: 2732, height: 1536, dpr: 2,
    viewport: { width: 1824, height: 1080 }, safeArea: { top: 4, right: 8, bottom: 12, left: 16 } });
  await vi.advanceTimersByTimeAsync(1000); expect(sent('hello')).toHaveLength(1); expect(sent('resize')).toHaveLength(1);
});
it('reports screen shrink, rotation and safe-area changes without CSS resize events', async () => {
  await start(); Object.assign(screen, { width: 1024, height: 600 });
  await vi.advanceTimersByTimeAsync(600);
  expect(sent('hello').at(-1)!.hello.displays[0]).toMatchObject({ width: 1280, height: 750 });
  Object.assign(screen, { width: 600, height: 1024 }); insets.left = 10;
  await vi.advanceTimersByTimeAsync(600);
  expect(sent('hello').at(-1)!.hello.displays[0]).toMatchObject({ width: 750, height: 1280, safeArea: { left: 12.5 } });
});
it('keeps resize immediate while debouncing the physical viewport report and renders aspect ratios', async () => {
  await start(); frame(); viewport = { width: 1000, height: 600 }; resizeObserved();
  expect(sent('resize').at(-1)).toEqual({ type: 'resize', width: 1250, height: 750 });
  expect(sent('hello')).toEqual([]);
  expect(node('picture-size').textContent).toBe('Picture 1440×900 (1.60:1) · Window 1250×750 (1.67:1)');
  await vi.advanceTimersByTimeAsync(600);
  expect(sent('hello').at(-1)!.hello.displays[0].viewport).toEqual({ width: 1250, height: 750 });
});
it('adopts relay frame rate, removes the hello override once and waits for the settings echo', async () => {
  await start(); welcome(['display-settings']); worker().receive(relaySettings('FPS_55'));
  expect(node('frame-rate').value).toBe('FPS_55'); expect(saved.get('oxp.frameRate')).toBe('FPS_55');
  expect(sent('hello')).toHaveLength(1); expect(sent('hello')[0].hello.displays[0].frameRate).toBeUndefined();
  worker().receive(relaySettings('FPS_60')); expect(sent('hello')).toHaveLength(1); worker().commands = [];
  change('frame-rate', 'FPS_35'); expect(worker().commands).toEqual([relaySettings('FPS_35')]); applying(true);
  expect(node('frame-rate').value).toBe('FPS_60'); expect(saved.get('oxp.frameRate')).toBe('FPS_60');
  worker().receive({ type: 'playing' }); applying(true);
  worker().receive(relaySettings('FPS_35')); applying(false);
  expect(node('frame-rate').value).toBe('FPS_35'); expect(saved.get('oxp.frameRate')).toBe('FPS_35');
});
it.each(['resolution', 'screen-width', 'relay-video-codec'])('shares the ten-second restart gate with %s and preserves the other relay fields', async id => {
  await start(); welcome(['display-settings']); worker().receive(relaySettings('FPS_50')); worker().commands = [];
  const value = id === 'resolution' ? '720p' : id === 'screen-width' ? '200' : 'h264';
  change(id, value);
  const field = id === 'resolution' ? 'resolution' : id === 'screen-width' ? 'screenWidth' : 'videoCodec';
  expect(sent('display-settings')).toEqual([{ ...relaySettings('FPS_50'), [field]: id === 'screen-width' ? 200 : value }]);
  change('frame-rate', 'FPS_40'); expect(sent('display-settings')).toHaveLength(1); applying(false);
  await vi.advanceTimersByTimeAsync(10_000); worker().commands = []; change('frame-rate', 'FPS_40');
  expect(sent('display-settings')).toEqual([relaySettings('FPS_40')]); applying(true);
  change(id, value); expect(sent('display-settings')).toHaveLength(1);
});
it('keeps legacy hello/restart behavior, clears applying on picture and unlocks at ten seconds', async () => {
  await start(); welcome(['display-settings']); worker().receive(relaySettings()); change('frame-rate', 'FPS_45');
  expect(sent('hello').at(-1)!.hello.displays[0].frameRate).toBe('FPS_45'); expect(sent('restart')).toHaveLength(1); applying(true);
  worker().receive({ type: 'playing' }); expect(node('frame-rate-status').hidden).toBe(true); expect(node('frame-rate').disabled).toBe(true);
  await vi.advanceTimersByTimeAsync(10_000); applying(false);
});
it('restores legacy reporting on disconnect and gates unsupported settings until the next welcome', async () => {
  await start(); welcome(['display-settings']); worker().receive(relaySettings('FPS_60')); change('frame-rate', 'FPS_40'); applying(true);
  worker().receive({ type: 'display-disconnected' }); applying(false);
  expect(node('display-settings').hidden).toBe(true); expect(sent('hello').at(-1)!.hello.displays[0].frameRate).toBe('FPS_60');
  await vi.advanceTimersByTimeAsync(10_000); worker().commands = []; change('resolution', '1080p'); expect(sent('display-settings')).toEqual([]);
  welcome(['display-settings']); worker().receive(relaySettings('FPS_30'));
  expect(node('frame-rate').value).toBe('FPS_30'); expect(sent('hello').at(-1)!.hello.displays[0].frameRate).toBeUndefined();
});
it('cancels a confirmed applying deadline and times out the subsequent request independently', async () => {
  await start(); welcome(['display-settings']); worker().receive(relaySettings('AUTO')); change('frame-rate', 'FPS_45'); applying(true);
  await vi.advanceTimersByTimeAsync(2000); worker().receive(relaySettings('FPS_45')); applying(false);
  vi.mocked(performance.now).mockImplementation(() => Date.now() + 8000);
  change('frame-rate', 'FPS_35'); applying(true);
  await vi.advanceTimersByTimeAsync(8000); applying(true);
  await vi.advanceTimersByTimeAsync(1999); applying(true);
  await vi.advanceTimersByTimeAsync(1); applying(false); expect(node('frame-rate').value).toBe('FPS_45');
});
it('clears held contacts and cropped plugin geometry when the display socket disconnects', async () => {
  await start(); welcome(['view-area']); frame();
  worker().receive({ type: 'view-area', x: 0, y: 0, width: 720, height: 900, canvasWidth: 1440, canvasHeight: 900 });
  expect(bindings.plugins.geometry().video.w).toBe(432);
  pointer('pointerdown'); expect(sent('touch').at(-1)!.contacts).toHaveLength(1);
  bindings.plugins.refresh.mockClear(); worker().receive({ type: 'display-disconnected' });
  expect(sent('touch').at(-1)!.contacts).toEqual([]); expect(bindings.plugins.geometry().video.w).toBe(864);
  expect(bindings.plugins.refresh).toHaveBeenCalledOnce();
  const count = sent('touch').length; pointer('pointermove'); expect(sent('touch')).toHaveLength(count);
});
it('routes host UI and card entry, collapses plugin cover before hiding it, and releases touch', async () => {
  await start(); welcome(['host-ui']); frame(); pointer('pointerdown');
  expect(sent('touch').at(-1)!.contacts).toHaveLength(1);
  expect(node('gear').hidden).toBe(true); expect(node('drawer-back').attributes.get('aria-label')).toBe('Back');
  bindings.plugins.layout('fullscreen'); worker().receive({ type: 'host-ui' });
  expect(bindings.drawer.open).toHaveBeenCalledOnce(); expect(bindings.plugins.cover).toBe('none');
  expect(bindings.plugins.visibility).toHaveBeenLastCalledWith(false); expect(sent('touch').at(-1)!.contacts).toEqual([]);
  expect(bindings.plugins.collapse.mock.invocationCallOrder.at(-1)).toBeLessThan(bindings.plugins.visibility.mock.invocationCallOrder.at(-1));
  bindings.drawer.close(); node('card-settings').click(); expect(bindings.drawer.open).toHaveBeenCalledTimes(2);
});
it.each(['panel', 'fullscreen'])('keeps an unfaded plugin escape for %s cover, then hides it after collapse', async cover => {
  await start(); welcome(['host-ui']); worker().receive({ type: 'playing' });
  expect(bindings.plugins.geometry().gear).toMatchObject({ x: 898, y: 10, w: 0, h: 0, side: 'right', faded: true });
  bindings.plugins.layout(cover); expect(node('gear').hidden).toBe(false);
  expect(bindings.plugins.geometry().gear).toMatchObject({ w: 52, h: 52, faded: false });
  await vi.advanceTimersByTimeAsync(3000); expect(node('gear').classes.has('faded')).toBe(false);
  node('gear').click(); expect(bindings.plugins.cover).toBe('none'); expect(node('gear').hidden).toBe(true);
  expect(bindings.drawer.open).not.toHaveBeenCalled();
});
it('fades the fallback gear after three seconds and wakes it through the plugin callback', async () => {
  await start(); welcome(); worker().receive({ type: 'playing' });
  await vi.advanceTimersByTimeAsync(2999); expect(node('gear').classes.has('faded')).toBe(false);
  await vi.advanceTimersByTimeAsync(1); expect(node('gear').classes.has('faded')).toBe(true);
  bindings.plugins.wake(); expect(node('gear').classes.has('faded')).toBe(false);
  node('gear').click(); expect(bindings.drawer.open).toHaveBeenCalledOnce();
});
it('wires quick tiles, plugin preferences and live playback without a restart', async () => {
  await start(); node('quick-fill').click(); expect(saved.get('oxp.fit')).toBe('fill'); expect(sent('fit').at(-1)).toEqual({ type: 'fit', fit: 'fill' });
  node('quick-sound').click(); expect(saved.get('oxp.sound')).toBe('false'); expect(node('quick-sound').attributes.get('aria-pressed')).toBe('false');
  node('plugins-enabled').checked = true; node('plugins-enabled').onchange!({ target: node('plugins-enabled') });
  expect(bindings.plugins.enable).toHaveBeenCalledWith(true); expect(node('reset-plugin-positions').hidden).toBe(false);
  node('reset-plugin-positions').click(); expect(bindings.plugins.resetPositions).toHaveBeenCalledOnce();
  change('playback', 'SPEED'); expect(sent('hello').at(-1)!.hello.playback).toBe('SPEED'); expect(sent('restart')).toEqual([]);
});
it('retries fullscreen on the next gesture after rejection and consumes both ends of the restore touch', async () => {
  page.fullscreenEnabled = true; await start();
  page.documentElement.requestFullscreen.mockRejectedValueOnce(Error('Denied'));
  const down = pointer('pointerdown'); await vi.advanceTimersByTimeAsync(0);
  expect(down.preventDefault).toHaveBeenCalled(); expect(sent('touch')).toEqual([]);
  expect(node('fullscreen-hint').hidden).toBe(false); expect(node('fullscreen-state').textContent).toBe('Unavailable in this browser');
  expect(pointer('pointerup').preventDefault).toHaveBeenCalled();
  pointer('pointerdown'); expect(page.documentElement.requestFullscreen).toHaveBeenCalledTimes(2);
  page.fullscreenElement = page.documentElement; page.emit('fullscreenchange'); pointer('pointerup');
  expect(node('fullscreen-hint').hidden).toBe(true); expect(sent('touch')).toEqual([]);
});
it('defers an involuntary fullscreen-exit report until visible and reports raw physical window geometry', async () => {
  page.fullscreenEnabled = true; await start(); welcome(['view-area']);
  page.fullscreenElement = page.documentElement; page.emit('fullscreenchange');
  page.hidden = true; page.emit('visibilitychange'); page.fullscreenElement = undefined; page.emit('fullscreenchange');
  await vi.advanceTimersByTimeAsync(1000); expect(sent('fullscreen-exit')).toEqual([]);
  viewport = { width: 800, height: 450 }; page.hidden = false; page.emit('visibilitychange');
  page.documentElement.requestFullscreen.mockRejectedValue(Error('Denied'));
  await vi.advanceTimersByTimeAsync(516);
  expect(page.documentElement.requestFullscreen).toHaveBeenCalledTimes(2);
  expect(sent('fullscreen-exit')).toEqual([{ type: 'fullscreen-exit', hiddenMs: 1000, autoRestore: 'denied', display: expect.objectContaining({ fullscreen: false, viewport: { width: 1000, height: 563 } }) }]);
});
