// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-15

import type { VideoFormat } from '../protocol/json';
import { protocolStatus, type StatusState } from './ui';

export const VIDEO_GRACE_MS = 3000;
export function sameVideoFormat(a: VideoFormat | null, b: VideoFormat | null): boolean {
  return a === b || !!a && !!b && a.codecString === b.codecString && a.width === b.width && a.height === b.height && a.config === b.config;
}
export interface VideoHold { frame: boolean; deadline: number | null; clearAt: number | null }
export const emptyVideoHold: VideoHold = { frame: false, deadline: null, clearAt: null };
export type HoldEvent = 'interrupt' | 'replace' | 'frame' | 'end' | 'tick';
export function videoHold(state: VideoHold, event: HoldEvent, now: number): { state: VideoHold; clear: boolean } {
  if (event === 'end') return { state: emptyVideoHold, clear: true };
  if (event === 'frame') return { state: { frame: true, deadline: null, clearAt: null }, clear: false };
  if (!state.frame) return { state, clear: false };
  if (event === 'tick') {
    const clear = state.clearAt !== null && now >= state.clearAt;
    return { state: clear ? emptyVideoHold : state, clear };
  }
  const deadline = state.deadline ?? now + VIDEO_GRACE_MS;
  const clearAt = event === 'replace' ? state.clearAt ?? deadline : state.clearAt;
  const clear = clearAt !== null && now >= clearAt;
  return { state: clear ? emptyVideoHold : { ...state, deadline, clearAt }, clear };
}
export function holdingVideo(state: VideoHold, now: number): boolean {
  return state.frame && state.deadline !== null && now < state.deadline;
}
export interface PlaybackUi { state: StatusState; protocol: string; holding: boolean }
export type PlaybackEvent = { type: 'status'; text: string } | { type: 'playing' } | { type: 'hold'; active: boolean } | { type: 'ended' };
export function playbackUi(state: PlaybackUi, event: PlaybackEvent): PlaybackUi {
  if (event.type === 'playing') return { ...state, state: 'playing', holding: false };
  if (event.type === 'ended') return { state: 'waiting', protocol: 'ENDED', holding: false };
  if (event.type === 'hold') return { ...state, holding: event.active,
    state: event.active ? state.state : protocolStatus(state.protocol, false) };
  return { ...state, protocol: event.text, state: state.holding ? state.state : protocolStatus(event.text, state.state === 'playing') };
}
