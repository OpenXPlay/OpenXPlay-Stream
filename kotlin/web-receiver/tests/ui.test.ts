// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

import { expect, it } from 'vitest';
import { ReceiverPreferences, selectLanguage, statusCard, protocolStatus, strings, type StatusState } from '../receiver/ui';
function store() {
  const values = new Map<string, string>();
  return { getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => { values.set(key, value); }, removeItem: (key: string) => { values.delete(key); } };
}
it('chooses the first supported language and falls back to English', () => {
  for (const language of ['zh', 'zh-CN', 'zh-SG', 'zh-Hans', 'zh-Hans-CN']) expect(selectLanguage([language])).toBe('zh-CN');
  expect(selectLanguage(['fr', 'zh-CN', 'en'])).toBe('zh-CN');
  expect(selectLanguage(['en-GB', 'zh-CN'])).toBe('en');
  for (const languages of [[], ['fr'], ['zh-TW'], ['zh-Hant']]) expect(selectLanguage(languages)).toBe('en');
  expect(Object.keys(strings['zh-CN'])).toEqual(Object.keys(strings.en));
});
it('persists settings and developer mode, supports debug fragment and allows switching it off', () => {
  const storage = store(), preferences = new ReceiverPreferences(() => storage);
  const values = preferences.load(new URLSearchParams());
  expect(values).toEqual({ frameRate: 'AUTO', playback: 'SMART', sound: true, microphone: true, fit: 'fit', developer: false });
  preferences.save({ frameRate: 'AUTO', playback: 'SPEED', sound: false, microphone: false, fit: 'fill', developer: true });
  expect(preferences.load(new URLSearchParams())).toEqual({ frameRate: 'AUTO', playback: 'SPEED', sound: false, microphone: false, fit: 'fill', developer: true });
  values.developer = false; preferences.save(values);
  expect(preferences.load(new URLSearchParams()).developer).toBe(false);
  const debug = preferences.load(new URLSearchParams('debug')); expect(debug.developer).toBe(true);
  debug.developer = false; preferences.save(debug); expect(debug.developer).toBe(false);
});
it('tolerates localStorage access, read, write and clear failures', () => {
  for (const preferences of [new ReceiverPreferences(() => { throw Error('blocked'); }),
    new ReceiverPreferences(() => ({ getItem() { throw Error(); }, setItem() { throw Error(); }, removeItem() { throw Error(); } }))]) {
    const values = preferences.load(new URLSearchParams()); expect(values.fit).toBe('fit');
    expect(() => preferences.save(values)).not.toThrow();
  }
});
it('shows a useful status card for every non-playing state', () => {
  for (const state of ['connecting', 'waiting', 'reconnecting', 'error', 'unsupported'] as StatusState[]) {
    const card = statusCard(state); expect(card.hidden).toBe(false);
    for (const language of ['en', 'zh-CN'] as const) { expect(strings[language][card.headline]).toBeTruthy(); expect(strings[language][card.subline]).toBeTruthy(); }
  }
  expect(statusCard('playing').hidden).toBe(true);
});
it('keeps the required legal attribution visible in both languages', () => {
  for (const language of ['en', 'zh-CN'] as const) expect(strings[language].powered).toBe('Powered by OpenXPlay.');
});

it('maps protocol phases and errors without exposing technical text on the card', () => {
  expect(protocolStatus('ACTIVE', false)).toBe('waiting');
  expect(protocolStatus('ACTIVE', true)).toBe('playing');
  for (const phase of ['CONNECTING', 'ENDED']) expect(protocolStatus(phase, true)).toBe('waiting');
  for (const phase of ['RECONNECTING', 'Disconnected; reconnecting…']) expect(protocolStatus(phase, true)).toBe('reconnecting');
  expect(protocolStatus('Connecting…', false)).toBe('connecting');
  for (const phase of ['Protocol error: invalid JSON', 'Connection failed']) expect(protocolStatus(phase, true)).toBe('error');
});

it('offers browser guidance for unavailable or unsupported decoders', () => {
  for (const reason of ['Decoder unavailable: failed', 'Error: Unsupported codec avc1', 'WebCodecs is unavailable']) expect(protocolStatus(reason, false)).toBe('unsupported');
});
it('removes the obsolete gear preference without saving it again', () => {
  const storage = store(), preferences = new ReceiverPreferences(() => storage);
  storage.setItem('oxp.gear', '{"x":50,"y":50}');
  const loaded = preferences.load(new URLSearchParams());
  expect(storage.getItem('oxp.gear')).toBeNull();
  preferences.save(loaded); expect(storage.getItem('oxp.gear')).toBeNull();
});
