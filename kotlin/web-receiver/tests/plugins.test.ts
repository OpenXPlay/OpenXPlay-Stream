// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

import { afterEach, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { PluginPolicy, resetHostPositions, area, clipPath, discover, validHostMessage, validHostPrefs, readHostPrefs, saveHostPrefs, HOST_PREFS_KEY, type Geometry } from '../receiver/plugin-policy';
import { PluginSlot } from '../receiver/plugin-slot';
import { settingsAnchor } from '../receiver/drawer-core';
import { parseMessage } from '../protocol/json';
const geometry: Geometry = { locale: 'en', theme: 'dark', reducedMotion: false, viewport: { w: 1000, h: 500 }, video: { x: 0, y: 0, w: 1000, h: 500 }, gear: { x: 920, y: 400, w: 56, h: 56, side: 'right' }, channel: '/plugins/channel' };
afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers(); });
it('discovers only the advertised local host and respects both safe modes', () => {
  const yes = { plugins: { host: '/plugins/host/index.html' } };
  expect(discover({}, 'https://relay/')).toBeNull();
  expect(discover(yes, 'https://relay/')).toBe('/plugins/host/index.html');
  expect(discover(yes, 'https://relay/?plugins=off')).toBeNull();
  expect(discover(yes, 'https://relay/', false)).toBeNull();
  expect(discover({ plugins: { host: 'https://evil/' } }, 'https://relay/')).toBeNull();
});
it.each(['absent', 'query-off', 'preference-off'])('creates no host effects when plugins are off: %s', mode => {
  const create = vi.fn(), request = vi.fn(), listen = vi.fn();
  vi.stubGlobal('document', { createElement: create, addEventListener: listen });
  vi.stubGlobal('window', { addEventListener: listen }); vi.stubGlobal('fetch', request);
  vi.stubGlobal('location', { href: mode === 'query-off' ? 'https://relay/?plugins=off' : 'https://relay/' });
  vi.stubGlobal('localStorage', { getItem: (key: string) => mode === 'preference-off' && key === 'oxp.plugins' ? 'false' : null });
  vi.useFakeTimers();
  const getGeometry = vi.fn(() => geometry);
  const slot = new PluginSlot(getGeometry, vi.fn()); slot.configure(mode === 'absent' ? {} : { plugins: { host: '/plugins/host/index.html' } }); slot.receiverState('playing'); slot.refresh(); slot.collapse();
  expect(getGeometry).not.toHaveBeenCalled(); expect(create).not.toHaveBeenCalled(); expect(request).not.toHaveBeenCalled(); expect(listen).not.toHaveBeenCalled(); expect(vi.getTimerCount()).toBe(0);
});
it('clamps union coverage, subtracts gear even fullscreen, and collapses synchronously to dock', () => {
  const p = new PluginPolicy(geometry); p.mount(0);
  const dock = [{ x: 850, y: 400, w: 56, h: 56 }];
  p.layout(dock, 'none');
  p.layout([{ x: -100, y: -100, w: 2000, h: 2000 }], 'panel'); expect(p.regions).toEqual(dock);
  p.layout([{ x: 0, y: 0, w: 1000, h: 500 }], 'fullscreen'); expect(area(p.regions)).toBe(500000 - 56 * 56);
  for (const r of p.regions) expect(r.x >= 976 || r.y >= 456 || r.x + r.w <= 920 || r.y + r.h <= 400).toBe(true);
  p.collapse(); expect(p.cover).toBe('none'); expect(p.regions).toEqual(dock);
  expect(clipPath([])).toBe('inset(100%)'); expect(clipPath(dock)).toContain('M850 400');
  expect(area([{ x: 0, y: 0, w: 100, h: 100 }, { x: 0, y: 0, w: 100, h: 100 }])).toBe(10000);
});
it('accepts only solicited pongs and reloads at most once in five minutes', () => {
  const p = new PluginPolicy(geometry); p.mount(0); p.pong(999, 5999); expect(p.tick(5999)).toBe('ok'); expect(p.tick(6000)).toBe('reload');
  p.mount(6000); const seq = p.ping(); p.pong(seq, 10000); expect(p.tick(15999)).toBe('ok'); p.pong(seq, 15999); expect(p.tick(16000)).toBe('remove');
  p.mount(306000); expect(p.tick(312000)).toBe('reload');
});
it('checks the copied inbound host message vectors', () => {
  const vectors = JSON.parse(readFileSync(new URL('../../../spec/vectors/plugins/schema.json', import.meta.url), 'utf8'));
  for (const v of vectors) {
    if (v.schema === 'host-message' && ['ready', 'pong', 'layout', 'diagnostic', 'host-prefs', 'wake'].includes(v.value.type)) expect(validHostMessage(v.value), v.name).toBe(v.valid);
  }
  expect(validHostMessage({ oxp: 'host/0', type: 'layout', cover: 'none', regions: [{ x: 0, y: 0, w: NaN, h: 5 }] })).toBe(false);
});
it('preserves the welcome golden bytes with plugins omitted and accepts explicit discovery', () => {
  const golden = JSON.parse(readFileSync(new URL('../spec/vectors/web/welcome.json', import.meta.url), 'utf8'));
  const bytes = JSON.stringify(golden); expect(JSON.stringify(parseMessage(bytes, 'relay-to-client'))).toBe(bytes);
  expect(parseMessage({ ...golden, plugins: { host: '/plugins/host/index.html' } }, 'relay-to-client')).toHaveProperty('plugins.host', '/plugins/host/index.html');
});
it('sandboxes one frame, verifies message source, collapses before host reply, and watches for stalls', () => {
  vi.useFakeTimers();
  let now = 0; vi.stubGlobal('performance', { now: () => now });
  let handler: (e: unknown) => void = () => {};
  const post = vi.fn(), remove = vi.fn(), append = vi.fn();
  const frame = { setAttribute: vi.fn(), contentWindow: { postMessage: post }, style: {} as Record<string, string>, remove, src: '', referrerPolicy: '' };
  vi.stubGlobal('document', { createElement: vi.fn(() => frame), body: { append }, addEventListener: vi.fn(), hidden: false });
  vi.stubGlobal('window', { addEventListener: (type: string, fn: typeof handler) => { if (type === 'message') handler = fn; } });
  vi.stubGlobal('location', { href: 'https://relay/' }); vi.stubGlobal('localStorage', { getItem: () => null });
  const values = new Map([[HOST_PREFS_KEY, JSON.stringify({ dock: { x: 200, y: 100 } })]]);
  const setItem = vi.fn((key: string, value: string) => { values.set(key, value); });
  vi.stubGlobal('localStorage', { getItem: (key: string) => values.get(key) ?? null, setItem });
  const wake = vi.fn(), diagnostic = vi.fn(), slot = new PluginSlot(() => geometry, diagnostic, wake);
  slot.configure({ plugins: { host: '/plugins/host/index.html' } }); expect(append).toHaveBeenCalledTimes(1);
  expect(frame.setAttribute).toHaveBeenCalledWith('sandbox', 'allow-scripts'); expect(frame.setAttribute).toHaveBeenCalledWith('allow', ''); expect(frame.referrerPolicy).toBe('no-referrer');
  const message = (type: string, rest = {}, source = frame.contentWindow) => handler({ source, data: { oxp: 'host/0', type, ...rest } });
  message('ready', { spec: 'oxp-web-plugin/0' }, {} as typeof frame.contentWindow); expect(post).not.toHaveBeenCalled();
  message('ready', { spec: 'oxp-web-plugin/0' }); expect(post).toHaveBeenCalledWith(expect.objectContaining({ type: 'init', developer: false, video: geometry.video, gear: geometry.gear, hostPrefs: { dock: { x: 200, y: 100 } } }), '*');
  message('host-prefs', { value: { dock: { x: 300, y: 150 } } }, {} as typeof frame.contentWindow); expect(setItem).not.toHaveBeenCalled();
  message('wake', {}, {} as typeof frame.contentWindow); expect(wake).not.toHaveBeenCalled();
  message('wake'); expect(wake).toHaveBeenCalledTimes(1);
  message('host-prefs', { value: [] }); expect(setItem).not.toHaveBeenCalled();
  message('host-prefs', { value: { text: '界'.repeat(1400) } }); expect(setItem).not.toHaveBeenCalled();
  message('host-prefs', { value: { dock: { x: 300, y: 150 } } });
  expect(values.get(HOST_PREFS_KEY)).toBe(JSON.stringify({ dock: { x: 300, y: 150 } }));
  geometry.developer = true; slot.refresh();
  expect(post).toHaveBeenLastCalledWith(expect.objectContaining({ type: 'geometry', developer: true }), '*');
  geometry.developer = false; slot.refresh();
  expect(post).toHaveBeenLastCalledWith(expect.objectContaining({ type: 'geometry', developer: false }), '*');
  geometry.gear.faded = true;
  slot.refresh(); expect(post).toHaveBeenLastCalledWith(expect.objectContaining({ type: 'geometry', gear: expect.objectContaining({ faded: true }) }), '*');
  geometry.gear.faded = false;
  slot.refresh(); expect(post).toHaveBeenLastCalledWith(expect.objectContaining({ type: 'geometry', hostPrefs: { dock: { x: 300, y: 150 } } }), '*');
  message('layout', { cover: 'fullscreen', regions: [{ x: 0, y: 0, w: 1000, h: 500 }] });
  const full = frame.style.clipPath; expect(slot.collapse()).toBe(true); expect(frame.style.clipPath).not.toBe(full); expect(post).toHaveBeenLastCalledWith({ oxp: 'host/0', type: 'collapse' }, '*');
  expect(slot.collapse()).toBe(false);
  slot.visibility(false);
  message('ready', { spec: 'oxp-web-plugin/0' });
  expect(post).toHaveBeenLastCalledWith({oxp:'host/0',type:'visibility',visible:false}, '*');
  slot.resetPositions();
  expect(JSON.parse(values.get(HOST_PREFS_KEY)!)).toEqual({});
  expect(post).toHaveBeenLastCalledWith({oxp:'host/0',type:'reset-positions'}, '*');
  slot.refresh();
  expect(post).toHaveBeenLastCalledWith(expect.objectContaining({type:'geometry',hostPrefs:{}}), '*');
  slot.visibility(true);
  expect(post).toHaveBeenLastCalledWith({oxp:'host/0',type:'visibility',visible:true}, '*');
  now = 2000; vi.advanceTimersByTime(2000); expect(post).toHaveBeenLastCalledWith(expect.objectContaining({ type: 'ping', seq: 1 }), '*');
  now = 6000; vi.advanceTimersByTime(4000); expect(diagnostic).toHaveBeenCalledWith('plugins-host-stalled'); expect(append).toHaveBeenCalledTimes(2);
  now = 12000; vi.advanceTimersByTime(6000); expect(append).toHaveBeenCalledTimes(2); expect(remove).toHaveBeenCalledTimes(2);
});

