// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-15

import markup from './receiver/index.html?raw';
import css from './receiver/style.css?raw';
import { gearOffset, strings, statusCard, microphoneStatus, applyReducedMotion, soundPrompt, type Language, type StatusState, type TextKey } from './receiver/ui';
import { bindSheet } from './receiver/sheet';
import { version } from './receiver/version';
const $ = <T extends HTMLElement>(id: string) => document.getElementById(id)! as T;
import { previewOptions, previewStates } from './preview-options';
const options = previewOptions(new URLSearchParams(location.search));
const states = previewStates;
for (const state of states) { const option = document.createElement('option'); option.value = state; option.textContent = state; $('state').append(option); }
const frames: HTMLIFrameElement[] = [];
$<HTMLSelectElement>('state').value = options.state;
$<HTMLSelectElement>('language').value = options.language;
for (const key of ['sheet', 'large', 'motion'] as const) $<HTMLInputElement>(key).checked = options[key];
function paint(frame: HTMLIFrameElement): void {
  const doc = frame.contentDocument!;
  const el = <T extends HTMLElement = HTMLElement>(id: string) => doc.getElementById(id)! as T;
  const language = $<HTMLSelectElement>('language').value as Language;
  const fixture = $<HTMLSelectElement>('state').value;
  const playing = fixture === 'playing' || fixture === 'playingSoundBlocked';
  const state = (fixture === 'playingSoundBlocked' ? 'playing' : fixture) as StatusState;
  const text = strings[language], card = statusCard(state);
  doc.documentElement.lang = language;
  doc.documentElement.style.removeProperty('font-size');
  const fontSize = parseFloat(frame.contentWindow!.getComputedStyle(doc.documentElement).fontSize);
  if ($<HTMLInputElement>('large').checked) doc.documentElement.style.fontSize = `${fontSize * 2}px`;
  doc.documentElement.classList.toggle('large-text', $<HTMLInputElement>('large').checked);
  applyReducedMotion(doc.documentElement, $<HTMLInputElement>('motion').checked);
  doc.querySelectorAll<HTMLElement>('[data-text]').forEach(node => { node.textContent = text[node.dataset.text as TextKey]; });
  el('gear').setAttribute('aria-label', text.settings); el('close-settings').setAttribute('aria-label', text.close);
  el('version').textContent = version; el('card').dataset.state = state; el('card').hidden = card.hidden;
  el('headline').textContent = text[card.headline];
  el('subline').textContent = state === 'reconnecting' ? text.attempt.replace('{attempt}', '3').replace('{seconds}', '5') : text[card.subline];
  el('retry').hidden = !['error', 'microphoneDenied'].includes(state);
  const prompt = soundPrompt(playing, true, fixture === 'soundBlocked' || fixture === 'playingSoundBlocked' ? 'suspended' : 'running');
  el('sound-hint').hidden = prompt !== 'pill'; el('card-sound').hidden = prompt !== 'card';
  el('controls').classList.toggle('playing', playing);
  {
    const controls = el('controls'), style = frame.contentWindow!.getComputedStyle(controls);
    const bounds = { width: frame.contentWindow!.innerWidth, height: frame.contentWindow!.innerHeight, size: el('gear').offsetHeight,
      top: parseFloat(style.marginTop), right: parseFloat(style.marginRight), bottom: parseFloat(style.marginBottom), left: parseFloat(style.marginLeft) };
    const offset = gearOffset(options.gear, bounds); controls.dataset.side = offset.x < bounds.width / 2 ? 'left' : 'right';
    controls.style.transform = `translate(${offset.x - bounds.left}px, ${offset.y - bounds.top}px)`;
  }
  el('fullscreen-state').textContent = text.off;
  const mic = microphoneStatus(state === 'microphoneDenied' ? 'denied' : state === 'playing' ? 'capturing' : 'idle');
  el('mic-state').textContent = text[mic.label]; el('mic-hint').textContent = text[mic.hint]; el('mic-retry').hidden = !mic.retry;
  el<HTMLInputElement>('sound').checked = true;
  const dialog = el<HTMLDialogElement>('settings'); if (dialog.open) dialog.close();
  const sheet = bindSheet(dialog, el('gear'), el('close-settings'), el('sheet-backdrop'), () => doc.documentElement.classList.contains('reduced-motion'));
  el('gear').onclick = sheet.open;
  if ($<HTMLInputElement>('sheet').checked) sheet.open();
  el('retry').onclick = () => { $<HTMLSelectElement>('state').value = 'connecting'; render(); };
  const resume = () => { el('sound-hint').hidden = true; el('card-sound').hidden = true; };
  for (const type of ['pointerup', 'touchend', 'click', 'keydown']) doc.addEventListener(type, resume, { once: true, passive: true });
  for (const fit of ['fit', 'fill']) el(fit).onclick = () => { el('fit').setAttribute('aria-pressed', String(fit === 'fit')); el('fill').setAttribute('aria-pressed', String(fit === 'fill')); };
}
function render(): void { for (const frame of frames) { frame.srcdoc = markup.replace(/<script[\s\S]*?<\/script>/g, '').replace('</head>', `<style>${css}</style></head>`); } }
for (const [width, height] of options.sizes) {
  const article = document.createElement('article'), heading = document.createElement('h2'), box = document.createElement('div'), frame = document.createElement('iframe');
  heading.textContent = `${width} × ${height}`; box.className = 'viewport'; frame.width = String(width); frame.height = String(height); frame.title = `Receiver ${width} × ${height}`;
  frame.onload = () => paint(frame); frames.push(frame); box.append(frame); article.append(heading, box); $('previews').append(article);
}
for (const id of ['language', 'state', 'sheet', 'motion', 'large']) $(id).onchange = render;
render();
