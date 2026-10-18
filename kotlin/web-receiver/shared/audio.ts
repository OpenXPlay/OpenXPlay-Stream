// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

/** Streaming conversion with integer phase tracking and a causal low-pass filter for downsampling. */
export class Resampler {
  private readonly samples: Float32Array;
  private readonly kernels: Float32Array;
  private readonly taps: number;
  private readonly phases: number;
  private head = 0;
  private started = false;
  // Next source position relative to the newest sample, in target-rate units.
  private position: number;
  constructor(private readonly sourceRate: number, private readonly targetRate: number, alignDuration = false) {
    // Capture accounts for complete input durations; playback keeps its existing first-sample phase.
    this.position = alignDuration ? sourceRate : targetRate;
    const half = sourceRate > targetRate ? Math.ceil(24 * sourceRate / targetRate) : 0;
    this.taps = half ? 2 * half + 1 : 0;
    this.samples = new Float32Array(Math.max(2, this.taps + 1));
    let a = sourceRate, b = targetRate;
    while (b) { const remainder = a % b; a = b; b = remainder; }
    this.phases = Math.min(1024, targetRate / a);
    this.kernels = new Float32Array(this.taps * (this.phases + 1));
    // Cutoff is 90% of target Nyquist. Blackman gives a generous stopband;
    // scaling the length with the rate ratio keeps the transition width useful.
    const cutoff = 0.45 * targetRate / sourceRate;
    for (let phase = 0; half && phase <= this.phases; phase++) {
      let sum = 0;
      for (let k = 0; k < this.taps; k++) {
        const distance = k + phase / this.phases - half;
        const sinc = distance === 0 ? 2 * cutoff : Math.sin(2 * Math.PI * cutoff * distance) / (Math.PI * distance);
        const window = 0.42 - 0.5 * Math.cos(2 * Math.PI * k / (this.taps - 1))
          + 0.08 * Math.cos(4 * Math.PI * k / (this.taps - 1));
        const value = sinc * window;
        this.kernels[phase * this.taps + k] = value; sum += value;
      }
      for (let k = 0; k < this.taps; k++) this.kernels[phase * this.taps + k] /= sum;
    }
  }
  push(input: Float32Array): Float32Array {
    if (!input.length) return new Float32Array(0);
    const count = Math.max(0, Math.floor((input.length * this.targetRate - this.position) / this.sourceRate) + 1);
    const output = new Float32Array(count);
    let written = 0;
    for (let n = 0; n < input.length; n++) {
      // Extend the first sample backwards to avoid a DC startup transient.
      if (!this.started) { this.samples.fill(input[n]); this.started = true; }
      this.samples[this.head] = input[n];
      this.position -= this.targetRate;
      while (this.position <= 0) {
        const offset = Math.floor(this.position / this.targetRate);
        const remainder = this.position - offset * this.targetRate;
        const fraction = remainder / this.targetRate;
        let index = (this.head + offset + this.samples.length) % this.samples.length;
        let value = 0;
        if (this.taps) {
          const phase = remainder * this.phases / this.targetRate, lower = Math.floor(phase), blend = phase - lower;
          const base = lower * this.taps;
          for (let k = 0; k < this.taps; k++) {
            const coefficient = this.kernels[base + k];
            value += this.samples[index] * (blend === 0 ? coefficient
              : coefficient + blend * (this.kernels[base + this.taps + k] - coefficient));
            if (--index < 0) index = this.samples.length - 1;
          }
        } else {
          value = fraction === 0 ? this.samples[index] : this.samples[index] * (1 - fraction)
            + this.samples[(index + 1) % this.samples.length] * fraction;
        }
        output[written++] = value;
        this.position += this.sourceRate;
      }
      if (++this.head === this.samples.length) this.head = 0;
    }
    return output;
  }
}

