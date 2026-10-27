// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

import { collectReceiverInfo, codecPowerEfficient } from '../shared/receiver-info';
import { codecCandidates, probeDecoders, estimateRefreshHz, audioCapabilities } from '../shared/capabilities';
import { mapTouch, renderQueueDepth, videoRect, canvasTouch } from '../shared/math';
import type { Hello, DisplayInfo, DisplaySettings, ViewArea } from '../protocol/json';
import type { TouchContact } from '../protocol/binary';
import type { PageCommand, WorkerNotice } from './messages';
import workletUrl from './audio-worklet.ts?worker&url';
import type { MicFormat } from '../protocol/json';
import { Pairing, ReceiverPreferences, playbackHint, selectLanguage, strings, statusCard, soundPrompt, type StatusState, type TextKey } from './ui';
import { bindDrawer, syncChoices } from './drawer';
import { fallbackSettings, settingsAnchor } from './drawer-core';
import { PluginSlot } from './plugin-slot';
import { microphoneStatus, applyReducedMotion } from './ui';
import { CaptureLifecycle } from './capture-lifecycle';
import { playbackUi, type PlaybackUi, type PlaybackEvent } from './video-hold';
import { version, shouldReload } from './version';
import { StableScreen, DisplayDebounce, DisplaySettingsGate } from './display';
import { FullscreenIntent, FullscreenExitHistory, autoRestoreFullscreen } from './fullscreen';
import { frameRatePreference, RestartGate, FrameRateSync } from './frame-rate';
import { playbackMode, withPlayback } from '../shared/playback';
import './style.css';
const canvas = document.querySelector<HTMLCanvasElement>('#video')!;
const element = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id)! as T;
const source = element<HTMLAnchorElement>('source');
const debug = element('debug');
const fragment = new URLSearchParams(location.hash.slice(1));
const storage = new ReceiverPreferences(() => localStorage);
const stableScreen = new StableScreen(() => localStorage);
stableScreen.measure(screen.width, screen.height, devicePixelRatio || 1);
const preferences = storage.load(fragment);
// Capture is controlled by CarPlay requests rather than a saved UI switch.
preferences.microphone = true;
const pairing = new Pairing(storage, fragment);
const language = selectLanguage(navigator.languages);
const text = strings[language];
document.documentElement.lang = language;
for (const node of document.querySelectorAll<HTMLElement>('[data-text]')) node.textContent = text[node.dataset.text as TextKey];
for (const node of document.querySelectorAll<HTMLElement>('[data-label]')) node.setAttribute('aria-label', text[node.dataset.label as TextKey]);
element('card-settings').setAttribute('aria-label', text.settings);
canvas.setAttribute('aria-label', text.projection);
const gear = element<HTMLButtonElement>('gear'); gear.setAttribute('aria-label', text.settings);
const settings = element<HTMLDialogElement>('settings');
let viewArea: ViewArea | undefined;
let fallbackGear = true;
const restartGate = new RestartGate(), frameRateSync = new FrameRateSync(preferences.frameRate, restartGate);
const displayGate = new DisplaySettingsGate(), displayDebounce = new DisplayDebounce();
let currentDisplaySettings: DisplaySettings = { type: 'display-settings', resolution: 'auto', screenWidth: 200, videoCodec: 'auto' };
function updateDisplaySettings(): void {
  element('display-settings').hidden = !displayGate.supported;
  element<HTMLInputElement>('resolution').value = currentDisplaySettings.resolution;
  element<HTMLInputElement>('screen-width').value = String(currentDisplaySettings.screenWidth);
  element<HTMLInputElement>('relay-video-codec').value = currentDisplaySettings.videoCodec;
  syncChoices(settings); updatePictureValue();
}
let uiState: StatusState = 'connecting';
const pluginSlot = new PluginSlot(() => {
  const box = canvas.getBoundingClientRect(), g = gear.getBoundingClientRect();
  const style = getComputedStyle(element('controls')), gap = parseFloat(getComputedStyle(document.documentElement).fontSize) * .5;
  const r = videoRect(box.width, box.height, viewArea?.width ?? videoWidth, viewArea?.height ?? videoHeight, preferences.fit);
  return { locale: language, developer: preferences.developer, theme: 'dark', reducedMotion: reducedMotion(), viewport: { w: innerWidth, h: innerHeight },
    video: { x: box.x + r.x, y: box.y + r.y, w: r.width, h: r.height },
    gear: gear.hidden ? settingsAnchor(innerWidth, parseFloat(style.right), parseFloat(style.top), gap) : { gap, x: g.x, y: g.y, w: g.width, h: g.height, faded: gear.classList.contains('faded'), side: 'right' }, channel: '/plugins/channel' };
}, (code, detail) => { console.warn(code, detail ?? ''); }, wakeGear, syncGear);
const pluginSwitch = element<HTMLInputElement>('plugins-enabled');
pluginSwitch.checked = pluginSlot.preference;
function updatePlugins(): void {
  element('plugins-value').textContent = text[pluginSwitch.checked ? 'enabled' : 'off'];
  element('reset-plugin-positions').hidden = !pluginSwitch.checked;
}
pluginSwitch.onchange = () => { pluginSlot.enable(pluginSwitch.checked); updatePlugins(); };
updatePlugins();

