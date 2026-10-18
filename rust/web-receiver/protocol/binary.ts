// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

export class WireFormatError extends Error {
  constructor(message: string) { super(message); this.name = 'WireFormatError'; }
}
export enum BinaryKind { Video = 1, Audio = 2, Microphone = 3, Touch = 16, Key = 17 }
export enum KeyCode { Home, Back, Select, KnobLeft, KnobRight, KnobUp, KnobDown, PlayPause, Next, Previous, Voice }
export const HEADER_SIZE = 24;
export const KEYFRAME = 1;
export const GAP = 2;
export interface BinaryHeader {
  kind: BinaryKind; stream: number; flags: number; sequence: number; ptsUs: bigint; senderClockUs: bigint;
}
export interface TouchContact { id: number; pressed: boolean; x: number; y: number }
export interface KeyPayload { code: KeyCode; down: boolean }
export function check(condition: unknown, message: string): asserts condition {
  if (!condition) throw new WireFormatError(message);
}
const integer = (n: number, min: number, max: number) => Number.isInteger(n) && n >= min && n <= max;
const kinds = [1, 2, 3, 16, 17];
function validateHeader(h: BinaryHeader): void {
  check(kinds.includes(h.kind), 'Unknown message kind');
  check(integer(h.stream, 0, 255), 'Stream must fit u8');
  check(integer(h.flags, 0, 3), 'Unknown header flags');
  check(integer(h.sequence, 0, 0xffffffff), 'Sequence must fit u32');
  for (const clock of [h.ptsUs, h.senderClockUs])
    check(typeof clock === 'bigint' && clock >= -(1n << 63n) && clock < (1n << 63n), 'Clock must fit i64');
}
function view(b: Uint8Array): DataView { return new DataView(b.buffer, b.byteOffset, b.byteLength); }
export function encodeHeader(h: BinaryHeader): Uint8Array {
  validateHeader(h);
  const b = new Uint8Array(HEADER_SIZE), v = view(b);
  b[0] = h.kind; b[1] = h.stream; b[2] = h.flags;
  v.setUint32(4, h.sequence, true); v.setBigInt64(8, h.ptsUs, true); v.setBigInt64(16, h.senderClockUs, true);
  return b;
}
export function decodeHeader(b: Uint8Array): BinaryHeader {
  check(b.length === HEADER_SIZE, 'Header must be 24 bytes');
  check(b[3] === 0, 'Nonzero reserved header byte');
  const v = view(b);
  const h: BinaryHeader = { kind: b[0], stream: b[1], flags: b[2], sequence: v.getUint32(4, true),
    ptsUs: v.getBigInt64(8, true), senderClockUs: v.getBigInt64(16, true) };
  validateHeader(h); return h;
}
export function encodeTouch(contacts: TouchContact[]): Uint8Array {
  check(contacts.length <= 10, 'Too many contacts');
  const b = new Uint8Array(1 + contacts.length * 6), v = view(b), seen = new Set<number>();
  b[0] = contacts.length;
  contacts.forEach((c, i) => {
    check(integer(c.id, 0, 9) && !seen.has(c.id), 'Invalid or duplicate contact id'); seen.add(c.id);
    check(typeof c.pressed === 'boolean', 'Invalid pressed value');
    check(Number.isFinite(c.x) && c.x >= 0 && c.x <= 1 && Number.isFinite(c.y) && c.y >= 0 && c.y <= 1, 'Invalid coordinates');
    const o = 1 + i * 6; b[o] = c.id; b[o + 1] = c.pressed ? 1 : 0;
    v.setUint16(o + 2, Math.round(c.x * 65535), true); v.setUint16(o + 4, Math.round(c.y * 65535), true);
  }); return b;
}
export function decodeTouch(b: Uint8Array): TouchContact[] {
  check(b.length > 0, 'Missing touch count');
  check(b[0] <= 10 && b.length === 1 + b[0] * 6, 'Invalid touch payload length or count');
  const v = view(b), seen = new Set<number>();
  return Array.from({ length: b[0] }, (_, i) => {
    const o = 1 + i * 6, id = b[o], pressed = b[o + 1];
    check(integer(id, 0, 9) && !seen.has(id), 'Invalid or duplicate contact id'); seen.add(id);
    check(pressed <= 1, 'Invalid pressed value');
    return { id, pressed: pressed === 1, x: v.getUint16(o + 2, true) / 65535, y: v.getUint16(o + 4, true) / 65535 };
  });
}
export function encodeKey(k: KeyPayload): Uint8Array {
  check(integer(k.code, 0, 10) && typeof k.down === 'boolean', 'Invalid key');
  return new Uint8Array([k.code, 0, k.down ? 1 : 0]);
}
export function decodeKey(b: Uint8Array): KeyPayload {
  check(b.length === 3, 'Key payload must be three bytes');
  const code = view(b).getUint16(0, true);
  check(code <= 10, 'Unknown key code'); check(b[2] <= 1, 'Invalid key down value');
  return { code, down: b[2] === 1 };
}
function validatePayload(kind: BinaryKind, b: Uint8Array): void {
  if (kind === BinaryKind.Touch) decodeTouch(b);
  if (kind === BinaryKind.Key) decodeKey(b);
}
export function encodeBinary(header: BinaryHeader, payload: Uint8Array): Uint8Array {
  validatePayload(header.kind, payload);
  const b = new Uint8Array(HEADER_SIZE + payload.length); b.set(encodeHeader(header)); b.set(payload, HEADER_SIZE); return b;
}
export function decodeBinary(b: Uint8Array): { header: BinaryHeader; payload: Uint8Array } {
  check(b.length >= HEADER_SIZE, 'Truncated message header');
  const header = decodeHeader(b.subarray(0, HEADER_SIZE)), payload = b.subarray(HEADER_SIZE);
  validatePayload(header.kind, payload); return { header, payload };
}
