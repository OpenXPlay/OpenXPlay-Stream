// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

import { expect, it } from 'vitest';
import { JitterBuffer, PcmFramer, Resampler } from '../shared/audio';
import { audioCapabilities } from '../shared/capabilities';
import { parseMessage } from '../protocol/json';
import { readFileSync } from 'node:fs';
it('waits for 80 ms, mixes channels, outputs silence on underrun and prebuffers again', () => {
  const buffer = new JitterBuffer(1000, 1);
  const render = (n: number) => { const out = [new Float32Array(n), new Float32Array(n)]; buffer.mix(out); return out; };
  buffer.push([new Float32Array(40).fill(.25)]);
  expect(render(10)[0]).toEqual(new Float32Array(10));
  buffer.push([new Float32Array(40).fill(.25)]);
  const output = render(90);
  expect([...output[0].slice(0, 80)]).toEqual(Array(80).fill(.25));
  expect(output[1]).toEqual(output[0]); expect(output[0][80]).toBe(0); expect(buffer.underruns).toBe(1);
  buffer.push([new Float32Array(99).fill(1)]); expect(render(10)[0][0]).toBe(0);
  buffer.push([new Float32Array(1).fill(1)]); expect(render(10)[0][0]).toBe(1);
});
it('drops the oldest samples at target + 200 ms and mixes independent streams', () => {
  const a = new JitterBuffer(1000, 1), b = new JitterBuffer(1000, 2);
  a.push([Float32Array.from({ length: 300 }, (_, i) => i)]);
  expect(a.bufferedMs).toBe(280);
  b.push([new Float32Array(80).fill(1), new Float32Array(80).fill(2)]);
  const output = [new Float32Array(10), new Float32Array(10)]; a.mix(output); b.mix(output);
  expect(output[0][0]).toBe(21); expect(output[1][0]).toBe(22);
});
it('preserves resampling phase across arbitrary input boundaries', () => {
  for (const [source, target] of [[48000, 16000], [44100, 48000], [8000, 48000]]) {
    const input = Float32Array.from({ length: 1000 }, (_, i) => Math.sin(i / 10));
    const full = new Resampler(source, target).push(input), split = new Resampler(source, target);
    const output: number[] = [];
    for (let i = 0; i < input.length; i += 37) output.push(...split.push(input.slice(i, i + 37)));
    expect(output.length).toBe(full.length);
    output.forEach((value, i) => expect(value).toBeCloseTo(full[i], 5));
  }
});
it('frames exactly 20 ms, saturates S16LE and duplicates mono into requested channels', () => {
  const framer = new PcmFramer(48000, 16000, 2);
  const frames = framer.push(new Float32Array(1921).fill(-2));
  expect(frames).toHaveLength(2);
  for (const frame of frames) {
    expect(frame.byteLength).toBe(320 * 2 * 2);
    const view = new DataView(frame);
    for (let i = 0; i < frame.byteLength; i += 2) expect(view.getInt16(i, true)).toBe(-32768);
  }
  expect(framer.push(new Float32Array(1))).toEqual([]);
});
it('maps only working worklets, supported AAC and available capture to hello', () => {
  expect(audioCapabilities(false, true, true)).toEqual({ audio: [], microphone: false });
  expect(audioCapabilities(true, false, false)).toEqual({ audio: ['PCM_S16LE'], microphone: false });
  expect(audioCapabilities(true, true, true)).toEqual({ audio: ['PCM_S16LE', 'AAC_LC'], microphone: true });
});
it('accepts optional audio stats and rejects invalid counters and durations', () => {
  const stats = JSON.parse(readFileSync(new URL('../spec/vectors/web/stats.json', import.meta.url), 'utf8'));
  for (const audioBufferedMs of [null, 0, 60.5]) expect(parseMessage({ ...stats, audioBufferedMs })).toBeTruthy();
  const { audioBufferedMs, audioUnderruns, ...old } = stats; expect(parseMessage(old)).toEqual(old);
  for (const value of [-1, .5, null, '1']) expect(() => parseMessage({ ...stats, audioUnderruns: value })).toThrow();
  for (const value of [-1, Infinity, '1']) expect(() => parseMessage({ ...stats, audioBufferedMs: value })).toThrow();
});

