// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

import { expect, it } from 'vitest';
import { Pairing, ReceiverPreferences, selectLanguage, statusCard, protocolStatus, strings, type StatusState } from '../receiver/ui';
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
it('saves only after welcome, preserves leading zeros and clears rejected codes', () => {
  const preferences = new ReceiverPreferences(() => storeForOrigin);
  const storeForOrigin = store();
  const pairing = new Pairing(preferences, new URLSearchParams());
  expect(pairing.token).toBeNull();
  for (const code of ['', '1234567', '123456789', 'abcd5678']) expect(pairing.submit(code)).toBe(false);
  expect(pairing.submit('00123456')).toBe(true);
  expect(new Pairing(preferences, new URLSearchParams()).token).toBeNull();
  pairing.welcome();
  expect(new Pairing(preferences, new URLSearchParams()).token).toBe('00123456');
  expect(new Pairing(new ReceiverPreferences(() => store()), new URLSearchParams()).token).toBeNull();
  pairing.rejected(); expect(pairing.token).toBeNull();
  expect(new Pairing(preferences, new URLSearchParams()).token).toBeNull();
});
it('keeps legacy fragment tokens and lets a new code replace a rejected fragment', () => {
  const preferences = new ReceiverPreferences(() => storage); const storage = store();
  preferences.write('code', '12345678');
  const pairing = new Pairing(preferences, new URLSearchParams('k=legacy-token'));
  expect(pairing.token).toBe('legacy-token'); pairing.welcome(); pairing.rejected();
  pairing.submit('87654321'); pairing.welcome();
  expect(new Pairing(preferences, new URLSearchParams()).token).toBe('87654321');
});
it('persists settings and developer mode, supports debug fragment and allows switching it off', () => {
  const storage = store(), preferences = new ReceiverPreferences(() => storage);
  const values = preferences.load(new URLSearchParams());
  expect(values).toEqual({ videoCodec: 'AUTO', frameRate: 'AUTO', playback: 'SMART', sound: true, microphone: true, fit: 'fit', developer: false });
  preferences.save({ videoCodec: 'AUTO', frameRate: 'AUTO', playback: 'SPEED', sound: false, microphone: false, fit: 'fill', developer: true });
  expect(preferences.load(new URLSearchParams())).toEqual({ videoCodec: 'AUTO', frameRate: 'AUTO', playback: 'SPEED', sound: false, microphone: false, fit: 'fill', developer: true });
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
    const pairing = new Pairing(preferences, new URLSearchParams()); pairing.submit('12345678');
    expect(() => pairing.welcome()).not.toThrow(); expect(() => pairing.rejected()).not.toThrow();
  }
});
it('shows a useful status card for every non-playing state', () => {
  for (const state of ['code', 'incorrect', 'connecting', 'waiting', 'reconnecting', 'error', 'unsupported'] as StatusState[]) {
    const card = statusCard(state); expect(card.hidden).toBe(false);
    for (const language of ['en', 'zh-CN'] as const) { expect(strings[language][card.headline]).toBeTruthy(); expect(strings[language][card.subline]).toBeTruthy(); }
  }
  expect(statusCard('playing').hidden).toBe(true);
  expect(statusCard('incorrect')).toMatchObject({ headline: 'incorrect', subline: 'codeHint' });
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

it('distinguishes empty and coded hellos independently of the current input', () => {
  const preferences = new ReceiverPreferences(() => storage), storage = store();
  const pairing = new Pairing(preferences, new URLSearchParams());
  expect(pairing.token ?? '').toBe('');
  pairing.submit('12345678');
  expect(pairing.rejected('')).toBe('code');
  expect(pairing.rejected('12345678')).toBe('incorrect');
});
it('never stores an empty welcome even after a code has been typed', () => {
  const storage = store(), preferences = new ReceiverPreferences(() => storage);
  const pairing = new Pairing(preferences, new URLSearchParams());
  pairing.welcome(''); expect(preferences.read('code')).toBeNull();
  pairing.submit('12345678'); pairing.welcome(''); expect(preferences.read('code')).toBeNull();
  pairing.welcome('00123456'); expect(preferences.read('code')).toBe('00123456');
});
it('discards the former floating button position during preference loading', () => {
  const storage = store(), preferences = new ReceiverPreferences(() => storage);
  preferences.write('gear', '{"x":77,"y":24}'); preferences.load(new URLSearchParams());
  expect(preferences.read('gear')).toBeNull();
});

it('shows all startup stages in both languages independently of Rust pairing', () => {
  const stages = { connecting: 'connectingIphone', authenticating: 'authenticating', 'joining-network': 'joiningNetwork', 'waiting-media': 'waitingMedia', failed: 'phoneFailed', disconnected: 'phoneFailed' } as const;
  for (const [reason, state] of Object.entries(stages)) {
    expect(protocolStatus(`startup/${reason}`, true)).toBe(state);
    const card = statusCard(state);
    for (const language of ['en', 'zh-CN'] as const) {
      expect(strings[language][card.headline]).toBeTruthy(); expect(strings[language][card.subline]).toBeTruthy();
    }
  }
  expect(statusCard('code').headline).toBe('code'); expect(statusCard('incorrect').subline).toBe('codeHint');
});
