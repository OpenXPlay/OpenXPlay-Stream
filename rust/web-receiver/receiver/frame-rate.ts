// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

import type { FrameRatePreference, Hello } from '../protocol/json';
export const frameRates: readonly FrameRatePreference[] = ['AUTO', 'FPS_30', 'FPS_35', 'FPS_40', 'FPS_45', 'FPS_50', 'FPS_55', 'FPS_60'];
export function frameRatePreference(value: string | null): FrameRatePreference {
  return frameRates.includes(value as FrameRatePreference) ? value as FrameRatePreference : 'AUTO';
}
export function withFrameRate(hello: Hello, preference: FrameRatePreference): Hello {
  return { ...hello, displays: hello.displays.map(display => {
    const { frameRate: _, ...rest } = display;
    return preference === 'AUTO' ? rest : { ...rest, frameRate: preference };
  }) };
}
export function effectiveFrameRate(display: Hello['displays'][number] | undefined): number {
  const preference = display?.frameRate ?? 'AUTO';
  if (preference !== 'AUTO') return Number(preference.slice(4));
  return (display?.refreshHz ?? 60) < 50 || (display?.sustainedFps ?? 60) < 50 ? 30 : 60;
}
/** A monotonic clock keeps the restart limit independent of wall-clock changes. */
export class RestartGate {
  private last = -Infinity;
  allow(now: number): boolean {
    if (now - this.last < 10_000) return false;
    this.last = now; return true;
  }
}

/** A relay value owns both the control and future hellos until that socket disconnects. */
export class FrameRateSync {
  private settings?: import('../protocol/json').DisplaySettings;
  constructor(public preference: FrameRatePreference, private readonly gate: RestartGate) {}
  get synced(): boolean { return this.settings?.frameRate !== undefined; }
  receive(settings: import('../protocol/json').DisplaySettings): void {
    this.settings = settings;
    if (settings.frameRate !== undefined) this.preference = settings.frameRate;
  }
  disconnect(): void { this.settings = undefined; }
  hello(hello: Hello): Hello { return withFrameRate(hello, this.synced ? 'AUTO' : this.preference); }
  change(preference: FrameRatePreference, hello: Hello, initialized: boolean, supported: boolean, now: number): {
    displaySettings?: import('../protocol/json').DisplaySettings; hello?: Hello; restart?: true;
  } {
    if (preference === this.preference) return {};
    if (this.synced) {
      if (!initialized || !supported || !this.gate.allow(now)) return {};
      return { displaySettings: { ...this.settings!, frameRate: preference } };
    }
    if (initialized && !this.gate.allow(now)) return {};
    this.preference = preference;
    return initialized ? { hello: this.hello(hello), restart: true } : {};
  }
}
