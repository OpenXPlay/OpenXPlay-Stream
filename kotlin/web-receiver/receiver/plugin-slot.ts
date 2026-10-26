// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

import { PluginPolicy, resetHostPositions, discover, clipPath, validHostMessage, readHostPrefs, saveHostPrefs, type Geometry, type Region, type Cover } from './plugin-policy';

/** Owns only a sandboxed frame and page lifecycle. No media, transport or input dependency. */
export class PluginSlot {
  private frame?: HTMLIFrameElement;
  private policy!: PluginPolicy;
  private capabilities: { plugins?: { host: string } } = {};
  private timer?: ReturnType<typeof setInterval>;
  private pingAt = 0;
  private diagnosticAt = -Infinity;
  private state: 'waiting' | 'playing' | 'error' = 'waiting';
  private enabled = true;
  private listening = false;
  private visible = true;
  constructor(private geometry: () => Geometry, private diagnostic: (code: string, detail?: string) => void, private wakeGear: () => void = () => {}, private coverChanged: () => void = () => {}) {
    try { this.enabled = localStorage.getItem('oxp.plugins') !== 'false'; } catch { /* Preferences are optional. */ }
  }
  get preference(): boolean { return this.enabled; }
  get cover(): Cover { return this.frame ? this.policy.cover : 'none'; }
  configure(capabilities: { plugins?: { host: string } }): void {
    this.capabilities = capabilities;
    this.mount();
  }
  enable(value: boolean): void {
    this.enabled = value;
    try { localStorage.setItem('oxp.plugins', String(value)); } catch { /* Preferences are optional. */ }
    this.mount();
  }
  private hostGeometry(): Geometry {
    const geometry = this.geometry();
    return { ...geometry, developer: geometry.developer === true, hostPrefs: readHostPrefs(() => localStorage) };
  }
  private send(type: string, data: object = {}): void { this.frame?.contentWindow?.postMessage({ oxp: 'host/0', type, ...data }, '*'); }
  private clip(): void { if (this.frame) this.frame.style.clipPath = clipPath(this.policy.regions); }
  private remove(): void {
    const covered = this.cover !== 'none';
    this.frame?.remove(); this.frame = undefined; clearInterval(this.timer); this.timer = undefined;
    if (covered) this.coverChanged();
  }
  private mount(): void {
    const host = discover(this.capabilities, location.href, this.enabled);
    if (!host) { this.remove(); return; }
    if (this.frame) return;
    if (!this.listening) {
      window.addEventListener('message', this.message);
      window.addEventListener('resize', () => this.refresh());
      document.addEventListener('visibilitychange', () => this.visibility(this.visible));
      this.listening = true;
    }
    const frame = document.createElement('iframe'); frame.title = 'Plugin host';
    frame.setAttribute('sandbox', 'allow-scripts'); frame.setAttribute('allow', ''); frame.referrerPolicy = 'no-referrer';
    frame.className = 'plugins-host'; frame.style.clipPath = 'inset(100%)'; frame.src = host;
    this.policy ??= new PluginPolicy(this.hostGeometry());
    this.frame = frame; this.policy.geometry = this.hostGeometry(); this.policy.mount(performance.now()); this.pingAt = performance.now();
    document.body.append(frame);
    this.timer = setInterval(() => {
      const now = performance.now(), result = this.policy.tick(now);
      if (result !== 'ok') { this.remove(); this.diagnostic('plugins-host-stalled'); if (result === 'reload') this.mount(); return; }
      if (now - this.pingAt >= 2000) { this.pingAt = now; this.refresh(); this.send('ping', { seq: this.policy.ping() }); }
    }, 250);
  }
  private message = (event: MessageEvent): void => {
    if (!this.frame || event.source !== this.frame.contentWindow || !validHostMessage(event.data)) return;
    const m = event.data;
    switch (m.type) {
      case 'ready': this.send('init', this.hostGeometry()); this.receiverState(this.state); this.visibility(this.visible); break;
      case 'wake': this.wakeGear(); break;
      case 'host-prefs': saveHostPrefs(() => localStorage, m.value); break;
      case 'pong': this.policy.pong(m.seq as number, performance.now()); break;
      case 'layout': {
        const previous = this.cover;
        this.policy.geometry = this.hostGeometry(); this.policy.layout(m.regions as Region[], m.cover as Cover); this.clip();
        if (this.cover !== previous) this.coverChanged();
        break;
      }
      case 'diagnostic':
        if (performance.now() - this.diagnosticAt >= 1000) { this.diagnosticAt = performance.now(); this.diagnostic(m.code as string, m.detail as string); }
    }
  };
  refresh(): void {
    if (!this.frame) return;
    this.policy.geometry = this.hostGeometry(); this.policy.layout(this.policy.regions, this.policy.cover); this.clip(); this.send('geometry', this.policy.geometry);
  }
  receiverState(state: 'waiting' | 'playing' | 'error'): void { this.state = state; this.send('receiver-state', { state }); }
  visibility(visible: boolean): void { this.visible = visible; this.send('visibility', { visible: visible && !document.hidden }); }
  resetPositions(): void {
    saveHostPrefs(() => localStorage, resetHostPositions(readHostPrefs(() => localStorage)));
    this.send('reset-positions');
  }
  collapse(): boolean {
    if (!this.frame || this.policy.cover === 'none') return false;
    this.policy.geometry = this.hostGeometry(); this.policy.collapse(); this.clip(); this.send('collapse'); this.coverChanged(); return true;
  }
}
