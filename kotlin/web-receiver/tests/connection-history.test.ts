// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-30

import { expect, it } from 'vitest';
import { ConnectionHistory, loadClose, reloadClose, saveClose } from '../receiver/connection-history';
import { parseMessage } from '../protocol/json';

it('reports only the previous socket, once, with timing and bounded private text', () => {
  let now = 10;
  const history = new ConnectionHistory(() => now), first = {}, second = {};
  history.begin(first); expect(history.opened(first)).toBeUndefined();
  now = 110; history.received(first); now = 160;
  history.closed(first, { code: 1001, reason: 'private peer text\n'.repeat(100), wasClean: true });
  history.begin(second); now = 200;
  const report = history.opened(second)!;
  expect(report).toMatchObject({ type: 'receiver-close', code: 1001, reason: '[redacted]', wasClean: true,
    openForMs: 150, sinceLastMessageMs: 50, cause: 'server_close', reconnects: 1, visibility: 'visible', online: true });
  expect(parseMessage(report, 'client-to-relay')).toEqual(report);
  expect(history.opened(second)).toBeUndefined();
  history.closed(first, { code: 1000 });
  expect(history.snapshot()?.live).toBe(true);
});
it('keeps the current local cause and handles sockets that never opened', () => {
  const history = new ConnectionHistory(() => 20), first = {}, second = {};
  history.begin(first); history.online = false; history.closed(first, { code: 1006 });
  history.begin(second);
  expect(history.opened(second)).toMatchObject({ openForMs: 0, cause: 'offline', wasClean: false });
  history.cause = 'decoder_error'; history.visible = false;
  history.closed(second, { code: 1006 }); history.begin({});
  expect(history.snapshot()?.report.cause).toBe('reload');
});
it('saves a reload checkpoint and reads it once; unavailable or corrupt storage is harmless', () => {
  let raw: string | null = null;
  const port = { getItem: () => raw, setItem: (_: string, value: string) => { raw = value; }, removeItem: () => { raw = null; } };
  const history = new ConnectionHistory(() => 10), first = {};
  history.begin(first); history.opened(first);
  const checkpoint = history.snapshot()!; checkpoint.at = 100;
  saveClose(reloadClose(checkpoint, 150), () => port);
  expect(loadClose(() => port)).toMatchObject({ cause: 'reload', code: 1006, openForMs: 50, sinceLastMessageMs: 50 });
  expect(loadClose(() => port)).toBeUndefined();
  for (const value of ['bad json', '{}', 'x'.repeat(3000)]) { raw = value; expect(loadClose(() => port)).toBeUndefined(); }
  const unavailable = () => { throw new Error('storage blocked'); };
  expect(() => saveClose(checkpoint, unavailable)).not.toThrow();
  expect(loadClose(unavailable)).toBeUndefined();
});
it('rejects wrong direction, unbounded, multiline and invalid receiver-close fields', () => {
  const history = new ConnectionHistory(), socket = {};
  history.begin(socket); history.closed(socket, { code: 1006 });
  const report = history.snapshot()!.report;
  expect(() => parseMessage(report, 'relay-to-client')).toThrow();
  for (const fields of [{ reason: '字'.repeat(42) }, { reason: 'line\nbreak' }, { code: 65536 }, { wasClean: 1 },
    { openForMs: -1 }, { sinceLastMessageMs: NaN }, { cause: 'bad' }, { reconnects: 1.5 }, { online: 'yes' }, { visibility: 'bad' }]) {
    expect(() => parseMessage({ ...report, ...fields }, 'client-to-relay')).toThrow();
  }
});
