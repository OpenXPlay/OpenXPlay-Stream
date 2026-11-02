// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-19

import type { DisplayInfo, DisplaySettings } from '../protocol/json';
import type { Store } from './ui';

type ScreenSize = { width: number; height: number; dpr: number };
export class StableScreen {
  private maxima = new Map<string, ScreenSize>();
  constructor(private readonly storage: () => Pick<Store, 'getItem' | 'setItem'>) { }
  measure(width: number, height: number, dpr: number): ScreenSize {
    const next = { width: Math.max(1, Math.round(width * dpr)), height: Math.max(1, Math.round(height * dpr)), dpr };
    const landscape = next.width >= next.height;
    const key = `oxp-screen-max:${dpr}:${landscape ? 'landscape' : 'portrait'}`;
    let last = this.maxima.get(key);
    if (!last) {
      try {
        const saved = JSON.parse(this.storage().getItem(key) ?? 'null');
        if (saved && Number.isInteger(saved.width) && saved.width > 0 && Number.isInteger(saved.height) && saved.height > 0 &&
            saved.dpr === dpr && (saved.width >= saved.height) === landscape) last = saved;
      } catch { }
    }
    if (last) { next.width = Math.max(last.width, next.width); next.height = Math.max(last.height, next.height); }
    this.maxima.set(key, next);
    if (!last || last.width !== next.width || last.height !== next.height) {
      try { this.storage().setItem(key, JSON.stringify(next)); } catch { }
    }
    return { ...next };
  }
}

function signature(display: DisplayInfo): string {
  const { width, height, dpr, safeArea, viewport, fullscreen, fullscreenPreferred } = display;
  return JSON.stringify([width, height, dpr, safeArea, viewport, fullscreen, fullscreenPreferred]);
}
export class DisplayDebounce {
  private sent = '';
  private candidate = '';
  private changedAt = 0;
  seed(display: DisplayInfo): void { this.sent = signature(display); this.candidate = ''; }
  observe(display: DisplayInfo, now: number): boolean {
    const next = signature(display);
    if (next === this.sent) { this.candidate = ''; return false; }
    if (next !== this.candidate) { this.candidate = next; this.changedAt = now; }
    if (now - this.changedAt < 500) return false;
    this.seed(display); return true;
  }
}
export class DisplaySettingsGate {
  supported = false;
  welcome(features?: string[]): void { this.supported = features?.includes('display-settings') === true; }
  message(resolution: DisplaySettings['resolution'], screenWidth: DisplaySettings['screenWidth'], videoCodec: DisplaySettings['videoCodec'], frameRate?: DisplaySettings['frameRate']): DisplaySettings | undefined {
    if (this.supported) return { type: 'display-settings', resolution, screenWidth, videoCodec, ...(frameRate === undefined ? {} : { frameRate }) };
  }
}
export function pictureSize(width: number, height: number): string {
  return width && height ? `${width}×${height} (${(width / height).toFixed(2)}:1)` : '—';
}