export class JitterBuffer {
  private readonly planes: Float32Array[];
  private readonly capacity: number;
  private head = 0;
  private frames = 0;
  private playing = false;
  underruns = 0;
  targetMs = 80;
  private ended = false;
  private accepting = true;
  private elapsedMs = 0;
  private quietSinceMs = 0;
  // Longest recent gap between arrivals on the playback clock (media only); shrinks with each quiet period.
  private pushedAtMs: number | null = null;
  private peakGapMs = 0;
  /**
   * [maxTargetMs] bounds the adaptive target. Speech keeps the low-latency 20 ms creep up to 300 ms; a
   * larger bound (buffered media) also covers the observed arrival gap at once.
   */
  constructor(readonly rate: number, readonly channels: number, readonly maxTargetMs = 300) {
    // Media also holds one more batch above the target (see trim), so its storage covers target + 1.2 s.
    this.capacity = Math.ceil(rate * (maxTargetMs > 300 ? (maxTargetMs + 1200) / 1000 : 0.5));
    this.planes = Array.from({ length: channels }, () => new Float32Array(this.capacity));
  }
  get bufferedMs(): number { return this.frames / this.rate * 1000; }
  // Suppress terminal starvation while a decoder flush can still deliver its final samples.
  beginEnd(): void { this.ended = true; }
  end(): void { this.ended = true; this.accepting = false; }
  get drained(): boolean { return !this.accepting && this.frames === 0; }
  private trim(): void {
    // A Wi-Fi batch lands on top of the target; trimming it at target + 200 ms discarded music every batch.
    const excess = Math.max(0, this.frames - Math.floor(this.rate * (this.targetMs + 200 + this.gapCover(1)) / 1000));
    this.head = (this.head + excess) % this.capacity; this.frames -= excess;
  }
  push(planes: Float32Array[]): void {
    if (!this.accepting) return;
    if (!planes.length || !planes[0].length || !this.capacity) return;
    // Gaps beyond 2 s are pauses, not jitter.
    const gapMs = this.pushedAtMs === null ? 0 : this.elapsedMs - this.pushedAtMs;
    if (gapMs <= 2000) this.peakGapMs = Math.max(this.peakGapMs, gapMs);
    this.pushedAtMs = this.elapsedMs;
    const count = Math.min(planes[0].length, this.capacity), start = planes[0].length - count;
    const excess = Math.max(0, this.frames + count - this.capacity);
    this.head = (this.head + excess) % this.capacity; this.frames -= excess;
    let tail = (this.head + this.frames) % this.capacity;
    for (let i = start; i < planes[0].length; i++) {
      for (let c = 0; c < this.channels; c++) this.planes[c][tail] = planes[c][i];
      if (++tail === this.capacity) tail = 0;
    }
    this.frames += count;
    this.trim();
  }
  private gapCover(margin: number): number { return this.maxTargetMs > 300 ? this.peakGapMs * margin : 0; }
  mix(output: Float32Array[]): void {
    this.elapsedMs += output[0].length / this.rate * 1000;
    if (!this.ended && this.elapsedMs - this.quietSinceMs >= 10000) {
      this.peakGapMs *= 0.75;
      this.targetMs = Math.max(80, this.targetMs - 10, Math.min(this.maxTargetMs, this.gapCover(1.1)));
      this.quietSinceMs = this.elapsedMs; this.trim();
    }
    if (!this.playing && (this.bufferedMs >= this.targetMs || (this.ended && this.frames > 0))) this.playing = true;
    if (!this.playing) return;
    for (let i = 0; i < output[0].length; i++) {
      if (!this.frames) {
        this.playing = false;
        if (!this.ended) {
          // Bursty Wi-Fi delivers in batches; cover the observed gap at once instead of creeping up.
          this.underruns++; this.targetMs = Math.min(this.maxTargetMs, Math.max(this.targetMs + 20, this.gapCover(1.25)));
          this.quietSinceMs = this.elapsedMs;
        }
        return;
      }
      for (let c = 0; c < output.length; c++) output[c][i] += this.planes[Math.min(c, this.channels - 1)][this.head];
      if (++this.head === this.capacity) this.head = 0;
      this.frames--;
    }
  }
}

export class PcmFramer {
  private readonly resampler: Resampler;
  private readonly pending: Float32Array;
  private head = 0;
  private size = 0;
  constructor(sourceRate: number, readonly rate: number, readonly channels: number) {
    this.resampler = new Resampler(sourceRate, rate, true);
    this.pending = new Float32Array(rate / 50);
  }
  push(input: Float32Array): ArrayBuffer[] {
    const samples = this.resampler.push(input), frames: ArrayBuffer[] = [], count = this.pending.length;
    for (let n = 0; n < samples.length; n++) {
      this.pending[(this.head + this.size) % count] = samples[n]; this.size++;
      if (this.size < count) continue;
      const buffer = new ArrayBuffer(count * this.channels * 2), view = new DataView(buffer);
      for (let i = 0; i < count; i++) {
        const value = Math.max(-1, Math.min(1, this.pending[this.head]));
        for (let c = 0; c < this.channels; c++) view.setInt16((i * this.channels + c) * 2, Math.round(value * (value < 0 ? 32768 : 32767)), true);
        if (++this.head === count) this.head = 0;
      }
      this.size = 0; frames.push(buffer);
    }
    return frames;
  }
}
