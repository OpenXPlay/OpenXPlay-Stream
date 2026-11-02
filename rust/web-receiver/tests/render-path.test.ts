// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-11-01

import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { BinaryKind, KEYFRAME, encodeBinary } from '../protocol/binary';
import type { PlaybackMode, Stats } from '../protocol/json';

const REFRESH_MS = 1000 / 60, WARMUP_MS = 6000, END_MS = 16000;
const vector = (name: string) => JSON.parse(readFileSync(new URL(`../spec/vectors/web/${name}.json`, import.meta.url), 'utf8'));
interface Frame { timestamp: number; displayWidth: number; displayHeight: number; close(): void }
interface Sample { ptsUs: number; arrivedAt: number; readyAt: number; decodedAt: number; shownAt?: number; closes: number }
type Pattern = 'steady' | 'pairs' | 'decoder-pairs' | 'jitter' | 'static' | 'stall';
let now = 0;
let scope: { onmessage(event: { data: unknown }): void; postMessage: ReturnType<typeof vi.fn>; requestAnimationFrame(draw: () => void): void };
let draws: (() => void)[];
class Socket {
  static OPEN = 1;
  static current: Socket;
  readyState = 1; bufferedAmount = 0;
  onopen = () => {}; onclose = () => {}; onmessage = (_event: { data: unknown }) => {};
  sent: Stats[] = [];
  constructor() { Socket.current = this; }
  send(value: unknown) {
    if (typeof value !== 'string') return;
    const message = JSON.parse(value);
    if (message.type === 'stats') this.sent.push(message);
    if (message.type === 'ping') this.receive({ type: 'pong', t0Us: message.t0Us, t1Us: message.t0Us, t2Us: message.t0Us });
  }
  close() {}
  receive(value: unknown) { this.onmessage({ data: value instanceof ArrayBuffer ? value : JSON.stringify(value) }); }
}
class Decoder {
  static current: Decoder;
  static async isConfigSupported(config: unknown) { return { supported: true, config }; }
  state = 'configured'; decodeQueueSize = 0;
  submitted = new Set<number>();
  onSubmit = (_timestamp: number) => {};
  constructor(public callbacks: { output(frame: Frame): void; error(): void }) { Decoder.current = this; }
  configure() {}
  close() { this.state = 'closed'; }
  decode(chunk: { timestamp: number }) { this.submitted.add(chunk.timestamp); this.onSubmit(chunk.timestamp); }
}
const command = (data: unknown) => scope.onmessage({ data });
beforeEach(async () => {
  vi.resetModules(); vi.useFakeTimers(); now = 0; draws = [];
  vi.spyOn(performance, 'now').mockImplementation(() => now);
  scope = { onmessage() {}, postMessage: vi.fn(), requestAnimationFrame(draw) { draws.push(draw); } };
  vi.stubGlobal('self', scope); vi.stubGlobal('WebSocket', Socket); vi.stubGlobal('VideoDecoder', Decoder);
  vi.stubGlobal('EncodedVideoChunk', class { constructor(public init: { timestamp: number }) {} get timestamp() { return this.init.timestamp; } });
  await import('../receiver/worker');
});
afterEach(() => { command({ type: 'stop' }); vi.clearAllTimers(); vi.useRealTimers(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

async function simulate(mode: PlaybackMode, pattern: Pattern) {
  const samples = new Map<number, Sample>();
  const context = { fillRect() {}, drawImage(frame: Frame) { samples.get(frame.timestamp)!.shownAt = now; } };
  const hello = { ...vector('hello'), playback: mode };
  command({ type: 'init', canvas: { width: 1920, height: 864, getContext: () => context }, hello, url: 'ws://localhost', queueDepth: 1 });
  const socket = Socket.current; socket.onopen();
  socket.receive(vector('welcome')); socket.receive(vector('video-format'));
  await Promise.resolve(); await Promise.resolve();
  const decoder = Decoder.current;
  const emitted = new Set<number>();
  const emit = (sample: Sample) => {
    if (!decoder.submitted.has(sample.ptsUs) || emitted.has(sample.ptsUs)) return;
    emitted.add(sample.ptsUs); sample.decodedAt = now;
    decoder.callbacks.output({ timestamp: sample.ptsUs, displayWidth: 1920, displayHeight: 864, close() { sample.closes++; } });
  };
  decoder.onSubmit = timestamp => { const sample = samples.get(timestamp)!; if (sample.decodedAt <= now) emit(sample); };
  const events: { at: number; run(): void }[] = [];
  for (let index = 0; index < 960; index++) {
    const at = 2 + index * REFRESH_MS, measuring = at >= WARMUP_MS;
    if (measuring && pattern === 'static' && index % 60 !== 0) continue;
    const ptsUs = Math.round(at * 1000);
    const pairedAt = 2 + (index - index % 2 + 1) * REFRESH_MS;
    const jitter = [0, 4, 0, 12, 0, 0, 8, 0][index % 8];
    const arrivedAt = measuring && pattern === 'pairs' ? pairedAt : at + (measuring && pattern === 'jitter' ? jitter : 0);
    let decodedAt = (measuring && pattern === 'decoder-pairs' ? pairedAt : arrivedAt) + 4;
    // A 100 ms decoder stall releases six pending outputs together, without losing encoded input.
    if (pattern === 'stall' && at >= 8000 && at < 8100) decodedAt = 8106;
    const sample: Sample = { ptsUs, arrivedAt, readyAt: decodedAt, decodedAt, closes: 0 }; samples.set(ptsUs, sample);
    events.push({ at: arrivedAt, run() {
      socket.receive(encodeBinary({ kind: BinaryKind.Video, stream: 0, flags: index === 0 ? KEYFRAME : 0,
        sequence: index, ptsUs: BigInt(ptsUs), senderClockUs: BigInt(ptsUs) }, new Uint8Array([1])).buffer);
    } });
    events.push({ at: decodedAt, run() { emit(sample); } });
  }
  // The real worker chooses and queues each frame; this display only runs callbacks at fixed refreshes.
  for (let tick = 1; tick <= 1020; tick++) events.push({ at: tick * REFRESH_MS, run() { const pending = draws; draws = []; pending.forEach(draw => draw()); } });
  events.sort((a, b) => a.at - b.at);
  let clock = 0;
  for (const event of events) {
    now = event.at;
    const next = Math.round(now);
    await vi.advanceTimersByTimeAsync(next - clock); clock = next;
    event.run();
  }
  const decoded = [...samples.values()].filter(sample => sample.arrivedAt >= WARMUP_MS && sample.arrivedAt < END_MS && emitted.has(sample.ptsUs));
  const shown = decoded.filter(sample => sample.shownAt !== undefined);
  const waits = shown.map(sample => sample.shownAt! - sample.decodedAt).sort((a, b) => a - b);
  // Added latency excludes the first eligible refresh; it includes Quality's wait before decoder submission.
  const added = shown.map(sample => sample.shownAt! - Math.ceil(sample.readyAt / REFRESH_MS) * REFRESH_MS);
  const percentile = (fraction: number) => waits[Math.min(waits.length - 1, Math.floor(waits.length * fraction))] ?? 0;
  const metrics = { mode, pattern, decoded: decoded.length, shown: shown.length,
    ratio: shown.length / decoded.length, waitP50Ms: percentile(.5), waitP95Ms: percentile(.95),
    addedMaxMs: Math.max(0, ...added) };
  expect(decoded.every(sample => sample.closes === 1)).toBe(true);
  expect(new Set(shown.map(sample => sample.shownAt)).size).toBe(shown.length);
  const drops = socket.sent.reduce((sum, stats) => sum + (stats.renderSupersededFrames ?? 0) + (stats.renderLateFrames ?? 0) +
    (stats.renderResetFrames ?? 0) + (stats.renderInactiveFrames ?? 0), 0);
  const allDecoded = [...samples.values()].filter(sample => emitted.has(sample.ptsUs));
  return { metrics, drops, unshown: allDecoded.filter(sample => sample.shownAt === undefined).length };
}

const patterns: Pattern[] = ['steady', 'pairs', 'decoder-pairs', 'jitter', 'static', 'stall'];
it.each((['SPEED', 'SMART', 'QUALITY'] as const).flatMap(mode => patterns.map(pattern => ({ mode, pattern }))))('simulates $mode $pattern', async ({ mode, pattern }) => {
  // Each invocation starts with six smooth seconds, enough to exercise Smart's evidence windows.
  const { metrics, drops, unshown } = await simulate(mode, pattern);
  console.info(JSON.stringify(metrics));
  expect(metrics.decoded).toBe(pattern === 'static' ? 10 : 600);
  if (mode === 'SMART') {
    expect(metrics.ratio).toBeGreaterThanOrEqual(.95);
    expect(metrics.addedMaxMs).toBeLessThanOrEqual(REFRESH_MS + .001);
  } else if (mode === 'SPEED') {
    expect(metrics.shown).toBe({ steady: 600, pairs: 300, 'decoder-pairs': 300, jitter: 525, static: 10, stall: 594 }[pattern]);
    expect(metrics.addedMaxMs).toBeCloseTo(0);
  } else {
    expect(metrics.shown).toBe(metrics.decoded);
    expect(metrics.addedMaxMs).toBeLessThanOrEqual(150);
  }
  expect(drops).toBe(unshown);
});
