// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

import type { Hello, PlaybackMode, VideoFormat, Stats } from '../protocol/json';
import type { TouchContact } from '../protocol/binary';
export interface AudioBufferDebug {
  stream: number; run: number; minBufferedMs: number; maxBufferedMs: number; targetMs: number;
}
export interface AudioDebug {
  stream: number; packets: number; maxArrivalGapMs: number; decoderOutputs: number; maxDecoderOutputGapMs: number;
}
export type PageCommand = import('../protocol/json').FullscreenExit | import('../protocol/json').DisplaySettings |
  { type: 'receiver-info'; info: import('../protocol/json').ReceiverInfo } |
  { type: 'page-metrics'; metrics: Partial<Stats> } |
  { type: 'init'; canvas: OffscreenCanvas; url: string; hello: Hello; queueDepth: 0 | 1 } |
  { type: 'resize'; width: number; height: number } |
  { type: 'hello'; hello: Hello } |
  { type: 'touch'; contacts: TouchContact[] } |
  { type: 'visibility'; visible: boolean } |
  { type: 'audio-port'; port: MessagePort } |
  { type: 'audio-probe' } |
  { type: 'mic-detach' } |
  { type: 'mic-port'; port: MessagePort; token: number } |
  { type: 'reconnect' } |
  { type: 'restart' } |
  { type: 'fit'; fit: 'fit' | 'fill' } |
  { type: 'stop' };
export type WorkerNotice = { type: 'view-area'; area: import('../protocol/json').ViewArea | undefined } | { type: 'display-disconnected' } | import('../protocol/json').DisplaySettings |
  { type: 'retry'; delayMs: number; attempt: number } |
  { type: 'playing' } |
  { type: 'hold'; active: boolean } |
  { type: 'ended' } |
  { type: 'audio-support'; aac: boolean } |
  { type: 'audio-active'; active: boolean } |
  { type: 'mic'; stream: number; request: import('../protocol/json').MicFormat | null; token: number } |
  { type: 'status'; text: string } |
  { type: 'welcome'; receiverBuild?: string; features?: string[]; relay?: { name: string; version: string }; plugins?: { host: string }; sourceUrl: string } |
  { type: 'format'; format: VideoFormat | null } |
  { type: 'stats'; stats: Stats; skipped: number; renderQueue: number; queueDepth: number; playback: PlaybackMode; audio: AudioDebug[]; audioBuffers: AudioBufferDebug[] };
