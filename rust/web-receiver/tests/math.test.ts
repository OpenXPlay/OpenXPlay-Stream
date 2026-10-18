// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

import { expect, it } from 'vitest';
import { clockSample, latencyMs, mapTouch, videoRect, dropPolicy, renderQueueDepth, renderQueuePolicy } from '../shared/math';
it('maps pillarbox and letterbox coordinates and ignores bar taps', () => {
  const letterbox = videoRect(1000, 1000, 1920, 1080);
  expect(letterbox).toEqual({ x: 0, y: 218.75, width: 1000, height: 562.5 });
  expect(mapTouch(500, 500, letterbox)).toEqual({ x: 0.5, y: 0.5 });
  expect(mapTouch(500, 0, letterbox)).toBeNull();
  expect(mapTouch(500, 0, letterbox, true)).toEqual({ x: 0.5, y: 0 });
  const pillarbox = videoRect(1000, 500, 500, 500);
  expect(pillarbox).toEqual({ x: 250, y: 0, width: 500, height: 500 });
  expect(mapTouch(250, 500, pillarbox)).toEqual({ x: 0, y: 1 });
  expect(mapTouch(0, 0, videoRect(0, 0, 1, 1))).toBeNull();
});
it('computes offset, excludes relay processing from RTT and maps latency', () => {
  expect(clockSample(1000000n, 1501000n, 1501100n, 1002100n)).toEqual({ offsetUs: 500000n, rttUs: 2000n });
  expect(latencyMs(1010000n, 1500000n, 500000n)).toBe(10);
  expect(latencyMs(1n, 2n, null)).toBeNull();
  expect(latencyMs(1n, 2n, 0n)).toBeNull();
  expect(clockSample(10n, 20n, 19n, 30n)).toBeNull();
  expect(clockSample(10n, 20n, 100n, 30n)).toBeNull();
});
it('drops overflow deltas until a keyframe, with queue boundary three', () => {
  expect(dropPolicy(false, false, 3).decode).toBe(true);
  const overflow = dropPolicy(false, false, 4);
  expect(overflow).toEqual({ waiting: true, decode: false, reset: false, requestKeyframe: true });
  expect(dropPolicy(overflow.waiting, false, 0)).toEqual({ waiting: true, decode: false, reset: false, requestKeyframe: true });
  expect(dropPolicy(true, true, 0)).toEqual({ waiting: false, decode: true, reset: false, requestKeyframe: false });
  expect(dropPolicy(false, true, 4)).toEqual({ waiting: false, decode: true, reset: false, requestKeyframe: false });
  expect(dropPolicy(false, false, 0, true)).toEqual({ waiting: true, decode: false, reset: false, requestKeyframe: true });
  expect(dropPolicy(false, true, 0, true)).toEqual({ waiting: false, decode: true, reset: false, requestKeyframe: false });
  expect(dropPolicy(false, true, 0, false, true)).toEqual({ waiting: true, decode: false, reset: true, requestKeyframe: true });
});
it('resumes deltas only after a keyframe following decoder pressure', () => {
  let waiting = false;
  const decisions = [
    { key: false, queue: 3 }, { key: false, queue: 4 }, { key: false, queue: 0 },
    { key: true, queue: 0 }, { key: false, queue: 0 },
  ].map(({ key, queue }) => {
    const decision = dropPolicy(waiting, key, queue);
    waiting = decision.waiting;
    return decision;
  });
  expect(decisions.map(d => d.decode)).toEqual([true, false, false, true, true]);
  expect(decisions.every(d => !d.reset)).toBe(true);
});
it('displays a two-frame burst on consecutive refreshes without mutating the input', () => {
  const first = renderQueuePolicy<number>([], 1, 1);
  const burst = renderQueuePolicy(first.frames, 2, 1);
  expect(first.frames).toEqual([1]);
  expect(burst).toEqual({ frames: [1, 2], skipped: [] });
  const displayed = [burst.frames.shift(), burst.frames.shift()];
  expect(displayed).toEqual([1, 2]);
  expect(burst.frames).toEqual([]);
});
it('keeps only the two newest frames when a render stall overflows the queue', () => {
  const frames = [1, 2];
  const third = renderQueuePolicy(frames, 3, 1);
  expect(frames).toEqual([1, 2]);
  expect(third).toEqual({ frames: [2, 3], skipped: [1] });
  const fourth = renderQueuePolicy(third.frames, 4, 1);
  expect(fourth).toEqual({ frames: [3, 4], skipped: [2] });
  expect(fourth.frames.shift()).toBe(3);
  expect(renderQueuePolicy(fourth.frames, 5, 1)).toEqual({ frames: [4, 5], skipped: [] });
});
it('drops a queued frame that is already late instead of showing it', () => {
  const late = (frame: number) => frame === 1;
  expect(renderQueuePolicy([1], 2, 1, late)).toEqual({ frames: [2], skipped: [1] });
  expect(renderQueuePolicy([2], 3, 1, late)).toEqual({ frames: [2, 3], skipped: [] });
});
it('restores newest-only rendering at depth zero', () => {
  expect(renderQueuePolicy<number>([], 1, 0)).toEqual({ frames: [1], skipped: [] });
  expect(renderQueuePolicy([1], 2, 0)).toEqual({ frames: [2], skipped: [1] });
  expect(renderQueuePolicy([1, 2], 3, 0)).toEqual({ frames: [3], skipped: [1, 2] });
});
it('defaults to one extra frame and bounds fragment queue settings to zero or one', () => {
  expect(renderQueueDepth('0')).toBe(0);
  for (const value of [null, '', '1', '2', '-1', '0.5', 'NaN']) expect(renderQueueDepth(value)).toBe(1);
});
it('uses the same fill crop for video rendering and touch mapping', () => {
  const rect = videoRect(1000, 1000, 1920, 1080, 'fill');
  expect(rect.height).toBe(1000); expect(rect.width).toBeCloseTo(1777.777777);
  expect(mapTouch(500, 500, rect)).toEqual({ x: .5, y: .5 });
  expect(mapTouch(0, 0, rect)!.x).toBeCloseTo(.21875);
});