function tone(frequency: number, rate: number, count: number): Float32Array {
  return Float32Array.from({ length: count }, (_, i) => Math.sin(2 * Math.PI * frequency * i / rate));
}
function rms(samples: Float32Array): number {
  let sum = 0;
  for (const sample of samples) sum += sample * sample;
  return Math.sqrt(sum / samples.length);
}
function frequency(samples: Float32Array, rate: number): number {
  let first = 0, last = 0, crossings = 0;
  for (let i = 1; i < samples.length; i++) {
    if (samples[i - 1] <= 0 && samples[i] > 0) {
      const crossing = i - 1 - samples[i - 1] / (samples[i] - samples[i - 1]);
      if (!crossings) first = crossing;
      last = crossing; crossings++;
    }
  }
  return (crossings - 1) * rate / (last - first);
}
it('preserves sine frequency and passband gain at 48 kHz to 16 and 24 kHz', () => {
  for (const rate of [16000, 24000]) for (const hz of [1000, 3000, 5000]) {
    const output = new Resampler(48000, rate).push(tone(hz, 48000, 48000)).subarray(1000);
    expect(frequency(output, rate)).toBeCloseTo(hz, 1);
    expect(rms(output)).toBeCloseTo(Math.SQRT1_2, 3);
  }
});
it('attenuates the aliased 10 kHz tone by at least 40 dB at 48 kHz to 16 kHz', () => {
  const input = tone(10000, 48000, 48000);
  const output = new Resampler(48000, 16000).push(input).subarray(1000);
  const attenuationDb = -20 * Math.log10(rms(output) / rms(input));
  expect(attenuationDb).toBeGreaterThanOrEqual(40);
});
it('retains filter history and exact phase through tiny, empty and irregular chunks', () => {
  for (const [source, target] of [[48000, 16000], [48000, 24000], [44100, 16000], [48000, 44100], [96001, 16000]]) {
    const input = tone(3100, source, source + 17);
    const full = new Resampler(source, target).push(input), split = new Resampler(source, target);
    const output = new Float32Array(full.length), sizes = [1, 2, 127, 13, 4096, 37];
    let written = 0, chunk = 0;
    for (let i = 0; i < input.length;) {
      expect(split.push(new Float32Array(0))).toHaveLength(0);
      const size = sizes[chunk++ % sizes.length], part = split.push(input.subarray(i, i + size));
      output.set(part, written); written += part.length; i += size;
    }
    expect(written).toBe(Math.floor((input.length - 1) * target / source) + 1);
    expect(output).toEqual(full);
  }
});
it('keeps the linear playback path and its phase unchanged', () => {
  for (const [source, target] of [[16000, 48000], [44100, 48000], [48000, 48000]]) {
    const input = Float32Array.from({ length: 1000 }, (_, i) => Math.sin(i / 10));
    const resampler = new Resampler(source, target), parts: number[] = [];
    for (let i = 0; i < input.length; i += 17) parts.push(...resampler.push(input.subarray(i, i + 17)));
    expect(parts).toHaveLength(Math.floor((input.length - 1) * target / source) + 1);
    for (let i = 0; i < parts.length; i++) {
      const position = i * source, index = Math.floor(position / target), fraction = (position % target) / target;
      const expected = fraction === 0 ? input[index] : input[index] * (1 - fraction) + input[index + 1] * fraction;
      expect(parts[i]).toBeCloseTo(expected, 7);
    }
  }
});
it('wraps jitter storage, drops overflow, and counts underruns only when rendering past the end', () => {
  const buffer = new JitterBuffer(1000, 2);
  const push = (start: number, count: number) => buffer.push([
    Float32Array.from({ length: count }, (_, i) => start + i),
    Float32Array.from({ length: count }, (_, i) => -start - i),
  ]);
  const render = (count: number) => { const out = [new Float32Array(count), new Float32Array(count)]; buffer.mix(out); return out; };
  push(0, 200); render(150); push(200, 200);
  const wrapped = render(250);
  expect(wrapped[0]).toEqual(Float32Array.from({ length: 250 }, (_, i) => i + 150));
  expect(wrapped[1]).toEqual(Float32Array.from({ length: 250 }, (_, i) => -i - 150));
  expect(buffer.underruns).toBe(0); render(1); expect(buffer.underruns).toBe(1);
  push(400, 70); push(470, 500);
  expect(buffer.bufferedMs).toBe(300);
  expect(render(300)[0]).toEqual(Float32Array.from({ length: 300 }, (_, i) => i + 670));
});
it('keeps 20 ms framing, channel duplication and PCM bytes independent of capture chunks', () => {
  for (const rate of [16000, 24000]) {
    const input = tone(1000, 48000, 48017), full = new PcmFramer(48000, rate, 2).push(input);
    const framer = new PcmFramer(48000, rate, 2), split: ArrayBuffer[] = [];
    for (let i = 0; i < input.length; i += 128) split.push(...framer.push(input.subarray(i, i + 128)));
    expect(split).toEqual(full); expect(split).toHaveLength(50);
    for (const frame of split) {
      expect(frame.byteLength).toBe(rate / 50 * 4);
      const view = new DataView(frame);
      for (let i = 0; i < frame.byteLength; i += 4) expect(view.getInt16(i, true)).toBe(view.getInt16(i + 2, true));
    }
    expect(new DataView(new PcmFramer(rate, rate, 1).push(new Float32Array(rate / 50).fill(2))[0]).getInt16(0, true)).toBe(32767);
  }
});

