// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-30

import { parseMessage, type ReceiverClose } from '../protocol/json';
export interface CloseCheckpoint { report: ReceiverClose; live: boolean; at: number }
interface StoragePort { getItem(key: string): string | null; setItem(key: string, value: string): void; removeItem(key: string): void }
const key = 'oxp-receiver-close';
export function closeReason(reason: string): string {
  return ['', 'Input backpressure', 'Invalid OXP-Web message'].includes(reason) ? reason : '[redacted]';
}
export function saveClose(value: CloseCheckpoint, storage: () => StoragePort): void {
  try { storage().setItem(key, JSON.stringify(value)); } catch { /* Storage is optional. */ }
}
export function reloadClose(value: CloseCheckpoint, now = Date.now()): CloseCheckpoint {
  if (!value.live) return value;
  const elapsed = Math.max(0, now - value.at);
  return { report: { ...value.report, openForMs: value.report.openForMs + elapsed,
    sinceLastMessageMs: value.report.sinceLastMessageMs + elapsed }, live: false, at: now };
}
export function loadClose(storage: () => StoragePort): ReceiverClose | undefined {
  try {
    const port = storage(), raw = port.getItem(key); port.removeItem(key);
    if (!raw || raw.length > 2048) return;
    const value = JSON.parse(raw) as CloseCheckpoint;
    const report = parseMessage(value.report, 'client-to-relay');
    if (report.type !== 'receiver-close' || typeof value.live !== 'boolean' || !Number.isFinite(value.at)) return;
    // If pagehide never ran, retain the last observed timings rather than counting time off-page.
    return { ...report, reason: closeReason(report.reason) };
  } catch { return; }
}
/** Only the current socket may update history; a late close from a replaced socket is ignored. */
export class ConnectionHistory {
  private previous?: ReceiverClose;
  private current: object | null = null;
  private openedAt: number | null = null;
  private receivedAt = 0;
  private reconnects = -1;
  cause: ReceiverClose['cause'] = 'unknown';
  visible = true;
  online = true;
  constructor(private readonly now: () => number = () => performance.now()) {}
  restore(previous?: ReceiverClose): void {
    if (previous) { this.previous = previous; this.reconnects = previous.reconnects; }
  }
  begin(socket: object): void {
    this.current = socket; this.openedAt = null; this.receivedAt = this.now(); this.reconnects++; this.cause = 'unknown';
  }
  opened(socket: object): ReceiverClose | undefined {
    if (this.current !== socket) return;
    this.openedAt = this.now(); this.receivedAt = this.openedAt;
    const previous = this.previous; this.previous = undefined;
    return previous ? { ...previous, reconnects: this.reconnects } : undefined;
  }
  received(socket: object): void { if (socket === this.current) this.receivedAt = this.now(); }
  snapshot(): CloseCheckpoint | undefined {
    if (!this.current) return this.previous ? { report: this.previous, live: false, at: Date.now() } : undefined;
    return { report: this.report(1006, '', false, 'reload'), live: true, at: Date.now() };
  }
  private report(code: number, reason: string, wasClean: boolean, cause: ReceiverClose['cause']): ReceiverClose {
    const now = this.now();
    return { type: 'receiver-close', code, reason: closeReason(reason), wasClean,
      openForMs: this.openedAt === null ? 0 : Math.max(0, now - this.openedAt), sinceLastMessageMs: Math.max(0, now - this.receivedAt),
      cause, visibility: this.visible ? 'visible' : 'hidden', online: this.online, reconnects: Math.max(0, this.reconnects) };
  }
  closed(socket: object, event: { code?: number; reason?: string; wasClean?: boolean }, cause = this.cause): void {
    if (socket !== this.current) return;
    if (cause === 'unknown' || cause === 'decoder_error' && event.code !== undefined && event.code !== 1006) cause = !this.online ? 'offline' : !this.visible ? 'hidden' : event.code !== undefined && event.code !== 1006 ? 'server_close' : 'unknown';
    this.previous = this.report(event.code ?? 1006, event.reason ?? '', event.wasClean ?? false, cause);
    this.current = null;
  }
}
