// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-15

import { expect, it } from 'vitest';
import { emptyVideoHold, holdingVideo, playbackUi, sameVideoFormat, videoHold, type PlaybackUi } from '../receiver/video-hold';
import type { VideoFormat } from '../protocol/json';

const format: VideoFormat = { codec: 'H264', codecString: 'avc1.640028', width: 800, height: 480, config: 'AQ==' };
const playing: PlaybackUi = { state: 'playing', protocol: 'ACTIVE', holding: false };
it('compares decoder fields by value and ignores the frame rate hint', () => {
  expect(sameVideoFormat(format, { ...format, fpsHint: 30 })).toBe(true);
  expect(sameVideoFormat(null, null)).toBe(true);
  expect(sameVideoFormat(format, null)).toBe(false);
  expect(sameVideoFormat(null, format)).toBe(false);
  for (const change of [{ codecString: 'avc1.640029' }, { width: 1024 }, { height: 720 }, { config: 'Ag==' }]) {
    expect(sameVideoFormat(format, { ...format, ...change })).toBe(false);
  }
});
it('does not hold before the first frame', () => {
  for (const event of ['interrupt', 'replace', 'tick'] as const) {
    expect(videoHold(emptyVideoHold, event, 0)).toEqual({ state: emptyVideoHold, clear: false });
  }
});
it('holds exactly three seconds without extending grace on repeated interruptions', () => {
  const frame = videoHold(emptyVideoHold, 'frame', 0).state;
  const hold = videoHold(frame, 'interrupt', 100).state;
  expect(holdingVideo(hold, 3099)).toBe(true); expect(holdingVideo(hold, 3100)).toBe(false);
  expect(videoHold(hold, 'interrupt', 2000).state.deadline).toBe(3100);
  expect(videoHold(hold, 'tick', 3100).clear).toBe(false);
});
it('keeps the format replacement deadline across interruptions and cancels it on render', () => {
  let hold = videoHold(emptyVideoHold, 'frame', 0).state;
  hold = videoHold(hold, 'replace', 100).state;
  hold = videoHold(hold, 'interrupt', 200).state;
  expect(videoHold(hold, 'tick', 3099).clear).toBe(false);
  expect(videoHold(hold, 'tick', 3100)).toEqual({ state: emptyVideoHold, clear: true });
  hold = videoHold(hold, 'frame', 3000).state;
  expect(videoHold(hold, 'tick', 3100).clear).toBe(false); expect(hold.deadline).toBeNull();
});
it('clears immediately when replacement follows an already expired hold', () => {
  const hold = videoHold({ frame: true, deadline: null, clearAt: null }, 'interrupt', 0).state;
  expect(videoHold(hold, 'replace', 4000)).toEqual({ state: emptyVideoHold, clear: true });
});
it('clears on end and cancels every pending deadline', () => {
  const hold = videoHold({ frame: true, deadline: null, clearAt: null }, 'replace', 0).state;
  expect(videoHold(hold, 'end', 100)).toEqual({ state: emptyVideoHold, clear: true });
});
it('keeps the main playing state throughout grace and cancels it only on a rendered frame', () => {
  let state = playbackUi(playing, { type: 'hold', active: true });
  for (const text of ['RECONNECTING', 'Disconnected; reconnecting…', 'Connecting…', 'ACTIVE']) {
    state = playbackUi(state, { type: 'status', text }); expect(state.state).toBe('playing');
  }
  expect(state.holding).toBe(true);
  state = playbackUi(state, { type: 'playing' });
  expect(state).toEqual(playing);
});
it('shows the latest status card on expiry and waits for an actual frame before hiding it', () => {
  let state = playbackUi(playing, { type: 'hold', active: true });
  state = playbackUi(state, { type: 'status', text: 'Disconnected; reconnecting…' });
  state = playbackUi(state, { type: 'hold', active: false }); expect(state.state).toBe('reconnecting');
  state = playbackUi(state, { type: 'status', text: 'ACTIVE' }); expect(state.state).toBe('waiting');
  state = playbackUi(state, { type: 'playing' }); expect(state.state).toBe('playing');
});
it('ends the main playing state immediately during grace', () => {
  const state = playbackUi(playing, { type: 'hold', active: true });
  expect(playbackUi(state, { type: 'ended' })).toEqual({ state: 'waiting', protocol: 'ENDED', holding: false });
});
