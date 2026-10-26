// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-25

import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { bindDrawer } from '../receiver/drawer';
import { drawerPages } from '../receiver/drawer-core';

type Handler = (event: any) => void;
class Node {
  dataset: Record<string, string> = {};
  attributes = new Map<string, string>();
  children: Node[] = [];
  parent?: Node;
  disabled = false; hidden = false; open = false; isConnected = true;
  offsetWidth = 600;
  onclick?: Handler;
  listeners = new Map<string, Handler[]>();
  classes = new Set<string>();
  values = new Map<string, string>();
  style = { setProperty: (key: string, value: string) => this.values.set(key, value), removeProperty: (key: string) => { this.values.delete(key); delete this.style[key]; } } as any;
  classList = { add: (...names: string[]) => names.forEach(name => this.classes.add(name)), remove: (...names: string[]) => names.forEach(name => this.classes.delete(name)), contains: (name: string) => this.classes.has(name), toggle: (name: string, on: boolean) => on ? this.classes.add(name) : this.classes.delete(name) };
  captured?: number;
  constructor(public tag = 'section', public id = '', public ownerDocument: any = undefined) {}
  append(...nodes: Node[]) { for (const node of nodes) { node.parent = this; node.ownerDocument = this.ownerDocument; this.children.push(node); } }
  set className(value: string) { this.classes = new Set(value.split(' ')); }
  setAttribute(key: string, value: string) { this.attributes.set(key, value); }
  getAttribute(key: string) { return this.attributes.get(key) ?? null; }
  hasAttribute(key: string) { return this.attributes.has(key); }
  removeAttribute(key: string) { this.attributes.delete(key); }
  toggleAttribute(key: string, on: boolean) { if (on) this.setAttribute(key, ''); else this.removeAttribute(key); }
  set tabIndex(value: number) { this.setAttribute('tabindex', String(value)); }
  matches(selector: string): boolean {
    if (selector === ':disabled') return this.disabled;
    if (selector === 'input[type=checkbox]') return false;
    if (selector === '[hidden]') return this.hidden;
    if (selector === '[style="--order:4"]') return this.id === 'last-item';
    if (selector === '[data-choice]' || selector === '[data-page]') return selector.slice(6, -1) in this.dataset;
    if (selector === '[role=group]' || selector === 'input[type=hidden]') return false;
    if (selector.startsWith('[data-')) return selector.includes('drawer-page') ? !!this.dataset.drawerPage : !!this.dataset.title;
    return selector.startsWith('#') ? this.id === selector.slice(1) : selector.startsWith('.') ? this.classes.has(selector.slice(1)) : this.tag === selector;
  }
  querySelectorAll(selector: string): Node[] {
    return this.children.flatMap(node => [...(selector.split(',').some(part => node.matches(part.trim())) ? [node] : []), ...node.querySelectorAll(selector)]);
  }
  querySelector(selector: string) { return this.querySelectorAll(selector)[0] ?? null; }
  closest(selector: string): Node | null { return this.matches(selector) ? this : this.parent?.closest(selector) ?? null; }
  getClientRects() { return [1]; }
  getBoundingClientRect = vi.fn(() => ({ width: 600 }));
  focus() { this.ownerDocument.activeElement = this; }
  showModal() { this.open = true; }
  close() { this.open = false; }
  addEventListener(name: string, handler: Handler) { this.listeners.set(name, [...this.listeners.get(name) ?? [], handler]); }
  emit(name: string, event: any) { for (const handler of this.listeners.get(name) ?? []) handler(event); }
  setPointerCapture(id: number) { this.captured = id; }
  hasPointerCapture(id: number) { return this.captured === id; }
  releasePointerCapture() { this.captured = undefined; }
}
function fixture(reduced = false) {
  const events = new Node(), winEvents = new Node();
  const doc: any = { addEventListener: events.addEventListener.bind(events), activeElement: undefined, documentElement: new Node() };
  const win = { innerWidth: 1280, innerHeight: 720, setTimeout, requestAnimationFrame: (fn: () => void) => setTimeout(fn, 16), cancelAnimationFrame: clearTimeout,
    addEventListener: winEvents.addEventListener.bind(winEvents), getComputedStyle: () => ({ fontSize: '16' }) };
  doc.defaultView = win;
  const make = (tag = 'section', id = '') => new Node(tag, id, doc);
  const dialog = make('dialog'), panel = make(), scrim = make(), back = make('button', 'drawer-back'), close = make('button', 'close-settings');
  panel.classList.add('panel'); scrim.classList.add('scrim');
  const opener = make('button'); doc.activeElement = opener;
  const pages = drawerPages.map(name => { const page = make(); page.dataset.drawerPage = name; page.setAttribute('data-drawer-page', name); page.classList.add('page'); page.append(make('button', `${name}-first`)); return page; });
  const titles = drawerPages.map(name => { const node = make(); node.dataset.title = name; return node; });
  const row = make('button'); row.dataset.page = 'picture'; pages[0].append(row, make('div', 'last-item'));
  panel.append(back, close, ...titles, ...pages); dialog.append(scrim, panel);
  const visibility = vi.fn(); const drawer = bindDrawer(dialog as unknown as HTMLDialogElement, () => reduced, visibility);
  const pointer = (type: string, x: number, y = 20, timeStamp = 1) => panel.emit(type, { pointerId: 1, isPrimary: true, button: 0, clientX: x, clientY: y, timeStamp, preventDefault() {} });
  const key = (key: string) => events.emit('keydown', { key, shiftKey: false, target: doc.activeElement, preventDefault() {} });
  const pointerDown = () => events.emit('pointerdown', { target: doc.documentElement });
  return { pointerDown, dialog, panel, scrim, back, close, drawer, pages, doc, opener, row, visibility, pointer, key, win, winEvents };
}
beforeEach(() => vi.useFakeTimers());
afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); });

