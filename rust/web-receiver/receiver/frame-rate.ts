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
