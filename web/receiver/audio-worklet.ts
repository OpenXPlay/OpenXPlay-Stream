// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

import type { AudioBufferDebug } from './messages';
import { JitterBuffer, PcmFramer, Resampler } from '../shared/audio';
declare const sampleRate: number;
declare class AudioWorkletProcessor { readonly port: MessagePort; constructor(options?: unknown); }
declare function registerProcessor(name: string, processor: typeof AudioWorkletProcessor): void;
class Player extends AudioWorkletProcessor {
  private streams = new Map<number, { stream: number; buffer: JitterBuffer; converters: Resampler[]; min: number; max: number }>();
  private link?: MessagePort;
  private playing = new Set<number>();
  private ticks = 0;
  private underruns = 0;
  constructor() {
    super();
    this.port.onmessage = event => {
      this.link = event.data.port;
      this.link!.onmessage = ({ data }) => {
        if (data.type === 'clear') { this.streams.clear(); this.underruns = 0; return; }
        if (data.type === 'format') {
          this.streams.set(data.run, { stream: data.stream, buffer: new JitterBuffer(sampleRate, data.channels, data.maxTargetMs),
            converters: Array.from({ length: data.channels }, () => new Resampler(data.rate, sampleRate)), min: 0, max: 0 });
          return;
        }
        const stream = this.streams.get(data.run);
        if (!stream) return;
        if (data.type === 'ending') { stream.buffer.beginEnd(); return; }
        if (data.type === 'end') { stream.buffer.end(); return; }
        if (data.type !== 'pcm') return;
        stream.buffer.push(data.planes.map((plane: Float32Array, c: number) => stream.converters[c].push(plane)));
        stream.min = Math.min(stream.min, stream.buffer.bufferedMs);
        stream.max = Math.max(stream.max, stream.buffer.bufferedMs);
      };
    };
  }
  process(_inputs: Float32Array[][], outputs: Float32Array[][]): boolean {
    const output = outputs[0];
    if (!output?.length) return true;
    // A new run of the same stream waits for the previous decoder and playback tail.
    const playing = this.playing; playing.clear();
    for (const stream of this.streams.values()) {
      if (!playing.has(stream.stream) && !stream.buffer.drained) {
        // Reserve this quantum before mixing, even when its last sample drains the run.
        playing.add(stream.stream);
        stream.buffer.mix(output);
      }
      stream.min = Math.min(stream.min, stream.buffer.bufferedMs);
      stream.max = Math.max(stream.max, stream.buffer.bufferedMs);
    }
    for (const channel of output) for (let i = 0; i < channel.length; i++) channel[i] = Math.max(-1, Math.min(1, channel[i]));
    this.ticks += output[0].length;
    if (this.ticks >= sampleRate) {
      this.ticks -= sampleRate;
      const streams: AudioBufferDebug[] = [];
      let bufferedMs: number | null = null, underruns = this.underruns;
      for (const [run, stream] of this.streams) {
        streams.push({ stream: stream.stream, run, minBufferedMs: stream.min, maxBufferedMs: stream.max, targetMs: stream.buffer.targetMs });
        underruns += stream.buffer.underruns;
        if (!stream.buffer.drained) bufferedMs = Math.max(bufferedMs ?? 0, stream.buffer.bufferedMs);
        stream.min = stream.max = stream.buffer.bufferedMs;
        if (stream.buffer.drained) { this.underruns += stream.buffer.underruns; this.streams.delete(run); }
      }
      this.link?.postMessage({ type: 'audio-stats', bufferedMs, underruns, streams });
    }
    return true;
  }
}
class Capture extends AudioWorkletProcessor {
  private framer: PcmFramer;
  private link?: MessagePort;
  constructor(options: { processorOptions: { rate: number; channels: number; token: number } }) {
    super();
    this.framer = new PcmFramer(sampleRate, options.processorOptions.rate, options.processorOptions.channels);
    this.port.onmessage = event => {
      if (event.data.type === 'stop') { this.stopped = true; this.link?.close(); this.link = undefined; this.port.close(); }
      else if (!this.stopped) this.link = event.data.port;
    };
    this.token = options.processorOptions.token;
  }
  private token: number;
  private stopped = false;
  process(inputs: Float32Array[][]): boolean {
    if (this.stopped) return false;
    if (inputs[0]?.[0]?.length) for (const data of this.framer.push(inputs[0][0])) this.link?.postMessage({ token: this.token, data }, [data]);
    return true;
  }
}
registerProcessor('oxp-player', Player);
registerProcessor('oxp-capture', Capture as unknown as typeof AudioWorkletProcessor);
