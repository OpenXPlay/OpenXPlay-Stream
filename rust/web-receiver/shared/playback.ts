// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

import type { Hello, PlaybackMode } from '../protocol/json';
import { QUEUE_LATENCY_BUDGET_MS } from './math';

export const playbackModes: readonly PlaybackMode[] = ['SMART', 'QUALITY', 'SPEED'];
export function playbackMode(value: string | null | undefined): PlaybackMode {
  return playbackModes.includes(value as PlaybackMode) ? value as PlaybackMode : 'SMART';
}
/** SMART is the protocol default and stays off the wire, so a default receiver also works with older relays. */
export function withPlayback(hello: Hello, mode: PlaybackMode): Hello {
  const { playback: _, ...rest } = hello;
  return mode === 'SMART' ? rest : { ...rest, playback: mode };
}

export interface PlaybackParameters {
  /** Bounds for decoded frames held behind the one due for display; Smart adapts between them. */
  minDepth: number; maxDepth: number;
  /** A queued frame older than this, from relay receipt to now, gives way to a newer one. */
  lateBudgetMs: number;
  /** Frames waiting for the decoder before deltas are dropped until the next key frame. */
  maxDecodeQueue: number;
  /** Hold encoded frames as the jitter buffer and present decoded ones at their timestamp cadence. */
  paced: boolean;
}
export const playbackParameters: Readonly<Record<PlaybackMode, Readonly<PlaybackParameters>>> = {
  // Newest decoded frame only, with the shortest decode queue.
  SPEED: { minDepth: 0, maxDepth: 0, lateBudgetMs: 50, maxDecodeQueue: 2, paced: false },
  SMART: { minDepth: 0, maxDepth: 2, lateBudgetMs: QUEUE_LATENCY_BUDGET_MS, maxDecodeQueue: 3, paced: false },
  // Three decoded frames spare hardware decoder buffers; a second of encoded frames counts as far behind.
  QUALITY: { minDepth: 3, maxDepth: 3, lateBudgetMs: 1000, maxDecodeQueue: 60, paced: true },
};
/** Quality's jitter buffer; a longer backlog is shown at the refresh rate until it fits again. */
export const QUALITY_BUFFER_MS = 150;
// Refresh callbacks arrive with a few milliseconds of jitter; waiting for the exact spacing would skip a refresh.
const PACE_SLACK_MS = 4;
// Larger timestamp steps are discontinuities (a new stream or a paused phone), not a cadence.
const MAX_SPACING_MS = 250;

export interface MediaTime { at: number; ptsUs: number }
/** Whether Quality shows the next frame now: once its timestamp spacing has elapsed, or at once to catch up. */
export function paceDue(now: number, last: MediaTime | null, nextPtsUs: number, backlogMs: number): boolean {
  if (!last || backlogMs > QUALITY_BUFFER_MS) return true;
  const spacing = (nextPtsUs - last.ptsUs) / 1000;
  if (!(spacing > 0 && spacing <= MAX_SPACING_MS)) return true;
  return now - last.at >= spacing - PACE_SLACK_MS;
}

// Smart evaluates one-second windows. A static CarPlay screen sends about one frame per second and gives no evidence.
const MIN_SAMPLES = 10;
const LATE_FRAMES = 3, LATE_SHARE = 0.05;
const RAISE_WINDOWS = 2, LOWER_WINDOWS = 5, RAISE_HOLD_WINDOWS = 5;
function percentile(values: number[], fraction: number): number {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * fraction))] ?? 0;
}
/**
 * Smart render depth (0-2). Arrival jitter relative to the timestamp cadence asks for depth; late frames mean the
 * buffer delays frames that are already late, so they lower it at once and hold further raises for five seconds.
 * Raising needs two consecutive windows and lowering five, so single bursts do not toggle the depth.
 */
export class SmartDepth {
  depth: number;
  private previous: MediaTime | null = null;
  private jitter: number[] = [];
  private spacing: number[] = [];
  private frames = 0;
  private late = 0;
  private raise = 0;
  private lower = 0;
  private hold = 0;
  constructor(initial = 1) { this.depth = Math.max(0, Math.min(2, Math.round(initial))); }
  arrival(at: number, ptsUs: number): void {
    this.frames++;
    const previous = this.previous; this.previous = { at, ptsUs };
    if (!previous) return;
    const spacing = (ptsUs - previous.ptsUs) / 1000;
    if (!(spacing > 0 && spacing <= MAX_SPACING_MS)) return;
    this.spacing.push(spacing);
    this.jitter.push(Math.abs(at - previous.at - spacing));
  }
  /** A decoded frame was skipped because it was already late. */
  lateFrame(): void { this.late++; }
  /** A decoder reset or gap breaks the arrival sequence. */
  restart(): void { this.previous = null; }
  /** Called once per second; returns the depth for the next window. */
  evaluate(): number {
    const enough = this.jitter.length >= MIN_SAMPLES;
    const target = enough ? Math.min(2, Math.round(percentile(this.jitter, 0.9) / percentile(this.spacing, 0.5))) : this.depth;
    const late = this.late >= LATE_FRAMES && this.late >= this.frames * LATE_SHARE;
    this.jitter = []; this.spacing = []; this.frames = 0; this.late = 0;
    if (this.hold > 0) this.hold--;
    if (late) {
      this.raise = 0; this.lower = 0; this.hold = RAISE_HOLD_WINDOWS;
      this.depth = Math.max(0, this.depth - 1);
    } else if (target > this.depth && this.hold === 0) {
      this.lower = 0;
      if (++this.raise >= RAISE_WINDOWS) { this.depth++; this.raise = 0; }
    } else if (target < this.depth) {
      this.raise = 0;
      if (++this.lower >= LOWER_WINDOWS) { this.depth--; this.lower = 0; }
    } else { this.raise = 0; this.lower = 0; }
    return this.depth;
  }
}