it('raises the target on starvation, caps it at 300 ms, and decays once per quiet 10 s', () => {
  const buffer = new JitterBuffer(1000, 1);
  const mix = (count: number) => buffer.mix([new Float32Array(count)]);
  expect(buffer.targetMs).toBe(80);
  for (let i = 0; i < 12; i++) {
    buffer.push([new Float32Array(buffer.targetMs)]); mix(buffer.targetMs + 1);
    expect(buffer.targetMs).toBe(Math.min(300, 100 + i * 20));
  }
  expect(buffer.underruns).toBe(12);
  mix(9999); expect(buffer.targetMs).toBe(300);
  mix(1); expect(buffer.targetMs).toBe(290);
  mix(9999); expect(buffer.targetMs).toBe(290);
  mix(1); expect(buffer.targetMs).toBe(280);
  for (let i = 0; i < 25; i++) mix(10000);
  expect(buffer.targetMs).toBe(80);
});
it('drains ended tails below the target and never counts their silence as starvation', () => {
  for (const count of [0, 20, 80, 160]) {
    const buffer = new JitterBuffer(1000, 1);
    buffer.push([new Float32Array(count).fill(.5)]); buffer.end();
    const output = [new Float32Array(count + 10)]; buffer.mix(output);
    expect(output[0].slice(0, count)).toEqual(new Float32Array(count).fill(.5));
    for (let i = 0; i < 12; i++) buffer.mix([new Float32Array(1000)]);
    expect(buffer.drained).toBe(true); expect(buffer.underruns).toBe(0); expect(buffer.targetMs).toBe(80);
  }
});
it('settles after at most two underruns with 200 ms bursts every 200 ms', () => {
  const buffer = new JitterBuffer(1000, 1);
  // Join mid-burst, then model decoder callbacks delivering ten 20 ms packets together.
  buffer.push([new Float32Array(80)]);
  for (let time = 0; time < 9000; time += 10) {
    if (time > 0 && time % 200 === 0) {
      for (let packet = 0; packet < 10; packet++) buffer.push([new Float32Array(20)]);
    }
    buffer.mix([new Float32Array(10)]);
  }
  expect(buffer.underruns).toBeGreaterThan(0); expect(buffer.underruns).toBeLessThanOrEqual(2);
  expect(buffer.targetMs).toBeGreaterThan(80);
});
it('uses the raised target for overflow and retains the newest samples', () => {
  const buffer = new JitterBuffer(1000, 1);
  buffer.push([new Float32Array(80)]); buffer.mix([new Float32Array(81)]);
  buffer.push([Float32Array.from({ length: 500 }, (_, i) => i)]);
  expect(buffer.bufferedMs).toBe(300);
  const out = [new Float32Array(1)]; buffer.mix(out); expect(out[0][0]).toBe(200);
});

it('suppresses starvation during a delayed terminal flush and accepts its last PCM', () => {
  const buffer = new JitterBuffer(1000, 1);
  buffer.push([new Float32Array(80)]); buffer.mix([new Float32Array(80)]);
  buffer.beginEnd(); buffer.mix([new Float32Array(1000)]);
  expect(buffer.underruns).toBe(0); expect(buffer.targetMs).toBe(80); expect(buffer.drained).toBe(false);
  buffer.push([new Float32Array(20).fill(.5)]); buffer.end();
  const out = [new Float32Array(30)]; buffer.mix(out);
  expect(out[0].slice(0, 20)).toEqual(new Float32Array(20).fill(.5));
  expect(buffer.underruns).toBe(0); expect(buffer.drained).toBe(true);
});

it('bursty media arrival settles without underruns, speech stays capped', () => {
  for (const [max, settles] of [[1000, true], [300, false]] as const) {
    const buffer = new JitterBuffer(1000, 1, max), out = [new Float32Array(10)];
    // 400 ms of audio arrives every 400 ms in one batch, the pattern seen from an iPhone over Wi-Fi.
    const cycle = () => { buffer.push([new Float32Array(400).fill(0.1)]); for (let i = 0; i < 40; i++) buffer.mix(out); };
    for (let n = 0; n < 20; n++) cycle();
    const before = buffer.underruns;
    for (let n = 0; n < 20; n++) cycle();
    expect(buffer.underruns === before).toBe(settles);
    expect(buffer.targetMs <= max).toBe(true);
  }
});
it('keeps a whole Wi-Fi batch above the media target instead of trimming it', () => {
  const buffer = new JitterBuffer(1000, 1, 1000), out = [new Float32Array(10)];
  let kept = 0;
  for (let cycle = 0; cycle < 30; cycle++) {
    buffer.push([new Float32Array(400).fill(0.1)]);
    for (let i = 0; i < 40; i++) { out[0].fill(0); buffer.mix(out); kept += out[0].filter(v => v !== 0).length; }
  }
  // After the target settles, every batch is played: later cycles lose nothing.
  const before = kept;
  for (let cycle = 0; cycle < 10; cycle++) {
    buffer.push([new Float32Array(400).fill(0.1)]);
    for (let i = 0; i < 40; i++) { out[0].fill(0); buffer.mix(out); kept += out[0].filter(v => v !== 0).length; }
  }
  expect(kept - before).toBe(4000);
});