let protocolState = 'Not connected';
let playback: PlaybackUi = { state: 'connecting', protocol: protocolState, holding: false };
function updatePlayback(event: PlaybackEvent): void {
  playback = playbackUi(playback, event); setState(playback.state);
}
let micState: TextKey = 'idle';
let requestedMic: { request: MicFormat | null; token: number } | null = null;
let initialized = false, starting = false;
const soundHint = element('sound-hint');
let audioContext: AudioContext | null = null, player: AudioWorkletNode | null = null, gain: GainNode | null = null;
let audioActive = false;
function syncSound(): void {
  const prompt = soundPrompt(uiState === 'playing', preferences.sound, audioContext?.state, audioActive);
  soundHint.hidden = prompt !== 'pill';
  element('card-sound').hidden = prompt !== 'card';
  element('controls').classList.toggle('playing', uiState === 'playing');
  pluginSlot.refresh();
}
const reducedMotion = () => document.documentElement.classList.contains('reduced-motion') || matchMedia('(prefers-reduced-motion: reduce)').matches;
const motion = matchMedia('(prefers-reduced-motion: reduce)');
const syncMotion = () => {
  applyReducedMotion(document.documentElement, motion.matches);
  if (motion.matches) { document.getAnimations().forEach(animation => animation.cancel()); element('headline').textContent = text[statusCard(uiState).headline]; }
};
motion.addEventListener('change', syncMotion); syncMotion();
element('version').textContent = version; element('about-value').textContent = version;
element('close-settings').setAttribute('aria-label', text.close);
let cardTimer: ReturnType<typeof setTimeout>;
let countdown: ReturnType<typeof setInterval> | undefined;
const codeInput = element<HTMLInputElement>('code');
function renderCode(): void {
  codeInput.value = codeInput.value.replace(/\D/g, '').slice(0, 8);
  [...element('digits').children].forEach((digit, index) => {
    digit.textContent = codeInput.value[index] ?? '';
    digit.classList.toggle('next', index === Math.min(7, codeInput.value.length));
  });
}
codeInput.oninput = renderCode; renderCode();
function setState(state: StatusState): void {
  if (state === uiState && element('card').dataset.state === state) return;
  uiState = state;
  pluginSlot.receiverState(state === 'playing' ? 'playing' : state === 'error' ? 'error' : 'waiting');
  syncSound();
  element('protocol-state').textContent = `Protocol: ${protocolState}`;
  clearInterval(countdown); clearTimeout(cardTimer);
  const card = statusCard(state), panel = element('card');
  panel.dataset.state = state;
  panel.classList.toggle('leaving', card.hidden);
  if (card.hidden && !reducedMotion()) cardTimer = setTimeout(() => { panel.hidden = true; }, 200);
  else panel.hidden = card.hidden;
  const headline = element('headline');
  if (headline.textContent !== text[card.headline]) {
    if (!reducedMotion()) {
      const fade = headline.animate?.([{ opacity: 1 }, { opacity: 0 }], { duration: 100 });
      if (fade) fade.onfinish = () => { if (state !== uiState) return; headline.textContent = text[card.headline]; headline.animate?.([{ opacity: 0 }, { opacity: 1 }], { duration: 100 }); };
      else headline.textContent = text[card.headline];
    } else headline.textContent = text[card.headline];
  }
  element('subline').textContent = text[card.subline];
  element('pairing').hidden = state !== 'code' && state !== 'incorrect';
  element('retry').hidden = state !== 'error';
  if (state !== 'playing') setGearFaded(false);
}
let fadeTimer: ReturnType<typeof setTimeout>;
function setGearFaded(faded: boolean): void {
  if (gear.classList.contains('faded') === faded) return;
  gear.classList.toggle('faded', faded); pluginSlot.refresh();
}
function wakeGear(): void {
  clearTimeout(fadeTimer); if (gear.hidden) return;
  setGearFaded(false);
  if (fallbackGear) fadeTimer = setTimeout(() => { if (uiState === 'playing' && !settings.open) setGearFaded(true); }, 3000);
}
function syncGear(): void {
  gear.hidden = !fallbackGear && pluginSlot.cover === 'none'; wakeGear(); pluginSlot.refresh();
}
function showMic(state: TextKey): void {
  micState = state; const status = microphoneStatus(state);
  element('mic-state').textContent = text[status.label];
  element('sound-value').textContent = text[status.label];
  element('mic-hint').textContent = text[status.hint];
  element('mic-retry').hidden = !status.retry || !requestedMic?.request;
}
/** The relay decides the codec; show the active format alongside its preference. */
function showCodecInUse(codec: 'H264' | 'H265' | undefined): void {
  const line = element('video-codec-current');
  line.hidden = !codec;
  line.textContent = codec ? text[codec === 'H265' ? 'codecInUseH265' : 'codecInUseH264'] : '';
}
function updateSettings(): void {
  for (const key of ['sound', 'developer'] as const) element<HTMLInputElement>(key).checked = preferences[key];
  debug.hidden = !preferences.developer;
  element<HTMLInputElement>('frame-rate').value = preferences.frameRate;
  element<HTMLInputElement>('playback').value = preferences.playback;
  element('playback-hint').textContent = text[playbackHint(preferences.playback)];
  element('fit').setAttribute('aria-pressed', String(preferences.fit === 'fit'));
  element('fill').setAttribute('aria-pressed', String(preferences.fit === 'fill'));
  element('quick-fill').setAttribute('aria-pressed', String(preferences.fit === 'fill'));
  element('quick-sound').setAttribute('aria-pressed', String(preferences.sound));
  element('fit-segments').style.setProperty('--index', preferences.fit === 'fill' ? '1' : '0');
  syncChoices(settings); updatePictureValue();
}
function updatePictureValue(): void {
  element('picture-value').textContent = element(displayGate.supported ? 'resolution-value' : 'frame-rate-value').textContent;
}
function updatePictureSize(): void {
  const box = canvas.getBoundingClientRect(), dpr = devicePixelRatio || 1;
  const size = (w: number, h: number) => w && h ? `${w} × ${h}` : '—';
  element('picture-size').textContent = `${text.picture} ${size(videoWidth, videoHeight)} · ${text.windowSize} ${size(Math.round(box.width * dpr), Math.round(box.height * dpr))}`;
}
const drawer = bindDrawer(settings, reducedMotion, open => {
  if (open) releaseAll(); pluginSlot.visibility(!open); wakeGear();
}, () => { pluginSlot.collapse(); });
element('reset-plugin-positions').onclick = () => pluginSlot.resetPositions();
gear.onclick = () => { if (pluginSlot.collapse()) { wakeGear(); return; } drawer.open(); };
element('card-settings').onclick = () => drawer.open();
for (const id of ['sound', 'fullscreen']) element(`quick-${id}`).onclick = () => element(id).click();
element('quick-fill').onclick = () => element(preferences.fit === 'fill' ? 'fit' : 'fill').click();
document.addEventListener('pointerdown', event => { if (event.target !== canvas) wakeGear(); }, { passive: true });
const fullscreen = new FullscreenIntent(!!document.fullscreenEnabled, () => localStorage);
const fullscreenHistory = new FullscreenExitHistory();
const restoreSwitch = element<HTMLInputElement>('fullscreen-restore');
restoreSwitch.checked = fullscreen.restore;
restoreSwitch.onchange = () => { fullscreen.setRestore(restoreSwitch.checked); fullscreenState(); };
function fullscreenState(): void {
  document.querySelectorAll<SVGElement>('[data-fullscreen]').forEach(icon => icon.toggleAttribute('hidden', (icon.dataset.fullscreen === 'on') !== fullscreen.intent));
  element('fullscreen').setAttribute('aria-pressed', String(fullscreen.intent));
  element('fullscreen-hint').hidden = !fullscreen.pending;
  element('fullscreen-state').textContent = fullscreen.state === 'fullscreenUnavailable' ? text.fullscreenUnavailable : '';
  for (const id of ['fullscreen', 'quick-fullscreen']) {
    element<HTMLButtonElement>(id).disabled = !document.fullscreenEnabled;
    element(id).setAttribute('aria-pressed', String(fullscreen.intent));
  }
}
async function requestFullscreen(): Promise<void> {
  try {
    if (!document.documentElement.requestFullscreen) throw new Error('Fullscreen unavailable');
    await document.documentElement.requestFullscreen();
  } catch { fullscreen.denied(); fullscreenState(); }
}
async function reportFullscreenExit(): Promise<void> {
  if (document.hidden) await new Promise<void>(resolve => {
    const visible = () => { if (!document.hidden) { document.removeEventListener('visibilitychange', visible); resolve(); } };
    document.addEventListener('visibilitychange', visible);
  });
  const hiddenMs = fullscreenHistory.take(performance.now()), display = measuredDisplay();
  const autoRestore = await autoRestoreFullscreen({ enabled: () => fullscreen.restore && fullscreen.intent,
    restored: () => !!document.fullscreenElement, hidden: () => document.hidden,
    visible: () => new Promise<void>(resolve => { const visible = () => { if (!document.hidden) { document.removeEventListener('visibilitychange', visible); resolve(); } }; document.addEventListener('visibilitychange', visible); }),
    request: () => document.documentElement.requestFullscreen(), delay: ms => new Promise(resolve => setTimeout(resolve, ms)) });
  post({ type: 'fullscreen-exit', hiddenMs, display, autoRestore });
}
document.addEventListener('pointerdown', event => {
  if (settings.open || (event.target instanceof Node && element('fullscreen').contains(event.target))) return;
  const decision = fullscreen.pointerDown(event);
  if (decision.swallow) { event.preventDefault(); event.stopImmediatePropagation(); }
  if (decision.request) void requestFullscreen();
}, { capture: true });
for (const type of ['pointerup', 'pointercancel']) document.addEventListener(type, event => {
  const pointer = event as PointerEvent;
  const decision = fullscreen.pointerUp(pointer.pointerId, type === 'pointercancel');
  if (decision.swallow) { event.preventDefault(); event.stopImmediatePropagation(); }
  if (decision.request) void requestFullscreen();
}, { capture: true });
document.addEventListener('fullscreenchange', () => {
  if (fullscreen.changed(!!document.fullscreenElement, hello ? measuredDisplay() : undefined)) { releaseAll(); void reportFullscreenExit(); }
  fullscreenState(); resize();
});
fullscreenState();
element('fullscreen').onclick = () => {
  fullscreen.toggle(() => { void requestFullscreen(); }, () => { void document.exitFullscreen().catch(() => {}); });
  fullscreenState(); reportDisplay();
};
for (const key of ['sound', 'developer'] as const) {
  element<HTMLInputElement>(key).onchange = event => {
    preferences[key] = (event.target as HTMLInputElement).checked; storage.save(preferences); updateSettings();
    if (key === 'developer') pluginSlot.refresh();
    if (key === 'sound') {
      if (audioContext && gain) gain.gain.setValueAtTime(preferences.sound ? 1 : 0, audioContext.currentTime);
      if (preferences.sound) void audioContext?.resume().catch(() => {});
      syncSound();
    }

  };
}
for (const fit of ['fit', 'fill'] as const) element(fit).onclick = () => {
  releaseAll(); preferences.fit = fit; storage.save(preferences); updateSettings(); post({ type: 'fit', fit });
};
let frameRateTimer: ReturnType<typeof setTimeout> | undefined;
function applying(value: boolean): void {
  for (const id of ['frame-rate-status', 'frame-rate-choice-status']) element(id).hidden = !value;
}
function unlockFrameRate(): void {
  clearTimeout(frameRateTimer); element<HTMLInputElement>('frame-rate').disabled = false;
  applying(false); syncChoices(settings);
}
function lockFrameRate(): void {
  clearTimeout(frameRateTimer); element<HTMLInputElement>('frame-rate').disabled = true; syncChoices(settings);
  frameRateTimer = setTimeout(unlockFrameRate, 10_000);
}
for (const id of ['resolution', 'screen-width', 'relay-video-codec']) element<HTMLInputElement>(id).onchange = () => {
  if (initialized && displayGate.supported && restartGate.allow(performance.now())) {
    const message = displayGate.message(element<HTMLInputElement>('resolution').value as DisplaySettings['resolution'], Number(element<HTMLInputElement>('screen-width').value) as DisplaySettings['screenWidth'], element<HTMLInputElement>('relay-video-codec').value as DisplaySettings['videoCodec'], currentDisplaySettings.frameRate);
    if (message) post(message);
  }
  updateDisplaySettings();
};
element<HTMLInputElement>('frame-rate').onchange = event => {
  const choice = event.target as HTMLInputElement;
  if (choice.disabled) { updateSettings(); return; }
  const change = frameRateSync.change(frameRatePreference(choice.value), hello, initialized, displayGate.supported, performance.now());
  preferences.frameRate = frameRateSync.preference; storage.save(preferences); updateSettings();
  if (change.displaySettings) post(change.displaySettings);
  if (change.hello) { hello = change.hello; post({ type: 'hello', hello }); }
  if (change.restart) post({ type: 'restart' });
  if (change.displaySettings || change.restart) { applying(true); lockFrameRate(); }
};
// The mode applies without a restart: the receiver and the relay's queue for this client follow the repeated hello.
element<HTMLInputElement>('playback').onchange = event => {
  const mode = playbackMode((event.target as HTMLInputElement).value);
  if (mode === preferences.playback) return;
  preferences.playback = mode; storage.save(preferences); updateSettings();
  if (!initialized) return;
  hello = withPlayback(hello, mode); post({ type: 'hello', hello });
};
element('mic-retry').onclick = () => { if (requestedMic?.request) void microphone(requestedMic.request, requestedMic.token); };
for (const id of ['sound-hint', 'card-sound']) element(id).onclick = () => { void audioContext?.resume().then(syncSound).catch(() => {}); };
element<HTMLFormElement>('pairing').onsubmit = event => {
  event.preventDefault(); if (!pairing.submit(codeInput.value)) return;
  element('paired').hidden = true; setState('connecting');
  if (initialized) { hello = { ...hello, token: pairing.token! }; post({ type: 'pair', token: pairing.token! }); }
  else void start().catch(() => setState('error'));
};
element('retry').onclick = () => { if (initialized) post({ type: 'reconnect' }); else void start().catch(() => setState('error')); setState('connecting'); };
function statistics(message: Extract<WorkerNotice, { type: 'stats' }>): void {
  if (debug.hidden) return;
  const rows: [string, unknown][] = [['Protocol', protocolState], ['Video', `${videoWidth} × ${videoHeight}`],
    ['Rendered / dropped / skipped', `${message.stats.rendered} / ${message.stats.dropped} / ${message.skipped}`],
    ['Decode / display latency (ms)', `${message.stats.decodeMs?.toFixed(1) ?? '—'} / ${message.stats.renderedLatencyMs?.toFixed(1) ?? '—'}`],
    ['Decode / render queue', `${message.stats.decodeQueue} / ${message.renderQueue} (depth ${message.queueDepth}, ${message.playback})`],
    ['Audio buffered (ms) / underruns', `${message.stats.audioBufferedMs?.toFixed(1) ?? '—'} / ${message.stats.audioUnderruns ?? 0}`],
    ['Microphone', micState], ['Clock offset / RTT (µs)', `${message.stats.clockOffsetUs ?? '—'} / ${message.stats.rttUs ?? '—'}`]];
  for (const a of message.audio) rows.push([`Audio ${a.stream}: packets / decoder outputs`, `${a.packets} / ${a.decoderOutputs}`],
    [`Audio ${a.stream}: max arrival / decode gap (ms)`, `${a.maxArrivalGapMs.toFixed(1)} / ${a.maxDecoderOutputGapMs.toFixed(1)}`]);
  for (const a of message.audioBuffers) rows.push([`Buffer ${a.stream}/${a.run}: min / max / target (ms)`, `${a.minBufferedMs.toFixed(1)} / ${a.maxBufferedMs.toFixed(1)} / ${a.targetMs}`]);
  const list = document.createElement('dl');
  for (const [label, value] of rows) { const dt = document.createElement('dt'), dd = document.createElement('dd'); dt.textContent = label; dd.textContent = String(value); list.append(dt, dd); }
  element('statistics').replaceChildren(list);
}
syncGear();
setState(uiState); updateSettings(); showMic('idle');
// Read permission state without prompting or starting a microphone stream.
void navigator.permissions?.query({ name: 'microphone' as PermissionName }).then(permission => {
  const update = () => {
    if (permission.state === 'denied') showMic('denied');
    else if (micState === 'denied') showMic('idle');
  };
  permission.addEventListener('change', update); update();
}).catch(() => {});
const contactLimit = Math.min(10, navigator.maxTouchPoints || 0);
const contacts = new Map<number, TouchContact>();
let videoWidth = 0, videoHeight = 0, worker: Worker | null = null, hello: Hello;
let micAvailable = false;
const captureLifecycle = new CaptureLifecycle();
let supportResult: ((aac: boolean) => void) | null = null;
function stopCapture(): void { captureLifecycle.stop(); showMic('idle'); post({ type: 'mic-detach' }); }
async function microphone(request: MicFormat | null, token: number): Promise<void> {
  post({ type: 'mic-detach' });
  const fence = captureLifecycle.begin();
  if (!request) { showMic('idle'); return; }
  if (!audioContext || request.codec !== 'PCM_S16LE' || request.frameMs !== 20 ||
      request.sampleRate < 8000 || request.sampleRate > 48000 || request.sampleRate % 50 || ![1, 2].includes(request.channels)) { showMic('unavailable'); return; }
  showMic('requesting');
  let stream: MediaStream;
  try {
    stream = await navigator.mediaDevices.getUserMedia({ audio: {
      echoCancellation: true, noiseSuppression: true, autoGainControl: true, channelCount: 1,
    } });
  } catch { if (captureLifecycle.current(fence)) showMic('denied'); return; }
  if (!captureLifecycle.adopt(fence, stream)) return;
  for (const track of stream.getAudioTracks()) track.onended = () => {
    if (captureLifecycle.current(fence)) { stopCapture(); showMic('denied'); }
  };
  try {
    const captureSource = audioContext.createMediaStreamSource(stream);
    const capture = new AudioWorkletNode(audioContext, 'oxp-capture', { processorOptions: { rate: request.sampleRate, channels: request.channels, token } });
    captureLifecycle.attach(captureSource, capture);
    const channel = new MessageChannel();
    capture.port.postMessage({ port: channel.port1 }, [channel.port1]);
    worker?.postMessage({ type: 'mic-port', port: channel.port2, token } satisfies PageCommand, [channel.port2]);
    captureSource.connect(capture); capture.connect(audioContext.destination); showMic('capturing');
  } catch { stopCapture(); showMic('unavailable'); }
}
// Only these events grant user activation for touch input (pointerdown does so only for a mouse).
for (const type of ['pointerup', 'touchend', 'click', 'keydown']) {
  document.addEventListener(type, () => {
    if (audioContext?.state === 'suspended') void audioContext.resume().then(syncSound).catch(() => {});
  }, { passive: true });
}
async function prepareAudio(): Promise<boolean> {
  try {
    if (typeof AudioContext === 'undefined' || typeof AudioWorkletNode === 'undefined') return false;
    audioContext = new AudioContext({ latencyHint: 'interactive' });
    await audioContext.audioWorklet.addModule(workletUrl);
    player = new AudioWorkletNode(audioContext, 'oxp-player', { outputChannelCount: [2] });
    gain = audioContext.createGain(); gain.gain.value = preferences.sound ? 1 : 0;
    player.connect(gain); gain.connect(audioContext.destination);
    const channel = new MessageChannel();
    player.port.postMessage({ port: channel.port1 }, [channel.port1]);
    worker!.postMessage({ type: 'audio-port', port: channel.port2 } satisfies PageCommand, [channel.port2]);
    audioContext.onstatechange = syncSound; syncSound();
    return true;
  } catch { void audioContext?.close(); audioContext = null; syncSound(); return false; }
}
function safeArea(dpr: number) {
  const style = getComputedStyle(document.documentElement);
  const inset = (side: string) => (parseFloat(style.getPropertyValue(`--safe-${side}`)) || 0) * dpr;
  return { top: inset('top'), right: inset('right'), bottom: inset('bottom'), left: inset('left') };
}
function post(message: PageCommand): void { worker?.postMessage(message); }
function releaseAll(): void { contacts.clear(); post({ type: 'touch', contacts: [] }); }
function resize(): void {
  pluginSlot.refresh(); updatePictureSize();
  const box = canvas.getBoundingClientRect(), dpr = devicePixelRatio || 1;
  const width = Math.max(1, Math.round(box.width * dpr)), height = Math.max(1, Math.round(box.height * dpr));
  if (worker) post({ type: 'resize', width, height }); else { canvas.width = width; canvas.height = height; }
  reportDisplay();

}
function measuredDisplay(): DisplayInfo {
  const dpr = devicePixelRatio || 1, box = canvas.getBoundingClientRect();
  return { ...hello.displays[0], ...stableScreen.measure(screen.width, screen.height, dpr), safeArea: safeArea(dpr),
    viewport: { width: Math.max(1, Math.round(box.width)), height: Math.max(1, Math.round(box.height)) },
    fullscreen: !!document.fullscreenElement, fullscreenPreferred: fullscreen.intent };
}
function reportDisplay(): void {
  if (!hello) return;
  const display = fullscreen.display(measuredDisplay());
  if (displayDebounce.observe(display, performance.now())) { hello = { ...hello, displays: [display] }; post({ type: 'hello', hello }); }
}
setInterval(reportDisplay, 100);
function position(event: PointerEvent, clamp: boolean) {
  const box = canvas.getBoundingClientRect();
  const rect = videoRect(box.width, box.height, viewArea?.width ?? videoWidth, viewArea?.height ?? videoHeight, preferences.fit);
  return canvasTouch(mapTouch(event.clientX - box.left, event.clientY - box.top, rect, clamp), viewArea);
}
canvas.addEventListener('pointerdown', event => {
  event.preventDefault();
  const point = position(event, false); if (!point) { wakeGear(); return; }
  if (contacts.has(event.pointerId) || contacts.size >= contactLimit || event.button !== 0) return;
  const used = new Set([...contacts.values()].map(c => c.id));
  const id = Array.from({ length: contactLimit }, (_, i) => i).find(i => !used.has(i));
  if (id === undefined) return;
  contacts.set(event.pointerId, { id, pressed: true, ...point }); canvas.setPointerCapture(event.pointerId);
  post({ type: 'touch', contacts: [...contacts.values()] });
});
canvas.addEventListener('pointermove', event => {
  const contact = contacts.get(event.pointerId); if (!contact) return;
  event.preventDefault(); const point = position(event, true); if (!point) return;
  contacts.set(event.pointerId, { ...contact, ...point }); post({ type: 'touch', contacts: [...contacts.values()] });
});
canvas.addEventListener('pointerup', event => {
  const contact = contacts.get(event.pointerId); if (!contact) return;
  const point = position(event, true) ?? contact;
  contacts.delete(event.pointerId);
  post({ type: 'touch', contacts: [...contacts.values(), { ...contact, x: point.x, y: point.y, pressed: false }] });
  if (canvas.hasPointerCapture(event.pointerId)) canvas.releasePointerCapture(event.pointerId);
});
canvas.addEventListener('pointercancel', releaseAll);
canvas.addEventListener('lostpointercapture', event => { if (contacts.has(event.pointerId)) releaseAll(); });
document.addEventListener('visibilitychange', () => { fullscreenHistory.visibility(document.hidden, performance.now()); releaseAll(); post({ type: 'visibility', visible: !document.hidden }); });
window.addEventListener('pagehide', () => { stopCapture(); void audioContext?.close(); releaseAll(); post({ type: 'stop' }); });
window.addEventListener('pageshow', event => { if (event.persisted) location.reload(); });
new ResizeObserver(resize).observe(canvas);
let receiverInfoStarted = false;
let lag: number[] = [], longTaskCount = 0, longTaskTotalMs = 0, previousTick = performance.now(), pageWindow = previousTick;
try {
  new PerformanceObserver(list => { for (const entry of list.getEntries()) { longTaskCount++; longTaskTotalMs += entry.duration; } }).observe({ type: 'longtask', buffered: false });
} catch { /* Long tasks are unavailable in some browsers. */ }
setInterval(() => {
  const now = performance.now(); lag.push(Math.max(0, now - previousTick - 100)); previousTick = now;
  if (now - pageWindow < 1000) return;
  let usedJSHeapSize: number | undefined;
  try { usedJSHeapSize = (performance as Performance & { memory?: { usedJSHeapSize: number } }).memory?.usedJSHeapSize; } catch { /* Optional Chromium heap measurement. */ }
  post({ type: 'page-metrics', metrics: { eventLoopLagAvgMs: lag.reduce((a, b) => a + b, 0) / lag.length, eventLoopLagMaxMs: Math.max(...lag), longTaskCount, longTaskTotalMs, usedJSHeapSize } });
  lag = []; longTaskCount = 0; longTaskTotalMs = 0; pageWindow = now;
}, 100);
async function start(): Promise<void> {
  if (starting || initialized) return;
  starting = true;
  try {
    if (!isSecureContext || !canvas.transferControlToOffscreen || typeof Worker === 'undefined' || typeof VideoDecoder === 'undefined') {
      setState('unsupported'); return;
    }
    setState('connecting');
    const hardware = new Map<string, boolean>();
    void Promise.all(codecCandidates.map(async codec => hardware.set(codec, await codecPowerEfficient(codec)))).then(() => {
      if (hello) { hello = { ...hello, decoders: hello.decoders.map(d => ({ ...d, hardware: hardware.get(d.codecString) ?? false })) }; if (initialized) post({ type: 'hello', hello }); }
    });
    const [probes, refreshHz] = await Promise.all([probeDecoders(), estimateRefreshHz()]);
    const dpr = devicePixelRatio || 1;
    hello = { type: 'hello', version: 0, token: pairing.token ?? '',
      decoders: [...new Set(probes.filter(p => p.supported).map(p => p.codecString))].map(codecString => ({ codecString, hardware: hardware.get(codecString) ?? false })),
      displays: [{ screen: 0, ...stableScreen.measure(screen.width, screen.height, dpr), refreshHz,
        safeArea: safeArea(dpr) }], maxTouchContacts: contactLimit, audio: [], microphone: false };
    hello = withPlayback(frameRateSync.hello(hello), preferences.playback);
    hello = { ...hello, displays: [fullscreen.display(measuredDisplay())] }; displayDebounce.seed(hello.displays[0]);
    resize();
    worker?.terminate(); void audioContext?.close(); audioContext = null; gain = null;
    let workerFailed = false;
    worker = new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });
    worker.onmessage = (event: MessageEvent<WorkerNotice>) => {
      const message = event.data;
      switch (message.type) {
        case 'audio-support': supportResult?.(message.aac); supportResult = null; break;
        case 'audio-active': audioActive = message.active; syncSound(); break;
        case 'mic': requestedMic = message; void microphone(message.request, message.token); break;
        case 'status':
          protocolState = message.text;
          updatePlayback(message); break;
        case 'retry': {
          const until = performance.now() + message.delayMs;
          const tick = () => { if (uiState === 'reconnecting') element('subline').textContent = text.attempt.replace('{attempt}', String(message.attempt)).replace('{seconds}', String(Math.max(0, Math.ceil((until - performance.now()) / 1000)))); };
          tick(); countdown = setInterval(tick, 1000); break;
        }
        case 'hold': updatePlayback(message); break;
        case 'ended': updatePlayback(message); break;
        case 'playing': if (!frameRateSync.synced) applying(false); updatePlayback(message); wakeGear(); break;
        case 'rejected': {
          protocolState = 'Pairing rejected (1008)'; const rejectedState = pairing.rejected(message.token); fragment.delete('k');
          const hash = fragment.toString();
          history.replaceState(null, '', `${location.pathname}${location.search}${hash ? `#${hash}` : ''}`);
          codeInput.value = ''; renderCode(); playback = { state: rejectedState, protocol: protocolState, holding: false }; setState(rejectedState);
          if (rejectedState === 'incorrect' && !reducedMotion()) element('digits').animate?.([{ transform: 'translateX(0)' }, { transform: 'translateX(-6px)' }, { transform: 'translateX(6px)' }, { transform: 'translateX(0)' }], { duration: 250 });
          codeInput.focus(); break;
        }
        case 'view-area': releaseAll(); viewArea = message; pluginSlot.refresh(); break;
        case 'host-ui': if (!fallbackGear) drawer.open(); break;
        case 'display-disconnected': {
          viewArea = undefined; fullscreen.viewAreas = false; displayGate.welcome();
          const wasSynced = frameRateSync.synced; frameRateSync.disconnect();
          const { frameRate: _, ...legacy } = currentDisplaySettings; currentDisplaySettings = legacy;
          if (wasSynced && hello) { hello = frameRateSync.hello(hello); post({ type: 'hello', hello }); }
          unlockFrameRate(); updateDisplaySettings(); break;
        }
        case 'display-settings':
          unlockFrameRate(); currentDisplaySettings = message; frameRateSync.receive(message); updateDisplaySettings();
          if (message.frameRate !== undefined) {
            preferences.frameRate = frameRateSync.preference; storage.save(preferences); updateSettings();
            if (hello.displays.some(display => display.frameRate !== undefined)) { hello = frameRateSync.hello(hello); post({ type: 'hello', hello }); }
          }
          break;
        case 'welcome':
          if (shouldReload(message.receiverBuild, () => sessionStorage)) { location.reload(); break; }
          fallbackGear = fallbackSettings(message.features); syncGear();
          viewArea = undefined;
          fullscreen.viewAreas = message.features?.includes('view-area') === true;
          displayGate.welcome(message.features); updateDisplaySettings();
          element('relay-version-row').hidden = !message.relay;
          element('relay-name').textContent = message.relay?.name ?? '';
          element('relay-version').textContent = message.relay?.version ?? '';
          pluginSlot.configure(message);
          if (!receiverInfoStarted) { receiverInfoStarted = true; void collectReceiverInfo().then(info => post({ type: 'receiver-info', info })); }
          pairing.welcome(message.token); source.href = message.sourceUrl;
          if (!codeInput.value) break;
          element('paired').hidden = false;
          element('paired').textContent = `✓ ${text.paired}`;
          setTimeout(() => { element('paired').hidden = true; }, reducedMotion() ? 0 : 250);
          break;
        case 'format':
          releaseAll(); videoWidth = message.format?.width ?? 0; videoHeight = message.format?.height ?? 0;
          showCodecInUse(message.format?.codec); updatePictureSize();
          break;
        case 'stats': statistics(message); break;
      }
    };
    worker.onerror = () => { workerFailed = true; supportResult?.(false); supportResult = null; stopCapture(); releaseAll(); setState('error'); };
    const worklet = await prepareAudio();
    const aac = await new Promise<boolean>(resolve => {
      const timeout = setTimeout(() => { supportResult = null; resolve(false); }, 2000);
      supportResult = supported => { clearTimeout(timeout); resolve(supported); };
      post({ type: 'audio-probe' });
    });
    if (workerFailed) throw new Error('Receiver worker failed');
    micAvailable = worklet && typeof navigator.mediaDevices?.getUserMedia === 'function';
    hello = { ...hello, ...audioCapabilities(worklet, aac, preferences.microphone && micAvailable) };
    hello = { ...hello, decoders: hello.decoders.map(d => ({ ...d, hardware: hardware.get(d.codecString) ?? false })) };
    const offscreen = canvas.transferControlToOffscreen();
    const message: PageCommand = { type: 'init', canvas: offscreen, url: `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/oxp/v0`, hello,
      queueDepth: renderQueueDepth(fragment.get('queue')) };
    worker.postMessage(message, [offscreen]); initialized = true;
    post({ type: 'fit', fit: preferences.fit });
    post({ type: 'visibility', visible: !document.hidden });
  } finally { starting = false; }
}
void start().catch(() => setState('error'));