it('starts within one frame, keeps an already-open subpage, and resets only after closing', () => {
  const f = fixture(); f.drawer.open(); expect(f.dialog.open).toBe(true);
  expect(f.dialog.classList.contains('is-open')).toBe(false);
  vi.advanceTimersByTime(16); expect(f.dialog.classList.contains('is-open')).toBe(true);
  expect(f.doc.activeElement).toBe(f.pages[0]);
  vi.advanceTimersByTime(560);
  f.drawer.push('picture'); vi.advanceTimersByTime(16); expect(f.pages[1].classList.contains('active')).toBe(true);
  vi.advanceTimersByTime(480); f.drawer.open(); expect(f.pages[1].classList.contains('active')).toBe(true);
  f.drawer.close(); expect(f.dialog.open).toBe(true); expect(f.pages[1].classList.contains('active')).toBe(true);
  f.panel.emit('transitionend', { target: f.panel, propertyName: 'transform' });
  expect(f.dialog.open).toBe(false); expect(f.pages[0].classList.contains('active')).toBe(true);
  expect(f.doc.activeElement).toBe(f.opener); expect(f.visibility.mock.calls).toEqual([[true], [false]]);
  expect(f.dialog.classList.contains('is-moving')).toBe(false);
});
it('uses native modal semantics, isolates hidden pages and restores the menu row on Back', () => {
  const f = fixture(true); f.drawer.open();
  f.dialog.emit('click', { target: f.row }); expect(f.pages[1].classList.contains('active')).toBe(true);
  expect(f.pages[0].hasAttribute('inert')).toBe(true); expect(f.pages[0].children[0].getAttribute('tabindex')).toBe('-1');
  expect(f.pages[1].hasAttribute('inert')).toBe(false); expect(f.doc.activeElement).toBe(f.pages[1]);
  f.key('BrowserBack'); expect(f.doc.activeElement).toBe(f.row); expect(f.pages[0].children[0].getAttribute('tabindex')).toBeNull();
  f.key('Escape'); expect(f.dialog.open).toBe(false);
  f.key('Settings'); expect(f.dialog.open).toBe(true); f.key('ContextMenu'); expect(f.dialog.open).toBe(false);
});
it('focuses the first control only when a key opened the drawer', () => {
  const f = fixture(true);
  f.key('Settings'); expect(f.dialog.open).toBe(true); expect(f.doc.activeElement.id).toBe('root-first');
  f.key('Escape'); expect(f.dialog.open).toBe(false);
  f.pointerDown(); f.drawer.open(); expect(f.doc.activeElement).toBe(f.pages[0]);
});
it('reverses a closing drawer and ignores stale close timers', () => {
  const f = fixture(); f.drawer.open(); vi.advanceTimersByTime(600); f.drawer.close();
  vi.advanceTimersByTime(100); f.drawer.open(); vi.advanceTimersByTime(600);
  expect(f.dialog.open).toBe(true); expect(f.dialog.classList.contains('is-open')).toBe(true);
  expect(f.visibility.mock.calls).toEqual([[true]]); expect(f.dialog.classList.contains('is-moving')).toBe(false);
});
it('coalesces drag writes, reads panel geometry only at pointerdown and settles a cancelled swipe', () => {
  const f = fixture(); f.drawer.open(); vi.advanceTimersByTime(600);
  f.pointer('pointerdown', 100); f.pointer('pointermove', 160, 22, 30); f.pointer('pointermove', 180, 23, 60);
  expect(f.panel.style.transform).toBeUndefined(); vi.advanceTimersByTime(16);
  expect(f.panel.style.transform).toBe('translate3d(80px,0,0)'); expect(f.scrim.style.opacity).toBe(String(1 - 80 / 600));
  expect(f.panel.getBoundingClientRect).toHaveBeenCalledOnce();
  f.pointer('pointercancel', 180, 23, 70); vi.advanceTimersByTime(400);
  expect(f.dialog.open).toBe(true); expect(f.dialog.classList.contains('is-moving')).toBe(false);
});
it('pops a subpage interactively and completes a root swipe with momentum', () => {
  const f = fixture(); f.drawer.open(); vi.advanceTimersByTime(600); f.drawer.push('picture'); vi.advanceTimersByTime(600);
  f.pointer('pointerdown', 10); f.pointer('pointermove', 250, 20, 200); vi.advanceTimersByTime(16);
  expect(f.pages[0].style.transform).toBe('translate3d(-90px,0,0)');
  f.pointer('pointerup', 250, 20, 201); vi.advanceTimersByTime(400);
  expect(f.pages[0].classList.contains('active')).toBe(true); expect(f.dialog.open).toBe(true);
  f.pointer('pointerdown', 10); f.pointer('pointermove', 250, 20, 200); vi.advanceTimersByTime(16); f.pointer('pointerup', 250, 20, 201);
  expect(f.panel.values.get('--motion-duration')).toBe('280ms'); vi.advanceTimersByTime(360); expect(f.dialog.open).toBe(false);
});
it('leaves vertical scrolling native and cancels a live drag when the viewport changes', () => {
  const f = fixture(); f.drawer.open(); vi.advanceTimersByTime(600);
  f.pointer('pointerdown', 10); f.pointer('pointermove', 15, 100); expect(f.panel.captured).toBeUndefined();
  f.pointer('pointerup', 15, 100); f.pointer('pointerdown', 10); f.pointer('pointermove', 100);
  f.win.innerWidth = 800; f.win.innerHeight = 1280; f.winEvents.emit('resize', {});
  expect(f.panel.dataset.width).toBe('full'); expect(f.panel.captured).toBeUndefined(); expect(f.dialog.classList.contains('is-dragging')).toBe(false);
});

it('reverses an in-flight page push when Back arrives and can close during a drag', () => {
  const f = fixture(); f.drawer.open(); vi.advanceTimersByTime(600);
  f.drawer.push('picture'); vi.advanceTimersByTime(100); f.key('GoBack');
  expect(f.pages[0].classList.contains('active')).toBe(true); vi.advanceTimersByTime(500);
  f.pointer('pointerdown', 10); f.pointer('pointermove', 100); f.key('Escape');
  vi.advanceTimersByTime(400); expect(f.dialog.open).toBe(false); expect(f.panel.captured).toBeUndefined();
  f.drawer.open(); vi.advanceTimersByTime(600); expect(f.pages[0].classList.contains('active')).toBe(true);
});
