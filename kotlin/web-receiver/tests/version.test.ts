// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-18

import { describe, expect, it } from 'vitest';
import { build, shouldReload } from '../receiver/version';

describe('receiver build reload', () => {
  const session = () => {
    const values = new Map<string, string>();
    return { getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => { values.set(key, value); } };
  };
  it('keeps the page for an absent or matching build without accessing storage', () => {
    const unavailable = () => { throw new Error('Storage unavailable'); };
    expect(shouldReload(undefined, unavailable)).toBe(false);
    expect(shouldReload('', unavailable)).toBe(false);
    expect(shouldReload(build, unavailable)).toBe(false);
  });
  it('records a mismatch before reloading and guards reconnects and a stale page after reload', () => {
    const storage = session();
    expect(shouldReload('server-a', () => storage, 'old-page')).toBe(true);
    expect(shouldReload('server-a', () => storage, 'old-page')).toBe(false);
    // A new page instance uses the same sessionStorage even if cached assets stayed stale.
    const afterReload = () => storage;
    expect(shouldReload('server-a', afterReload, 'old-page')).toBe(false);
    expect(shouldReload('server-b', afterReload, 'old-page')).toBe(true);
    expect(shouldReload('server-a', afterReload, 'old-page')).toBe(false);
    expect(shouldReload('server-b', afterReload, 'server-b')).toBe(false);
  });
  it('allows another tab to reload independently', () => {
    expect(shouldReload('server', () => session(), 'old-page')).toBe(true);
    expect(shouldReload('server', () => session(), 'old-page')).toBe(true);
  });
  it('does not reload when storage access, reading, writing or persistence fails', () => {
    const fail = () => { throw new Error('Storage unavailable'); };
    for (const storage of [fail, () => ({ getItem: fail, setItem: () => {} }),
      () => ({ getItem: () => null, setItem: fail }), () => ({ getItem: () => null, setItem: () => {} })])
      expect(shouldReload('server', storage, 'old-page')).toBe(false);
    let reads = 0;
    expect(shouldReload('server', () => ({ getItem: () => { if (reads++) fail(); return null; }, setItem: () => {} }), 'old-page')).toBe(false);
  });
});
