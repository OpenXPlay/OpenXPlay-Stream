// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

import { afterEach, expect, it, vi } from 'vitest';
import { collectReceiverInfo, codecPowerEfficient } from '../shared/receiver-info';
import { bestEffort } from '../shared/best-effort';
import { codecCandidates } from '../shared/capabilities';
import { parseMessage } from '../protocol/json';

afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers(); });
it('bounds rejected and stalled probes and clears timeout handles', async () => {
  vi.useFakeTimers();
  expect(await bestEffort(() => { throw new Error('blocked'); })).toBeUndefined();
  expect(await bestEffort(() => 7)).toBe(7);
  const stalled = bestEffort(() => new Promise(() => {}), 50);
  await vi.advanceTimersByTimeAsync(50); expect(await stalled).toBeUndefined();
  expect(vi.getTimerCount()).toBe(0);
});
it('collects technical fields independently, bounds blocked APIs, and probes every codec/size/rate', async () => {
  vi.useFakeTimers();
  const configs = vi.fn(async () => ({ supported: true }));
  const decodingInfo = vi.fn(async (_config: MediaDecodingConfiguration) => ({ supported: true, smooth: true, powerEfficient: true }));
  const high = vi.fn(() => new Promise(() => {}));
  vi.stubGlobal('navigator', { userAgent: 'test', language: 'en', hardwareConcurrency: 4, deviceMemory: 2, maxTouchPoints: 5,
    userAgentData: { brands: [{ brand: 'test', version: '1' }], mobile: true, platform: 'Android', getHighEntropyValues: high },
    getBattery: () => Promise.reject(new Error('denied')), gpu: { requestAdapter: async () => ({ info: { vendor: 'v', architecture: 'a', device: 'd', description: 'GPU', extra: 'omit' } }) },
    mediaCapabilities: { decodingInfo }, connection: { type: 'wifi', effectiveType: '4g', downlink: 10, rtt: 5, saveData: false, extra: 'omit' } });
  vi.stubGlobal('VideoDecoder', { isConfigSupported: configs });
  vi.stubGlobal('screen', { width: 800, height: 480, availWidth: 800, availHeight: 450, colorDepth: 24, orientation: { type: 'landscape-primary' } });
  vi.stubGlobal('requestAnimationFrame', () => 1); vi.stubGlobal('cancelAnimationFrame', vi.fn());
  const pending = collectReceiverInfo();
  await vi.advanceTimersByTimeAsync(2000); const report = await pending;
  expect(report).toMatchObject({ type: 'receiver-info', userAgent: 'test', hardwareConcurrency: 4, deviceMemory: 2,
    userAgentData: { brands: [{ brand: 'test', version: '1' }], mobile: true, platform: 'Android' },
    webgpu: { vendor: 'v', architecture: 'a', device: 'd', description: 'GPU' }, screen: { width: 800, height: 480 },
    network: { type: 'wifi', effectiveType: '4g' } });
  expect(report).not.toHaveProperty('battery'); expect(report.webgpu).not.toHaveProperty('extra');
  expect(configs).toHaveBeenCalledTimes(codecCandidates.length * 2 * 2);
  expect(decodingInfo).toHaveBeenCalledTimes(codecCandidates.length * 2 * 2);
  expect(report.videoDecode).toHaveLength(codecCandidates.length * 2);
  for (const [{ video }] of decodingInfo.mock.calls) {
    expect(video).toBeDefined();
    expect([30, 60]).toContain(video!.framerate); expect(video!.bitrate).toBe(4_000_000);
    expect(video!.contentType).toMatch(/^video\/mp4; codecs="/);
  }
  expect(parseMessage(JSON.stringify(report), 'client-to-relay')).toEqual(JSON.parse(JSON.stringify(report)));
  expect(await codecPowerEfficient(codecCandidates[0])).toBe(true);
  vi.stubGlobal('navigator', {}); expect(await codecPowerEfficient(codecCandidates[0])).toBe(false);
});
it('accepts minimal reports and old stats, permits future diagnostics, bounds UTF-8 bytes, and enforces direction', () => {
  expect(parseMessage({ type: 'receiver-info' }, 'client-to-relay')).toEqual({ type: 'receiver-info' });
  expect(() => parseMessage({ type: 'receiver-info' }, 'relay-to-client')).toThrow();
  expect(() => parseMessage({ type: 'receiver-info', userAgent: '字'.repeat(22000) })).toThrow();
});
