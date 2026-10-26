// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-25

import { syncChoices } from './drawer';
import { playbackHint, type Preferences, type strings } from './ui';

type Text = typeof strings.en;
const element = <T extends HTMLElement = HTMLElement>(root: ParentNode, id: string) => root.querySelector<T>(`#${id}`)!;
export function bindQuickTiles(root: ParentNode, fit: () => Preferences['fit']): void {
  for (const id of ['sound', 'fullscreen']) element(root, `quick-${id}`).onclick = () => element(root, id).click();
  element(root, 'quick-fill').onclick = () => element(root, fit() === 'fill' ? 'fit' : 'fill').click();
}
export function syncPictureValue(root: ParentNode, displaySettings: boolean): void {
  element(root, 'picture-value').textContent = element(root, displaySettings ? 'resolution-value' : 'frame-rate-value').textContent;
}
export function syncPlugins(root: ParentNode, enabled: boolean, text: Text): void {
  element(root, 'plugins-value').textContent = text[enabled ? 'enabled' : 'off'];
  element(root, 'reset-plugin-positions').hidden = !enabled;
}
export function syncSettings(root: HTMLElement, preferences: Preferences, displaySettings: boolean, text: Text): void {
  for (const key of ['sound', 'developer'] as const) element<HTMLInputElement>(root, key).checked = preferences[key];
  for (const key of ['frameRate', 'playback'] as const) element<HTMLInputElement>(root, key === 'frameRate' ? 'frame-rate' : key).value = preferences[key];
  element(root, 'playback-hint').textContent = text[playbackHint(preferences.playback)];
  for (const id of ['fit', 'fill', 'quick-fill']) element(root, id).setAttribute('aria-pressed', String(id === 'fit' ? preferences.fit === 'fit' : preferences.fit === 'fill'));
  element(root, 'quick-sound').setAttribute('aria-pressed', String(preferences.sound));
  element(root, 'fit-segments').style.setProperty('--index', preferences.fit === 'fill' ? '1' : '0');
  syncChoices(root); syncPictureValue(root, displaySettings);
}
export function syncFullscreen(root: ParentNode, available: boolean, intent: boolean, pending: boolean, text: Text): void {
  root.querySelectorAll<SVGElement>('[data-fullscreen]').forEach(icon => { icon.toggleAttribute('hidden', (icon.dataset.fullscreen === 'on') !== intent); });
  for (const id of ['fullscreen', 'quick-fullscreen']) {
    element<HTMLButtonElement>(root, id).disabled = !available;
    element(root, id).setAttribute('aria-pressed', String(intent));
  }
  element(root, 'fullscreen-hint').hidden = !pending;
  element(root, 'fullscreen-state').textContent = available ? '' : text.fullscreenUnavailable;
}
