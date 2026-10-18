// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-18

import { expect, it } from 'vitest';
import { parseMessage, type ViewArea } from '../protocol/json';
import { videoRect, mapTouch } from '../shared/math';
const area: ViewArea = { type: 'view-area', x: 120, y: 40, width: 1440, height: 900, canvasWidth: 1920, canvasHeight: 1080 };
it('validates bounded canvas rectangles and direction', () => {
  expect(parseMessage(area, 'relay-to-client')).toEqual(area);
  expect(() => parseMessage(area, 'client-to-relay')).toThrow();
  for (const bad of [{ x: -1 }, { y: .5 }, { width: 0 }, { height: null }, { canvasWidth: 1500 },
    { canvasHeight: 900 }, { x: '0' }, { extra: true }, { canvasWidth: Infinity }])
    expect(() => parseMessage({ ...area, ...bad })).toThrow();
});
it('fits the active rectangle, rejects bars and maps its offset into canvas coordinates', () => {
  const r = videoRect(800, 800, 1920, 1080, 'fit', area);
  expect(r).toEqual({ x: 0, y: 150, width: 800, height: 500 });
  expect(mapTouch(0, 149, r, false, area)).toBeNull();
  expect(mapTouch(0, 150, r, false, area)).toEqual({ x: 120 / 1920, y: 40 / 1080 });
  expect(mapTouch(800, 650, r, false, area)).toEqual({ x: 1560 / 1920, y: 940 / 1080 });
  expect(mapTouch(-100, 900, r, true, area)).toEqual({ x: 120 / 1920, y: 940 / 1080 });
});
it('fill covers the viewport and maps touches to the visible middle of the active area', () => {
  const r = videoRect(800, 800, 1920, 1080, 'fill', area);
  expect(r).toEqual({ x: -240, y: 0, width: 1280, height: 800 });
  expect(mapTouch(400, 400, r, false, area)).toEqual({ x: 840 / 1920, y: 490 / 1080 });
  expect(mapTouch(0, 0, r, false, area)?.x).toBeCloseTo(390 / 1920);
});
it('validates the exit diagnostic including a missing hidden interval', () => {
  const display = { screen: 0, width: 1920, height: 1080, dpr: 1, refreshHz: 60, fullscreen: false,
    viewport: { width: 1000, height: 900 }, safeArea: { top: 0, right: 0, bottom: 0, left: 0 } };
  for (const hiddenMs of [null, 0, 1500.5]) parseMessage({ type: 'fullscreen-exit', hiddenMs, display }, 'client-to-relay');
  for (const hiddenMs of [-1, '100', undefined]) expect(() => parseMessage({ type: 'fullscreen-exit', hiddenMs, display })).toThrow();
  expect(() => parseMessage({ type: 'fullscreen-exit', hiddenMs: null, display }, 'relay-to-client')).toThrow();
});