it('stores only bounded JSON objects and tolerates unavailable or corrupt storage', () => {
  const values = new Map<string, string>();
  const storage = () => ({ getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => { values.set(key, value); } });
  expect(readHostPrefs(storage)).toEqual({});
  const boundary = { text: 'a'.repeat(4085) };
  expect(new TextEncoder().encode(JSON.stringify(boundary)).length).toBe(4096);
  expect(saveHostPrefs(storage, boundary)).toBe(true);
  expect(readHostPrefs(storage)).toEqual(boundary);
  const circular: Record<string, unknown> = {}; circular.self = circular;
  for (const value of [null, [], 'x', new Date(), new Map(), circular, { text: 'a'.repeat(4086) }, { text: '界'.repeat(1400) }, { value: undefined }, { value: NaN }]) {
    expect(validHostPrefs(value)).toBe(false); expect(saveHostPrefs(storage, value)).toBe(false);
  }
  expect(readHostPrefs(storage)).toEqual(boundary);
  for (const value of ['{', 'null', '[]', JSON.stringify({ text: '界'.repeat(1400) })]) {
    values.set(HOST_PREFS_KEY, value); expect(readHostPrefs(storage)).toEqual({});
  }
  const blocked = () => { throw new Error('Storage blocked'); };
  expect(readHostPrefs(blocked)).toEqual({}); expect(saveHostPrefs(blocked, {})).toBe(false);
  expect(saveHostPrefs(storage, {})).toBe(true); expect(readHostPrefs(storage)).toEqual({});
});

