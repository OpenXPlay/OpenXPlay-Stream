// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-18

import type { DisplayInfo, DisplaySettings } from '../protocol/json';
import type { Store } from './ui';

type ScreenSize = Pick<DisplayInfo, 'width' | 'height' | 'dpr'>;
/** Some browsers exclude the cutout from screen.width only in fullscreen. */
export class StableScreen {
  private maxima = new Map<string, ScreenSize>();
  constructor(private readonly storage: () => Pick<Store, 'getItem' | 'setItem'>) {}
  measure(width: number, height: number, dpr: number): ScreenSize {
    const next = { width: Math.max(1, Math.round(width * dpr)), height: Math.max(1, Math.round(height * dpr)), dpr };
    const landscape = next.width >= next.height;
    const key = `oxp.screen.${landscape ? 'landscape' : 'portrait'}.${dpr}`;
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

function displayKey(d: DisplayInfo): string {
  return JSON.stringify([d.width, d.height, d.dpr, d.safeArea.top, d.safeArea.right, d.safeArea.bottom, d.safeArea.left, d.viewport, d.fullscreen, d.fullscreenPreferred]);
}
/** Duplicate observations do not postpone a pending report. */
export class DisplayDebounce {
  private reported = '';
  private pending = '';
  private since = 0;
  observe(display: DisplayInfo, now: number): boolean {
    const key = displayKey(display);
    if (key === this.reported) { this.pending = ''; return false; }
    if (key !== this.pending) { this.pending = key; this.since = now; }
    if (now - this.since < 500) return false;
    this.reported = key; this.pending = ''; return true;
  }
  seed(display: DisplayInfo): void { this.reported = displayKey(display); this.pending = ''; }
}
export class DisplaySettingsGate {
  supported = false;
  welcome(features?: string[]): void { this.supported = features?.includes('display-settings') ?? false; }
  message(resolution: DisplaySettings['resolution'], screenWidth: DisplaySettings['screenWidth'], videoCodec: DisplaySettings['videoCodec'] = 'auto', frameRate?: DisplaySettings['frameRate']): DisplaySettings | undefined {
    return this.supported ? { type: 'display-settings', resolution, screenWidth, videoCodec, ...(frameRate !== undefined ? { frameRate } : {}) } : undefined;
  }
}
export function pictureSize(width: number, height: number): string {
  return width && height ? `${width}×${height} (${(width / height).toFixed(2)}:1)` : '—';
}
