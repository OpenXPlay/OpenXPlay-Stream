// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

// A missing or stalled browser API must never hold up playback or the report.
export async function bestEffort<T>(probe: () => T | PromiseLike<T>, timeoutMs = 1500): Promise<T | undefined> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([Promise.resolve().then(probe), new Promise<undefined>(resolve => { timer = setTimeout(resolve, timeoutMs); })]);
  } catch { return undefined; } finally { clearTimeout(timer); }
}
