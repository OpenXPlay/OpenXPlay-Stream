// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

import { afterEach, expect, it, vi } from 'vitest';
afterEach(() => { vi.unstubAllGlobals(); });
it('drains short ended runs in order and reports per-second buffer extrema without tail underruns', async () => {
  vi.resetModules();
  const processors = new Map<string, any>();
  vi.stubGlobal('sampleRate', 1000);
  vi.stubGlobal('AudioWorkletProcessor', class { port = { onmessage: (_event: any) => {} }; });
  vi.stubGlobal('registerProcessor', (name: string, processor: any) => processors.set(name, processor));
  await import('../receiver/audio-worklet');
  const player = new (processors.get('oxp-player'))();
  const link = { postMessage: vi.fn(), onmessage: (_event: any) => {} };
  player.port.onmessage({ data: { port: link } });
  const send = (data: unknown) => link.onmessage({ data });
  const render = (count: number) => {
    const output = [new Float32Array(count)]; player.process([], [output]); return output[0];
  };
  send({ type: 'format', stream: 1, run: 1, rate: 1000, channels: 1 });
  send({ type: 'pcm', stream: 1, run: 1, planes: [new Float32Array(20).fill(.25)] });
  send({ type: 'format', stream: 1, run: 2, rate: 1000, channels: 1 });
  send({ type: 'pcm', stream: 1, run: 2, planes: [new Float32Array(100).fill(.5)] });
  expect(render(10)).toEqual(new Float32Array(10));
  send({ type: 'end', stream: 1, run: 1 });
  expect(render(10)).toEqual(new Float32Array(10).fill(.25));
  expect(render(10)).toEqual(new Float32Array(10).fill(.25));
  send({ type: 'end', stream: 1, run: 2 });
  expect(render(100)).toEqual(new Float32Array(100).fill(.5));
  render(870);
  const stats = link.postMessage.mock.calls.at(-1)![0];
  expect(stats.underruns).toBe(0); expect(stats.bufferedMs).toBeNull();
  expect(stats.streams).toEqual([
    { stream: 1, run: 1, minBufferedMs: 0, maxBufferedMs: 20, targetMs: 80 },
    { stream: 1, run: 2, minBufferedMs: 0, maxBufferedMs: 100, targetMs: 80 },
  ]);
  render(1000); expect(link.postMessage.mock.calls.at(-1)![0].underruns).toBe(0);
});

it('captures exactly 50 frames per input second, skips absent data and terminates stopped processors', async () => {
  for (const inputRate of [44100, 48000]) for (const rate of [16000, 24000, 48000]) {
    vi.resetModules(); const processors = new Map<string, any>();
    vi.stubGlobal('sampleRate', inputRate);
    vi.stubGlobal('AudioWorkletProcessor', class { port = { onmessage: (_event: any) => {}, close: vi.fn() }; });
    vi.stubGlobal('registerProcessor', (name: string, processor: any) => processors.set(name, processor));
    await import('../receiver/audio-worklet');
    const Capture = processors.get('oxp-capture');
    const create = (token: number) => {
      const capture = new Capture({ processorOptions: { rate, channels: 1, token } });
      const link = { postMessage: vi.fn(), close: vi.fn() }; capture.port.onmessage({ data: { port: link } }); return { capture, link };
    };
    const old = create(1);
    for (let second = 0; second < 3; second++) {
      for (let i = 0; i < inputRate; i += 128) {
        old.capture.process([[]]); old.capture.process([]); old.capture.process([[new Float32Array()]]);
        old.capture.process([[new Float32Array(Math.min(128, inputRate - i)).fill(.25)]]);
      }
      expect(old.link.postMessage).toHaveBeenCalledTimes(50 * (second + 1));
    }
    for (const [frame] of old.link.postMessage.mock.calls) { expect(frame.token).toBe(1); expect(frame.data.byteLength).toBe(rate / 50 * 2); }
    old.capture.port.onmessage({ data: { type: 'stop' } }); expect(old.link.close).toHaveBeenCalledOnce();
    const current = create(2);
    for (let i = 0; i < inputRate; i += 128) {
      const samples = new Float32Array(Math.min(128, inputRate - i));
      expect(old.capture.process([[samples]])).toBe(false);
      current.capture.process([[samples]]);
    }
    expect(old.link.postMessage).toHaveBeenCalledTimes(150); expect(current.link.postMessage).toHaveBeenCalledTimes(50);
    // Real silent input remains valid PCM; absent channels never synthesize extra silence.
    expect(new Uint8Array(current.link.postMessage.mock.calls[0][0].data).every(n => n === 0)).toBe(true);
  }
});
