// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

import { codecCandidates, estimateRefreshHz } from './capabilities';
import type { ReceiverInfo } from '../protocol/json';
import { aacConfig, audioCodecs } from './audio-codecs';
import { bestEffort } from './best-effort';
export async function mediaDecode(codec: string, width: number, height: number, framerate: number): Promise<MediaCapabilitiesDecodingInfo | undefined> {
  return bestEffort(() => navigator.mediaCapabilities.decodingInfo({ type: 'file', video: {
    contentType: `video/mp4; codecs="${codec}"`, width, height, framerate, bitrate: 4_000_000,
  } }));
}
export async function codecPowerEfficient(codec: string): Promise<boolean> {
  return (await mediaDecode(codec, 1280, 720, 30))?.powerEfficient ?? false;
}
// These interfaces describe optional APIs absent from some TypeScript DOM libraries.
interface ExtraNavigator {
  deviceMemory?: number;
  userAgentData?: { brands?: unknown; mobile?: boolean; platform?: string; getHighEntropyValues(keys: string[]): Promise<Record<string, unknown>> };
  getBattery?: () => Promise<{ level: number; charging: boolean }>;
  connection?: { type?: string; effectiveType?: string; downlink?: number; rtt?: number; saveData?: boolean };
  gpu?: { requestAdapter(): Promise<{ info?: Record<string, unknown>; requestAdapterInfo?: () => Promise<Record<string, unknown>> } | null> };
}
export async function collectReceiverInfo(): Promise<ReceiverInfo> {
  const info: ReceiverInfo = { type: 'receiver-info' }, n = navigator as Navigator & ExtraNavigator;
  const tasks: Promise<unknown>[] = [];
  const field = (key: string, probe: () => unknown) => tasks.push(bestEffort(probe, 2000).then(value => { if (value !== undefined) info[key] = value; }));
  for (const key of ['userAgent', 'language', 'hardwareConcurrency', 'deviceMemory', 'maxTouchPoints'] as const) field(key, () => n[key]);
  field('isSecureContext', () => isSecureContext); field('crossOriginIsolated', () => crossOriginIsolated);
  field('userAgentData', async () => {
    const data = n.userAgentData; if (!data) return undefined;
    const result: Record<string, unknown> = { brands: data.brands, mobile: data.mobile, platform: data.platform };
    const high = await bestEffort(() => data.getHighEntropyValues(['platform', 'platformVersion', 'model', 'architecture', 'bitness', 'fullVersionList']));
    for (const key of ['platform', 'platformVersion', 'model', 'architecture', 'bitness', 'fullVersionList']) if (high?.[key] !== undefined) result[key] = high[key];
    return result;
  });
  for (const [key, query] of Object.entries({ pointerFine: '(pointer: fine)', pointerCoarse: '(pointer: coarse)', pointerNone: '(pointer: none)', hover: '(hover: hover)', anyPointerFine: '(any-pointer: fine)', anyPointerCoarse: '(any-pointer: coarse)', anyHover: '(any-hover: hover)' })) field(key, () => matchMedia(query).matches);
  field('battery', async () => { const b = await n.getBattery?.(); return b && { level: b.level, charging: b.charging }; });
  field('screen', () => ({ width: screen.width, height: screen.height, availWidth: screen.availWidth, availHeight: screen.availHeight, colorDepth: screen.colorDepth, orientation: screen.orientation?.type }));
  field('devicePixelRatio', () => devicePixelRatio); field('innerWidth', () => innerWidth); field('innerHeight', () => innerHeight);
  field('visualViewportScale', () => visualViewport?.scale); field('refreshHz', estimateRefreshHz);
  for (const kind of ['webgl', 'webgl2'] as const) field(kind, () => {
    const canvas = document.createElement('canvas'), gl = canvas.getContext(kind) as WebGLRenderingContext | WebGL2RenderingContext | null;
    if (!gl) return { available: false };
    try {
      const debug = gl.getExtension('WEBGL_debug_renderer_info');
      return { available: true, vendor: gl.getParameter(gl.VENDOR), renderer: gl.getParameter(gl.RENDERER), maxTextureSize: gl.getParameter(gl.MAX_TEXTURE_SIZE),
        unmaskedVendor: debug ? gl.getParameter(debug.UNMASKED_VENDOR_WEBGL) : undefined, unmaskedRenderer: debug ? gl.getParameter(debug.UNMASKED_RENDERER_WEBGL) : undefined };
    } finally { gl.getExtension('WEBGL_lose_context')?.loseContext(); }
  });
  field('webgpu', async () => {
    const adapter = await n.gpu?.requestAdapter(); if (!adapter) return undefined;
    const data = adapter.info ?? await adapter.requestAdapterInfo?.();
    return Object.fromEntries(['vendor', 'architecture', 'device', 'description'].map(key => [key, data?.[key]]));
  });
  field('videoDecoder', () => typeof VideoDecoder !== 'undefined'); field('videoFrame', () => typeof VideoFrame !== 'undefined');
  field('offscreenCanvas', () => typeof OffscreenCanvas !== 'undefined'); field('requestVideoFrameCallback', () => 'requestVideoFrameCallback' in HTMLVideoElement.prototype);
  field('videoDecode', () => Promise.all(codecCandidates.flatMap(codec => [ [1280, 720], [1920, 1080] ].map(async ([width, height]) => {
    const support = (hardwareAcceleration: HardwareAcceleration) => bestEffort(async () => (await VideoDecoder.isConfigSupported({ codec, codedWidth: width, codedHeight: height, hardwareAcceleration })).supported);
    const [preferHardware, preferSoftware, fps30, fps60, mediaSource] = await Promise.all([
      support('prefer-hardware'), support('prefer-software'), mediaDecode(codec, width, height, 30), mediaDecode(codec, width, height, 60),
      bestEffort(() => MediaSource.isTypeSupported(`video/mp4; codecs="${codec}"`)),
    ]);
    const summary = (v: MediaCapabilitiesDecodingInfo | undefined) => v && ({ supported: v.supported, smooth: v.smooth, powerEfficient: v.powerEfficient });
    return { codec, width, height, preferHardware, preferSoftware, fps30: summary(fps30), fps60: summary(fps60), mediaSource };
  }))));
  field('audio', async () => {
    const context = await bestEffort(() => new AudioContext());
    if (!context) return undefined;
    try { return { sampleRate: context.sampleRate, baseLatency: context.baseLatency, outputLatency: context.outputLatency, audioWorklet: !!context.audioWorklet }; }
    finally { void bestEffort(() => context.close()); }
  });
  field('audioDecoder', () => Promise.all(['mp4a.40.2', 'opus'].map(async codec => ({ codec, supported: await bestEffort(async () => {
    return (await audioCodecs.AudioDecoder?.isConfigSupported(codec === 'mp4a.40.2' ? aacConfig : { codec, sampleRate: 48000, numberOfChannels: 2 }))?.supported;
  }) }))));
  field('network', () => { const c = n.connection; return c && { type: c.type, effectiveType: c.effectiveType, downlink: c.downlink, rtt: c.rtt, saveData: c.saveData }; });
  field('memory', () => {
    const m = (performance as Performance & { memory?: { jsHeapSizeLimit: number; totalJSHeapSize: number; usedJSHeapSize: number } }).memory;
    return m && { jsHeapSizeLimit: m.jsHeapSizeLimit, totalJSHeapSize: m.totalJSHeapSize, usedJSHeapSize: m.usedJSHeapSize };
  });
  await Promise.all(tasks); return info;
}
