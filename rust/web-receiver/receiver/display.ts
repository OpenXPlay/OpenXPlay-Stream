// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-19

import type { DisplayInfo, DisplaySettings } from '../protocol/json';

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
