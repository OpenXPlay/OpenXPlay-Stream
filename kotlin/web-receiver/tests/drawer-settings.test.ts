// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-25

import { expect, it, vi } from 'vitest';
import { bindQuickTiles, syncSettings, syncPlugins, syncFullscreen } from '../receiver/drawer-settings';
import { strings, type Preferences } from '../receiver/ui';

class Control {
  value = ''; checked = false; disabled = false; hidden = false; textContent = '';
  dataset: Record<string, string> = {};
  attributes = new Map<string, string>(); properties = new Map<string, string>();
  style = { setProperty: (key: string, value: string) => this.properties.set(key, value) };
  onclick?: () => void;
  setAttribute(key: string, value: string) { this.attributes.set(key, value); }
  toggleAttribute(key: string, on: boolean) { if (on) this.attributes.set(key, ''); else this.attributes.delete(key); }
  click() { if (!this.disabled) this.onclick?.(); }
}
function fixture() {
  const ids = ['sound', 'developer', 'fit', 'fill', 'quick-fill', 'quick-sound', 'quick-fullscreen', 'fullscreen', 'fit-segments', 'playback-hint', 'picture-value', 'plugins-value', 'reset-plugin-positions', 'fullscreen-hint', 'fullscreen-state', 'native-hint'];
  const nodes = new Map(ids.map(id => [id, new Control()])), inputs: Control[] = [], choices: Control[] = [];
  for (const [id, values] of Object.entries({ resolution: ['auto', 'native', '720p'], 'frame-rate': ['AUTO', 'FPS_30', 'FPS_60'], playback: ['SMART', 'QUALITY', 'SPEED'] })) {
    const input = Object.assign(new Control(), { id, value: values[0] }); nodes.set(id, input); inputs.push(input);
    nodes.set(`${id}-value`, new Control()); nodes.set(`${id}-segments`, new Control());
    for (const value of values) choices.push(Object.assign(new Control(), { dataset: { choice: id, value }, textContent: value }));
  }
  const icons = ['on', 'off'].map(fullscreen => Object.assign(new Control(), { dataset: { fullscreen } }));
  const root = {
    querySelector: (selector: string) => nodes.get(selector.slice(1)) ?? null,
    querySelectorAll: (selector: string) => selector === 'input[type=hidden]' ? inputs : selector === '[data-fullscreen]' ? icons : choices.filter(button => selector === `[data-choice="${button.dataset.choice}"]`),
  } as unknown as HTMLElement;
  const preferences: Preferences = { sound: true, developer: false, microphone: true, fit: 'fit', frameRate: 'AUTO', playback: 'SMART' };
  const el = (id: string) => nodes.get(id)!;
  return { root, el, choices, icons, preferences };
}
it('syncs fit segments, sound tiles, choices and the display-dependent picture summary', () => {
  const { root, el, choices, preferences } = fixture();
  Object.assign(preferences, { sound: false, developer: true, fit: 'fill', frameRate: 'FPS_60' }); el('resolution').value = 'native';
  syncSettings(root, preferences, true, strings.en);
  expect(el('sound').checked).toBe(false); expect(el('developer').checked).toBe(true);
  for (const id of ['fill', 'quick-fill']) expect(el(id).attributes.get('aria-pressed')).toBe('true');
  for (const id of ['fit', 'quick-sound']) expect(el(id).attributes.get('aria-pressed')).toBe('false');
  expect(el('fit-segments').properties.get('--index')).toBe('1'); expect(el('picture-value').textContent).toBe('native');
  expect(el('native-hint').hidden).toBe(false);
  preferences.fit = 'fit'; preferences.sound = true; el('frame-rate').disabled = true;
  syncSettings(root, preferences, false, strings.en);
  expect(el('fit-segments').properties.get('--index')).toBe('0'); expect(el('picture-value').textContent).toBe('FPS_60');
  expect(el('quick-sound').attributes.get('aria-pressed')).toBe('true');
  const rates = choices.filter(button => button.dataset.choice === 'frame-rate');
  expect(rates.every(button => button.disabled)).toBe(true);
  expect(rates.filter(button => button.attributes.get('aria-pressed') === 'true').map(button => button.dataset.value)).toEqual(['FPS_60']);
});
it.each(['en', 'zh-CN'] as const)('shares playback hints and plugin reset visibility in %s', language => {
  const { root, el, preferences } = fixture(), text = strings[language];
  for (const [mode, hint] of [['SMART', 'smartHint'], ['QUALITY', 'qualityHint'], ['SPEED', 'speedHint']] as const) {
    preferences.playback = mode; syncSettings(root, preferences, true, text);
    expect(el('playback-hint').textContent).toBe(text[hint]);
    expect(el('playback-segments').properties.get('--index')).toBe(String(['SMART', 'QUALITY', 'SPEED'].indexOf(mode)));
  }
  for (const enabled of [false, true, false]) {
    syncPlugins(root, enabled, text); expect(el('plugins-value').textContent).toBe(text[enabled ? 'enabled' : 'off']);
    expect(el('reset-plugin-positions').hidden).toBe(!enabled);
  }
});
it.each(['en', 'zh-CN'] as const)('shows fullscreen intent, pending hint and unavailable text consistently in %s', language => {
  const { root, el, icons } = fixture(), text = strings[language];
  for (const intent of [false, true]) {
    syncFullscreen(root, true, intent, intent, text);
    for (const id of ['fullscreen', 'quick-fullscreen']) {
      expect(el(id).disabled).toBe(false); expect(el(id).attributes.get('aria-pressed')).toBe(String(intent));
    }
    expect(el('fullscreen-state').textContent).toBe(''); expect(el('fullscreen-hint').hidden).toBe(!intent);
    for (const icon of icons) expect(icon.attributes.has('hidden')).toBe((icon.dataset.fullscreen === 'on') !== intent);
  }
  syncFullscreen(root, false, false, false, text);
  expect(el('fullscreen').disabled).toBe(true); expect(el('quick-fullscreen').disabled).toBe(true);
  expect(el('fullscreen-state').textContent).toBe(text.fullscreenUnavailable);
});
it('routes quick tiles through the full controls and uses the current fit preference on each tap', () => {
  const { root, el, preferences } = fixture();
  const sound = vi.fn(), fullscreen = vi.fn(); el('sound').onclick = sound; el('fullscreen').onclick = fullscreen;
  for (const fit of ['fit', 'fill'] as const) el(fit).onclick = () => { preferences.fit = fit; syncSettings(root, preferences, true, strings.en); };
  bindQuickTiles(root, () => preferences.fit);
  el('quick-sound').click(); expect(sound).toHaveBeenCalledOnce();
  el('quick-fullscreen').click(); expect(fullscreen).toHaveBeenCalledOnce();
  el('quick-fill').click(); expect(preferences.fit).toBe('fill'); expect(el('fit-segments').properties.get('--index')).toBe('1');
  el('quick-fill').click(); expect(preferences.fit).toBe('fit'); expect(el('quick-fill').attributes.get('aria-pressed')).toBe('false');
  syncFullscreen(root, false, false, false, strings.en); el('quick-fullscreen').click(); expect(fullscreen).toHaveBeenCalledOnce();
});
