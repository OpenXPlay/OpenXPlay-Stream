// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-15

import markup from './receiver/index.html?raw';
import css from './receiver/style.css?raw';
import { strings, statusCard, microphoneStatus, applyReducedMotion, soundPrompt, type Language, type StatusState, type TextKey, type Preferences } from './receiver/ui';
import { bindDrawer } from './receiver/drawer';
import { bindQuickTiles, syncSettings, syncPlugins, syncFullscreen } from './receiver/drawer-settings';
import { frameRatePreference } from './receiver/frame-rate';
import { playbackMode } from './shared/playback';
import type { DrawerPage } from './receiver/drawer-core';
import { version } from './receiver/version';
import { previewOptions, previewStates } from './preview-options';
const $ = <T extends HTMLElement>(id: string) => document.getElementById(id)! as T;
const options = previewOptions(new URLSearchParams(location.search));
for (const state of previewStates) { const option = document.createElement('option'); option.value = state; option.textContent = state; $('state').append(option); }
const frames: HTMLIFrameElement[] = [];
$<HTMLSelectElement>('state').value = options.state;
$<HTMLSelectElement>('language').value = options.language;
$<HTMLSelectElement>('sheet').value = options.sheet;
for (const key of ['large', 'motion', 'fallback'] as const) $<HTMLInputElement>(key).checked = options[key];
function paint(frame: HTMLIFrameElement): void {
  const doc = frame.contentDocument!;
  const el = <T extends HTMLElement = HTMLElement>(id: string) => doc.getElementById(id)! as T;
  const language = $<HTMLSelectElement>('language').value as Language;
  const fixture = $<HTMLSelectElement>('state').value;
  const state = (fixture === 'playingSoundBlocked' ? 'playing' : fixture) as StatusState;
  const text = strings[language], card = statusCard(state);
  doc.documentElement.lang = language;
  const fontSize = parseFloat(frame.contentWindow!.getComputedStyle(doc.documentElement).fontSize);
  if ($<HTMLInputElement>('large').checked) doc.documentElement.style.fontSize = `${fontSize * 2}px`;
  doc.documentElement.classList.toggle('large-text', $<HTMLInputElement>('large').checked);
  applyReducedMotion(doc.documentElement, true);
  doc.querySelectorAll<HTMLElement>('[data-text]').forEach(node => { node.textContent = text[node.dataset.text as TextKey]; });
  doc.querySelectorAll<HTMLElement>('[data-label]').forEach(node => node.setAttribute('aria-label', text[node.dataset.label as TextKey]));
  for (const id of ['gear', 'card-settings']) el(id).setAttribute('aria-label', text.settings);
  el('close-settings').setAttribute('aria-label', text.close); el('drawer-back').setAttribute('aria-label', text.back);
  el('version').textContent = version; el('about-value').textContent = version;
  el('relay-version-row').hidden = false; el('relay-name').textContent = 'OpenXPlay Relay'; el('relay-version').textContent = '0.1.6';
  el('card').dataset.state = state; el('card').hidden = card.hidden;
  el('headline').textContent = text[card.headline];
  el('subline').textContent = state === 'reconnecting' ? text.attempt.replace('{attempt}', '3').replace('{seconds}', '5') : text[card.subline];
  el('retry').hidden = state !== 'error';
  const prompt = soundPrompt(state === 'playing', true, fixture === 'soundBlocked' || fixture === 'playingSoundBlocked' ? 'suspended' : 'running');
  el('sound-hint').hidden = prompt !== 'pill'; el('card-sound').hidden = prompt !== 'card';
  el('gear').hidden = !$<HTMLInputElement>('fallback').checked;
  el('picture-size').textContent = `${text.picture} 1920 × 720 · ${text.windowSize} ${frame.width} × ${frame.height}`;
  const mic = microphoneStatus(state === 'microphoneDenied' ? 'denied' : state === 'playing' ? 'capturing' : 'idle');
  el('mic-state').textContent = text[mic.label]; el('sound-value').textContent = text[mic.label]; el('mic-hint').textContent = text[mic.hint]; el('mic-retry').hidden = !mic.retry;
  el<HTMLInputElement>('sound').checked = true; el<HTMLInputElement>('fullscreen-restore').checked = true;
  el('display-settings').hidden = false;
  for (const [id, value] of Object.entries({ resolution: 'auto', 'screen-width': '200', 'relay-video-codec': 'auto', 'frame-rate': 'AUTO', playback: 'SMART' })) el<HTMLInputElement>(id).value = value;
  el('video-codec-current').hidden = false; el('video-codec-current').textContent = text.codecInUseH264;
  const dialog = el<HTMLDialogElement>('settings');
  const preferences: Preferences = { sound: true, developer: false, microphone: true, fit: 'fit', frameRate: 'AUTO', playback: 'SMART' };
  let fullscreenOn = false;
  const drawer = bindDrawer(dialog, () => doc.documentElement.classList.contains('reduced-motion') || frame.contentWindow!.matchMedia('(prefers-reduced-motion: reduce)').matches, () => {});
  for (const id of ['gear', 'card-settings']) el(id).onclick = drawer.open;
  const update = () => {
    preferences.sound = el<HTMLInputElement>('sound').checked; preferences.developer = el<HTMLInputElement>('developer').checked;
    preferences.frameRate = frameRatePreference(el<HTMLInputElement>('frame-rate').value); preferences.playback = playbackMode(el<HTMLInputElement>('playback').value);
    syncSettings(dialog, preferences, !el('display-settings').hidden, text);
    syncPlugins(dialog, el<HTMLInputElement>('plugins-enabled').checked, text);
    syncFullscreen(doc, true, fullscreenOn, false, text);
  };
  dialog.addEventListener('change', update);
  el('fullscreen').onclick = () => { fullscreenOn = !fullscreenOn; update(); };
  for (const fit of ['fit', 'fill'] as const) el(fit).onclick = () => { preferences.fit = fit; update(); };
  bindQuickTiles(dialog, () => preferences.fit); update();
  const selected = $<HTMLSelectElement>('sheet').value as DrawerPage | '';
  if (selected) {
    drawer.open();
    if (selected !== 'root') { drawer.push(['resolution', 'frame-rate', 'codec'].includes(selected) ? 'picture' : selected); if (['resolution', 'frame-rate', 'codec'].includes(selected)) drawer.push(selected); }
  }
  frame.contentWindow!.requestAnimationFrame(() => applyReducedMotion(doc.documentElement, $<HTMLInputElement>('motion').checked));
  el('retry').onclick = () => { $<HTMLSelectElement>('state').value = 'connecting'; render(); };
  const resume = () => { el('sound-hint').hidden = true; el('card-sound').hidden = true; };
  for (const id of ['sound-hint', 'card-sound']) el(id).onclick = resume;
}
function render(): void { for (const frame of frames) frame.srcdoc = markup.replace(/<script[\s\S]*?<\/script>/g, '').replace('</head>', `<style>${css}</style></head>`); }
for (const [width, height] of options.sizes) {
  const article = document.createElement('article'), heading = document.createElement('h2'), box = document.createElement('div'), frame = document.createElement('iframe');
  heading.textContent = `${width} × ${height}`; box.className = 'viewport'; frame.width = String(width); frame.height = String(height); frame.title = `Receiver ${width} × ${height}`;
  frame.onload = () => paint(frame); frames.push(frame); box.append(frame); article.append(heading, box); $('previews').append(article);
}
for (const id of ['language', 'state', 'sheet', 'motion', 'large', 'fallback']) $(id).onchange = render;
render();
