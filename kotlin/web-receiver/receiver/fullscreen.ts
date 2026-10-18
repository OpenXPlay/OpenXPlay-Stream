// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-18

import type { DisplayInfo } from '../protocol/json';
import type { Store } from './ui';

/** Browser exits preserve intent; only the settings toggle turns it off. */
export class FullscreenIntent {
  intent: boolean;
  private on = false;
  private last?: DisplayInfo;
  constructor(private readonly available: boolean, private readonly storage: () => Store) {
    let saved: string | null = null;
    try { saved = storage().getItem('oxp.fullscreen'); } catch { }
    this.intent = saved === 'false' ? false : available;
  }
  get pending(): boolean { return this.available && this.intent && !this.on; }
  private save(value: boolean): void {
    this.intent = value;
    try { this.storage().setItem('oxp.fullscreen', String(value)); } catch { }
  }
  changed(on: boolean, display?: DisplayInfo): void {
    this.on = on;
    if (on) { this.save(true); if (display) this.last = display; }
  }
  toggle(request: () => void, exit: () => void): void {
    if (this.intent) { this.save(false); if (this.on) exit(); }
    else if (this.available) { this.save(true); request(); }
  }
  pointerDown(event: Pick<PointerEvent, 'preventDefault' | 'stopImmediatePropagation'>, request: () => void): boolean {
    if (!this.pending) return false;
    event.preventDefault(); event.stopImmediatePropagation(); request(); return true;
  }
  display(real: DisplayInfo): DisplayInfo {
    if (real.fullscreen) { this.last = real; return real; }
    if (!this.pending || !this.last) return real;
    const { width, height, dpr, safeArea, viewport } = this.last;
    return { ...real, width, height, dpr, safeArea, viewport, fullscreen: true };
  }
}
