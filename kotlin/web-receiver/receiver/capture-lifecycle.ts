// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

/** Owns a capture and fences permission results across stop, restart and disconnect. */
export class CaptureLifecycle {
  private generation = 0;
  private stream: MediaStream | null = null;
  private source: MediaStreamAudioSourceNode | null = null;
  private node: AudioWorkletNode | null = null;
  stop(): void {
    ++this.generation;
    // Disconnecting alone does not terminate a processor that returns true.
    this.node?.port.postMessage({ type: 'stop' });
    this.stream?.getTracks().forEach(track => track.stop());
    this.source?.disconnect(); this.node?.disconnect();
    this.stream = null; this.source = null; this.node = null;
  }
  begin(): number { this.stop(); return this.generation; }
  current(fence: number): boolean { return this.generation === fence; }
  adopt(fence: number, stream: MediaStream): boolean {
    if (!this.current(fence)) { stream.getTracks().forEach(track => track.stop()); return false; }
    this.stream = stream; return true;
  }
  attach(source: MediaStreamAudioSourceNode, node: AudioWorkletNode): void {
    this.source = source; this.node = node;
  }
}
