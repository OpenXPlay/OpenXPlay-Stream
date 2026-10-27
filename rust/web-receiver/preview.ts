// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-15

import markup from './receiver/index.html?raw';
import css from './receiver/style.css?raw';
import { strings, statusCard, microphoneStatus, applyReducedMotion, soundPrompt, type Language, type StatusState, type TextKey } from './receiver/ui';
import { bindDrawer, syncChoices } from './receiver/drawer';
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
  doc.querySelectorAll<HTMLElement>('[data-label]').forEach(node => node.setAttribute('aria-label', text[node.dataset.label as TextKey]));
  doc.querySelectorAll<HTMLElement>('[data-text]').forEach(node => { node.textContent = text[node.dataset.text as TextKey]; });
  el('gear').setAttribute('aria-label', text.settings); el('close-settings').setAttribute('aria-label', text.close);
  el('version').textContent = version; el('card').dataset.state = state; el('card').hidden = card.hidden;
  el('headline').textContent = text[card.headline];
  el('subline').textContent = state === 'reconnecting' ? text.attempt.replace('{attempt}', '3').replace('{seconds}', '5') : text[card.subline];
  el('pairing').hidden = state !== 'code' && state !== 'incorrect'; el('retry').hidden = !['error', 'microphoneDenied'].includes(state);
  const prompt = soundPrompt(playing, true, fixture === 'soundBlocked' || fixture === 'playingSoundBlocked' ? 'suspended' : 'running');
  el('sound-hint').hidden = prompt !== 'pill'; el('card-sound').hidden = prompt !== 'card';
  el('controls').classList.toggle('playing', playing);
  el('fullscreen-state').textContent = text.off;
  const mic = microphoneStatus(state === 'microphoneDenied' ? 'denied' : state === 'playing' ? 'capturing' : 'idle');
  el('mic-state').textContent = text[mic.label]; el('mic-hint').textContent = text[mic.hint]; el('mic-retry').hidden = !mic.retry;
  el<HTMLInputElement>('sound').checked = true;
  el('quick-sound').setAttribute('aria-pressed', 'true'); el('fit').setAttribute('aria-pressed', 'true');
  el('quick-fullscreen').setAttribute('aria-pressed', 'false');
  const dialog = el<HTMLDialogElement>('settings'); if (dialog.open) dialog.close();
  const drawer = bindDrawer(dialog, () => doc.documentElement.classList.contains('reduced-motion'), () => {});
  el('gear').onclick = drawer.open; el('card-settings').onclick = drawer.open;
  el('quick-sound').onclick = () => { el<HTMLInputElement>('sound').click(); el('quick-sound').setAttribute('aria-pressed', String(el<HTMLInputElement>('sound').checked)); };
  el('quick-fill').onclick = () => el(el('fill').getAttribute('aria-pressed') === 'true' ? 'fit' : 'fill').click();
  el('about-value').textContent = version;
  el<HTMLInputElement>('frame-rate').value = 'AUTO'; el<HTMLInputElement>('playback').value = 'SMART';
  el<HTMLInputElement>('resolution').value = 'auto'; el<HTMLInputElement>('screen-width').value = '200'; el<HTMLInputElement>('relay-video-codec').value = 'auto';
  el('display-settings').hidden = false; el('plugins-value').textContent = text.off; el('sound-value').textContent = text[mic.label];
  el('playback-hint').textContent = text.smartHint;
  syncChoices(dialog); el('picture-value').textContent = el('resolution-value').textContent;
  if ($<HTMLInputElement>('sheet').checked) {
    drawer.open();
    if (options.page !== 'root') setTimeout(() => {
      drawer.push(['resolution', 'codec', 'frame-rate'].includes(options.page) ? 'picture' : options.page);
      if (['resolution', 'codec', 'frame-rate'].includes(options.page)) setTimeout(() => drawer.push(options.page), 500);
    }, 600);
  }
  el('retry').onclick = () => { $<HTMLSelectElement>('state').value = 'connecting'; render(); };
  const resume = () => { el('sound-hint').hidden = true; el('card-sound').hidden = true; };
  for (const type of ['pointerup', 'touchend', 'click', 'keydown']) doc.addEventListener(type, resume, { once: true, passive: true });
  for (const fit of ['fit', 'fill']) el(fit).onclick = () => { el('fit').setAttribute('aria-pressed', String(fit === 'fit')); el('fill').setAttribute('aria-pressed', String(fit === 'fill')); el('quick-fill').setAttribute('aria-pressed', String(fit === 'fill')); el('fit-segments').style.setProperty('--index', fit === 'fill' ? '1' : '0'); };
  const input = el<HTMLInputElement>('code');
  input.oninput = () => { input.value = input.value.replace(/\D/g, '').slice(0, 8); [...el('digits').children].forEach((digit, i) => { digit.textContent = input.value[i] ?? ''; digit.classList.toggle('next', i === Math.min(7, input.value.length)); }); };
  el<HTMLFormElement>('pairing').onsubmit = event => { event.preventDefault(); el('paired').hidden = false; };
}
function render(): void { for (const frame of frames) { frame.srcdoc = markup.replace(/<script[\s\S]*?<\/script>/g, '').replace('</head>', `<style>${css}</style></head>`); } }
for (const [width, height] of options.sizes) {
  const article = document.createElement('article'), heading = document.createElement('h2'), box = document.createElement('div'), frame = document.createElement('iframe');
  heading.textContent = `${width} × ${height}`; box.className = 'viewport'; frame.width = String(width); frame.height = String(height); frame.title = `Receiver ${width} × ${height}`;
  frame.onload = () => paint(frame); frames.push(frame); box.append(frame); article.append(heading, box); $('previews').append(article);
}
for (const id of ['language', 'state', 'sheet', 'motion', 'large']) $(id).onchange = render;
render();