it.each(['disable', 'capabilities', 'watchdog'])('reports cover changes synchronously and clears the escape rectangle on %s', cleanup => {
  vi.useFakeTimers();
  let now = 0, shown = false, message: (event: unknown) => void = () => {};
  vi.stubGlobal('performance', { now: () => now });
  const post = vi.fn(), frame = { setAttribute() {}, contentWindow: { postMessage: post }, style: {} as Record<string, string>, remove() {} };
  vi.stubGlobal('document', { createElement: () => frame, body: { append() {} }, addEventListener() {}, hidden: false });
  vi.stubGlobal('window', { addEventListener: (type: string, fn: typeof message) => { if (type === 'message') message = fn; } });
  vi.stubGlobal('location', { href: 'https://relay/' }); vi.stubGlobal('localStorage', { getItem: () => null, setItem() {} });
  const anchor = settingsAnchor(1000, 24, 24, 8);
  const changed = vi.fn(() => { shown = slot.cover !== 'none'; slot.refresh(); });
  const slot = new PluginSlot(() => ({ ...geometry, gear: shown ? geometry.gear : anchor }), vi.fn(), vi.fn(), changed);
  slot.configure({ plugins: { host: '/plugins/host/index.html' } });
  const layout = (cover: string) => message({ source: frame.contentWindow, data: { oxp: 'host/0', type: 'layout', cover, regions: [{ x: 0, y: 0, w: 1000, h: 500 }] } });
  expect(slot.cover).toBe('none');
  layout('fullscreen'); expect(shown).toBe(true); expect(slot.cover).toBe('fullscreen');
  expect(post).toHaveBeenLastCalledWith(expect.objectContaining({ type: 'geometry', gear: geometry.gear }), '*');
  layout('fullscreen'); expect(changed).toHaveBeenCalledOnce();
  layout('none'); expect(shown).toBe(false);
  expect(post).toHaveBeenLastCalledWith(expect.objectContaining({ type: 'geometry', gear: anchor }), '*');
  layout('panel'); expect(slot.collapse()).toBe(true); expect(shown).toBe(false); expect(slot.cover).toBe('none');
  expect(post).toHaveBeenCalledWith({ oxp: 'host/0', type: 'collapse' }, '*');
  expect(slot.collapse()).toBe(false);
  layout('fullscreen');
  if (cleanup === 'disable') slot.enable(false);
  else if (cleanup === 'capabilities') slot.configure({});
  else { now = 6000; vi.advanceTimersByTime(6000); }
  expect(shown).toBe(false); expect(slot.cover).toBe('none');
});

it('clears host geometry but retains selected surfaces and unrelated preferences', () => {
  expect(resetHostPositions({dock:{x:100,y:200},companions:{pet:{x:24,y:24}},surfaces:{pet:{x:24,y:24,anchor:'bottom-left'},ambient:{}},other:true}))
    .toEqual({surfaces:{pet:{},ambient:{}},other:true});
});
