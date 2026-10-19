// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-19

import type { DisplayInfo, FullscreenExit } from '../protocol/json';
import type { Store } from './ui';

export class FullscreenIntent {
  intent: boolean;
  restore = true;
  viewAreas = false;
  failed = false;
  private active = false;
  private last?: DisplayInfo;
  constructor(private readonly available: boolean, private readonly storage: () => Store) {
    this.intent = available;
    try { this.intent = available && storage().getItem('oxp.fullscreen') !== 'false'; this.restore = storage().getItem('oxp.fullscreenRestore') !== 'false'; } catch { }
  }
  get pending(): boolean { return this.available && this.intent && this.restore && !this.active && !this.failed; }
  private persist(key: string, value: boolean): void { try { this.storage().setItem(key, String(value)); } catch { } }
  setRestore(value: boolean): void { this.restore = value; this.persist('oxp.fullscreenRestore', value); }
  changed(active: boolean, display?: DisplayInfo): boolean {
    const exit = this.active && !active && this.intent;
    if (exit || active) this.failed = false;
    this.active = active;
    if (active) { this.intent = true; this.persist('oxp.fullscreen', true); if (display) this.last = display; }
    return exit;
  }
  denied(): void { this.failed = true; }
  toggle(request: () => void, exit: () => void): void {
    this.intent = this.available && !this.intent; this.persist('oxp.fullscreen', this.intent);
    if (this.intent) { this.failed = false; request(); } else if (this.active) exit();
  }
  pointerDown(event: Pick<PointerEvent, 'preventDefault' | 'stopImmediatePropagation'>, request: () => void): boolean {
    if (!this.pending) return false;
    event.preventDefault(); event.stopImmediatePropagation(); request(); return true;
  }
  display(real: DisplayInfo): DisplayInfo {
    if (real.fullscreen) this.last = real;
    if (this.viewAreas || !this.pending || !this.last) return real;
    return { ...real, width: this.last.width, height: this.last.height, dpr: this.last.dpr,
      safeArea: this.last.safeArea, viewport: this.last.viewport, fullscreen: true };
  }
}
export class FullscreenExitHistory {
  private hiddenAt: number | null = null;
  private visibleAt: number | null = null;
  private duration: number | null = null;
  visibility(hidden: boolean, now: number): void {
    if (hidden) { this.hiddenAt = now; this.visibleAt = null; this.duration = null; }
    else if (this.hiddenAt !== null) { this.duration = now - this.hiddenAt; this.hiddenAt = null; this.visibleAt = now; }
  }
  take(now: number): number | null {
    const result = this.hiddenAt !== null ? now - this.hiddenAt : this.visibleAt !== null && now - this.visibleAt <= 2000 ? this.duration : null;
    this.duration = null; this.visibleAt = null; return result;
  }
}
export async function autoRestoreFullscreen(options: {
  enabled: () => boolean; restored: () => boolean; hidden: () => boolean;
  visible: () => Promise<void>; request: () => Promise<void>; delay: (ms: number) => Promise<void>;
}): Promise<NonNullable<FullscreenExit['autoRestore']>> {
  for (const retry of [false, true]) {
    if (retry) await options.delay(500);
    while (options.hidden()) await options.visible();
    if (!options.enabled()) return 'off';
    if (options.restored()) return 'restored';
    try { await options.request(); return 'restored'; } catch { }
  }
  return !options.enabled() ? 'off' : options.restored() ? 'restored' : 'denied';
}
