// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

import { expect, it, vi } from 'vitest';
import { CaptureLifecycle } from '../receiver/capture-lifecycle';
it('stops the processor, tracks and source on replacement and rejects a late permission grant', () => {
  const lifecycle = new CaptureLifecycle();
  const track = { stop: vi.fn() }, stream = { getTracks: () => [track] } as unknown as MediaStream;
  const source = { disconnect: vi.fn() } as unknown as MediaStreamAudioSourceNode;
  const node = { port: { postMessage: vi.fn() }, disconnect: vi.fn() } as unknown as AudioWorkletNode;
  const first = lifecycle.begin(); expect(lifecycle.adopt(first, stream)).toBe(true); lifecycle.attach(source, node);
  const second = lifecycle.begin();
  expect(node.port.postMessage).toHaveBeenCalledWith({ type: 'stop' });
  expect(track.stop).toHaveBeenCalledOnce(); expect(source.disconnect).toHaveBeenCalledOnce(); expect(node.disconnect).toHaveBeenCalledOnce();
  expect(lifecycle.adopt(first, stream)).toBe(false); expect(track.stop).toHaveBeenCalledTimes(2);
  lifecycle.stop(); expect(lifecycle.adopt(second, stream)).toBe(false);
  expect(lifecycle.current(second)).toBe(false);
});
