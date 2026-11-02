// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-18

import type { DisplayInfo, FullscreenExit } from '../protocol/json';
import type { Store } from './ui';

/** Browser exits preserve intent; only the settings toggle turns it off. */
export class FullscreenIntent {
  intent: boolean;
  private on = false;
  restore = true;
  viewAreas = false;
  failed = false;
  private pointers = new Map<number, boolean>();
  private last?: DisplayInfo;
  constructor(private readonly available: boolean, private readonly storage: () => Store) {
    let saved: string | null = null;
    try { saved = storage().getItem('oxp.fullscreen'); } catch { }
    this.intent = saved === 'false' ? false : available;
    try { this.restore = storage().getItem('oxp.fullscreenRestore') !== 'false'; } catch { }
  }
  get pending(): boolean { return this.available && this.intent && !this.on && this.restore && !this.failed; }
  setRestore(value: boolean): void {
    this.restore = value;
    try { this.storage().setItem('oxp.fullscreenRestore', String(value)); } catch { }
  }
  private save(value: boolean): void {
    this.intent = value;
    try { this.storage().setItem('oxp.fullscreen', String(value)); } catch { }
  }
  changed(on: boolean, display?: DisplayInfo): boolean {
    const involuntary = this.on && !on && this.intent;
    this.on = on;
    if (on || involuntary) this.failed = false;
    if (on) { this.save(true); if (display) this.last = display; }
    return involuntary;
  }
  denied(): void { if (this.intent && !this.on) this.failed = true; }
  toggle(request: () => void, exit: () => void): void {
    this.failed = false;
    if (this.intent) { this.save(false); if (this.on) exit(); }
    else if (this.available) { this.save(true); request(); }
  }
  pointerDown(event: Pick<PointerEvent, 'pointerId' | 'pointerType' | 'preventDefault' | 'stopImmediatePropagation'>, request: () => void): boolean {
    this.pointers.delete(event.pointerId);
    if (!this.pending) return false;
    const deferred = event.pointerType !== 'mouse';
    this.pointers.set(event.pointerId, deferred);
    event.preventDefault(); event.stopImmediatePropagation();
    if (!deferred) request();
    return true;
  }
  pointerMove(event: Pick<PointerEvent, 'pointerId' | 'preventDefault' | 'stopImmediatePropagation'>): boolean {
    if (!this.pointers.has(event.pointerId)) return false;
    event.preventDefault(); event.stopImmediatePropagation(); return true;
  }
  pointerUp(event: Pick<PointerEvent, 'pointerId' | 'preventDefault' | 'stopImmediatePropagation'>, request: () => void, cancelled = false): boolean {
    const deferred = this.pointers.get(event.pointerId);
    if (deferred === undefined) return false;
    this.pointers.delete(event.pointerId);
    event.preventDefault(); event.stopImmediatePropagation();
    if (deferred && !cancelled && this.pending) request();
    return true;
  }
  display(real: DisplayInfo): DisplayInfo {
    if (this.viewAreas) return real;
    if (real.fullscreen) { this.last = real; return real; }
    if (!this.pending || !this.last) return real;
    const { width, height, dpr, safeArea, viewport } = this.last;
    return { ...real, width, height, dpr, safeArea, viewport, fullscreen: true };
  }
}

export class FullscreenExitHistory {
  private hiddenAt: number | null = null;
  private visibleAt: number | null = null;
  private hiddenMs: number | null = null;
  visibility(hidden: boolean, now: number): void {
    if (hidden) { this.hiddenAt = now; this.visibleAt = null; this.hiddenMs = null; }
    else if (this.hiddenAt !== null) { this.hiddenMs = now - this.hiddenAt; this.hiddenAt = null; this.visibleAt = now; }
  }
  take(now: number): number | null {
    const duration = this.hiddenAt !== null ? now - this.hiddenAt :
      this.visibleAt !== null && now - this.visibleAt <= 2000 ? this.hiddenMs : null;
    this.hiddenMs = null; this.visibleAt = null; return duration;
  }
}

/** One silent attempt per exit, with one delayed retry; visibility and intent are checked again before each call. */
export async function autoRestoreFullscreen(options: {
  enabled: () => boolean; restored: () => boolean; hidden: () => boolean;
  visible: () => Promise<void>; request: () => Promise<void>; delay: (ms: number) => Promise<void>;
}): Promise<NonNullable<FullscreenExit['autoRestore']>> {
  for (let attempt = 0; attempt < 2; attempt++) {
    while (options.hidden()) await options.visible();
    if (!options.enabled()) return 'off';
    if (options.restored()) return 'restored';
    try { await options.request(); return 'restored'; } catch { }
    if (attempt === 0) await options.delay(500);
  }
  if (!options.enabled()) return 'off';
  return options.restored() ? 'restored' : 'denied';
}
