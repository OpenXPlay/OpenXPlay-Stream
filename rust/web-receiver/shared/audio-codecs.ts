// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

export interface AudioConfig { codec: string; sampleRate: number; numberOfChannels: number; description?: Uint8Array }
export interface DecodedAudio {
  numberOfChannels: number; numberOfFrames: number; sampleRate: number;
  copyTo(destination: Float32Array, options: { planeIndex: number; format: 'f32-planar' }): void;
  close(): void;
}
export interface AudioDecoderInstance {
  state: string; decodeQueueSize: number;
  configure(config: AudioConfig): void; decode(chunk: unknown): void; flush(): Promise<void>; close(): void;
}
export const audioCodecs = globalThis as unknown as {
  AudioDecoder?: {
    new(init: { output(data: DecodedAudio): void; error(error: Error): void }): AudioDecoderInstance;
    isConfigSupported(config: AudioConfig): Promise<{ supported?: boolean }>;
  };
  EncodedAudioChunk: new(init: { type: 'key'; timestamp: number; data: Uint8Array }) => unknown;
};
export const aacConfig: AudioConfig = { codec: 'mp4a.40.2', sampleRate: 48000, numberOfChannels: 2, description: new Uint8Array([0x11, 0x90]) };
