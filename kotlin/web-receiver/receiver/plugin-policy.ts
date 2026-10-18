// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

export interface Region { x: number; y: number; w: number; h: number }
export interface Geometry {
  locale: string; theme: string; reducedMotion: boolean; viewport: { w: number; h: number };
  developer?: boolean;
  hostPrefs?: Record<string, unknown>;
  video: Region; gear: Region & { side: 'left' | 'right'; faded?: boolean; gap?: number }; channel: '/plugins/channel';
}
export type Cover = 'none' | 'panel' | 'fullscreen';
export function discover(capabilities: { plugins?: { host: string } }, url: string, preference = true): string | null {
  return preference && new URL(url).searchParams.get('plugins') !== 'off' && capabilities.plugins?.host === '/plugins/host/index.html' ? capabilities.plugins.host : null;
}
function clamp(r: Region, v: Geometry['viewport']): Region {
  const x = Math.max(0, Math.min(v.w, r.x)), y = Math.max(0, Math.min(v.h, r.y));
  return { x, y, w: Math.max(0, Math.min(v.w, r.x + r.w) - x), h: Math.max(0, Math.min(v.h, r.y + r.h) - y) };
}
export function area(regions: Region[]): number {
  const edges = [...new Set(regions.flatMap(r => [r.x, r.x + r.w]))].sort((a, b) => a - b);
  let result = 0;
  for (let i = 1; i < edges.length; i++) {
    const spans = regions.filter(r => r.x < edges[i] && r.x + r.w > edges[i - 1]).map(r => [r.y, r.y + r.h]).sort((a, b) => a[0] - b[0]);
    let end = -Infinity, height = 0;
    for (const [low, high] of spans) { height += Math.max(0, high - Math.max(low, end)); end = Math.max(end, high); }
    result += (edges[i] - edges[i - 1]) * height;
  }
  return result;
}
function exclude(r: Region, g: Region): Region[] {
  const left = Math.max(r.x, g.x), top = Math.max(r.y, g.y), right = Math.min(r.x + r.w, g.x + g.w), bottom = Math.min(r.y + r.h, g.y + g.h);
  if (left >= right || top >= bottom) return [r];
  return [{ x: r.x, y: r.y, w: r.w, h: top - r.y }, { x: r.x, y: bottom, w: r.w, h: r.y + r.h - bottom },
    { x: r.x, y: top, w: left - r.x, h: bottom - top }, { x: right, y: top, w: r.x + r.w - right, h: bottom - top }].filter(r => r.w > 0 && r.h > 0);
}
export function clipPath(regions: Region[]): string {
  return regions.length ? `path('${regions.map(r => `M${r.x} ${r.y}h${r.w}v${r.h}h-${r.w}Z`).join(' ')}')` : 'inset(100%)';
}
export class PluginPolicy {
  cover: Cover = 'none'; regions: Region[] = []; dock: Region[] = [];
  private lastPong = 0; private lastReload = -Infinity; private removed = true; private seq = 0; private pending = new Set<number>();
  constructor(public geometry: Geometry) {}
  mount(now: number): void { this.lastPong = now; this.removed = false; this.cover = 'none'; this.regions = []; this.dock = []; this.pending.clear(); }
  private fallback(): Region[] {
    if (this.dock.length) return this.dock;
    const { viewport: v, gear: g } = this.geometry, w = Math.min(g.w || 48, v.w), h = Math.min(g.h || 48, v.h);
    return [clamp({ x: g.side === 'right' ? g.x - w - (g.gap ?? 8) : g.x + g.w + (g.gap ?? 8), y: Math.min(g.y, v.h - h), w, h }, v)];
  }
  layout(regions: Region[], cover: Cover): Region[] {
    const { viewport, gear } = this.geometry;
    let clean = regions.map(r => clamp(r, viewport)).filter(r => r.w > 0 && r.h > 0);
    if (cover !== 'fullscreen' && area(clean) > viewport.w * viewport.h * .4) clean = this.fallback().map(r => clamp(r, viewport));
    this.regions = clean.flatMap(r => exclude(r, gear)); this.cover = cover;
    if (cover === 'none') this.dock = this.regions;
    return this.regions;
  }
  collapse(): void { this.layout(this.fallback(), 'none'); }
  ping(): number { const seq = ++this.seq; this.pending.add(seq); if (this.pending.size > 4) this.pending.delete(this.pending.values().next().value!); return seq; }
  pong(seq: number, now: number): void { if (this.pending.delete(seq)) this.lastPong = now; }
  tick(now: number): 'ok' | 'reload' | 'remove' {
    if (this.removed || now - this.lastPong < 6000) return 'ok';
    this.removed = true;
    if (now - this.lastReload < 300000) return 'remove';
    this.lastReload = now; return 'reload';
  }
}
export const HOST_PREFS_KEY = 'oxp.hostPrefs';
interface HostPrefsStorage { getItem(key: string): string | null; setItem(key: string, value: string): void }
function plainObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && (Object.getPrototypeOf(value) === Object.prototype || Object.getPrototypeOf(value) === null);
}
export function validHostPrefs(value: unknown): value is Record<string, unknown> {
  try {
    if (!plainObject(value)) return false;
    const json = JSON.stringify(value, (_key, item) => {
      if (item === null || typeof item === 'string' || typeof item === 'boolean' || typeof item === 'number' && Number.isFinite(item) || Array.isArray(item) || plainObject(item)) return item;
      throw new Error('Not JSON');
    });
    return new TextEncoder().encode(json).length <= 4096;
  } catch { return false; }
}
export function readHostPrefs(storage: () => HostPrefsStorage): Record<string, unknown> {
  try { const value: unknown = JSON.parse(storage().getItem(HOST_PREFS_KEY) ?? '{}'); return validHostPrefs(value) ? value : {}; } catch { return {}; }
}
export function saveHostPrefs(storage: () => HostPrefsStorage, value: unknown): boolean {
  if (!validHostPrefs(value)) return false;
  try { storage().setItem(HOST_PREFS_KEY, JSON.stringify(value)); return true; } catch { return false; }
}
export type HostMessage = { oxp: 'host/0'; type: string; [key: string]: unknown };
export function validHostMessage(value: unknown): value is HostMessage {
  if (!value || typeof value !== 'object') return false;
  const m = value as HostMessage;
  if (m.oxp !== 'host/0' || typeof m.type !== 'string') return false;
  if (m.type === 'wake') return true;
  if (m.type === 'host-prefs') return validHostPrefs(m.value);
  if (m.type === 'ready') return m.spec === 'oxp-web-plugin/0';
  if (m.type === 'pong') return Number.isSafeInteger(m.seq) && (m.seq as number) >= 0;
  if (m.type === 'layout') return ['none', 'panel', 'fullscreen'].includes(m.cover as string) && Array.isArray(m.regions) && m.regions.length <= 32 && m.regions.every(r => r && ['x', 'y', 'w', 'h'].every(k => typeof r[k] === 'number' && Number.isFinite(r[k]) && (k === 'x' || k === 'y' || r[k] >= 0)));
  if (m.type === 'diagnostic') return ['info', 'warning', 'error'].includes(m.level as string) && typeof m.code === 'string' && m.code.length > 0 && m.code.length <= 256 && typeof m.detail === 'string';
  return false;
}

/** Clear geometry while retaining selected surfaces and unrelated host preferences. */
export function resetHostPositions(value: Record<string, unknown>): Record<string, unknown> {
  const { dock: _dock, companions: _companions, ...rest } = value;
  if (rest.surfaces && typeof rest.surfaces === 'object' && !Array.isArray(rest.surfaces))
    rest.surfaces = Object.fromEntries(Object.keys(rest.surfaces).map(key => [key, {}]));
  return rest;
}
