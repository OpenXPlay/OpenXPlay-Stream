// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

import { afterEach, expect, it, vi } from 'vitest';
import { bindDrawer } from '../receiver/drawer';
import { drawerPages } from '../receiver/drawer-core';

type Listener = (event: any) => void;
class Element {
  children: Element[] = []; parent?: Element; dataset: Record<string, string> = {}; attrs = new Map<string, string>();
  classes = new Set<string>(); listeners = new Map<string, Listener[]>(); properties = new Map<string, string>();
  open = false; disabled = false; hidden = false; isConnected = true; offsetWidth = 640; captured?: number; onclick?: Listener;
  style = {
    setProperty: (key: string, value: string) => { this.properties.set(key, value); },
    removeProperty: (key: string) => { this.properties.delete(key); delete (this.style as any)[key]; },
  } as any;
  classList = {
    add: (...names: string[]) => names.forEach(name => this.classes.add(name)),
    remove: (...names: string[]) => names.forEach(name => this.classes.delete(name)),
    contains: (name: string) => this.classes.has(name),
    toggle: (name: string, on: boolean) => on ? this.classes.add(name) : this.classes.delete(name),
  };
  constructor(public tag: string, public ownerDocument: any, public id = '') {}
  append(node: Element): void { this.children.push(node); node.parent = this; }
  matches(selector: string): boolean {
    if (selector === ':disabled') return this.disabled;
    if (selector === '[hidden]') return this.hidden;
    if (selector.startsWith('#')) return this.id === selector.slice(1);
    if (selector.startsWith('.')) return this.classes.has(selector.slice(1));
    if (selector === '[data-drawer-page]') return !!this.dataset.drawerPage;
    if (selector === '[data-title]') return !!this.dataset.title;
    if (selector === '[data-page]') return !!this.dataset.page;
    if (selector === 'input[type=hidden]' || selector === 'input[type=checkbox]' || selector === '[data-choice]' || selector === '[role=group]') return false;
    return selector === this.tag;
  }
  querySelectorAll(selector: string): Element[] {
    return this.children.flatMap(node => [...(selector.split(',').some(s => node.matches(s.trim())) ? [node] : []), ...node.querySelectorAll(selector)]);
  }
  querySelector(selector: string): Element | undefined { return this.querySelectorAll(selector)[0]; }
  closest(selector: string): Element | undefined { return this.matches(selector) ? this : this.parent?.closest(selector); }
  getAttribute(key: string): string | null { return this.attrs.get(key) ?? null; }
  setAttribute(key: string, value: string): void { this.attrs.set(key, value); }
  removeAttribute(key: string): void { this.attrs.delete(key); }
  toggleAttribute(key: string, on: boolean): void { if (on) this.attrs.set(key, ''); else this.attrs.delete(key); }
  set tabIndex(value: number) { this.setAttribute('tabindex', String(value)); }
  addEventListener(name: string, fn: Listener): void { this.listeners.set(name, [...this.listeners.get(name) ?? [], fn]); }
  emit(name: string, event: any): void { for (const fn of this.listeners.get(name) ?? []) fn(event); }
  getClientRects(): number[] { return [1]; }
  getBoundingClientRect = vi.fn(() => ({ width: 640 }));
  focus(): void { this.ownerDocument.activeElement = this; }
  showModal(): void { this.open = true; }
  close(): void { this.open = false; }
  setPointerCapture(id: number): void { this.captured = id; }
  hasPointerCapture(id: number): boolean { return this.captured === id; }
  releasePointerCapture(): void { this.captured = undefined; }
}
function fixture(reduced = false) {
  vi.useFakeTimers();
  const doc: any = { activeElement: undefined }, events = new Element('document', doc), windowEvents = new Element('window', doc);
  const win = { innerWidth: 1440, innerHeight: 800, requestAnimationFrame: (fn: () => void) => setTimeout(fn, 16), cancelAnimationFrame: clearTimeout, setTimeout,
    getComputedStyle: (_node: unknown) => ({ transform: 'none', opacity: '1', fontSize: '16px' }), addEventListener: windowEvents.addEventListener.bind(windowEvents) };
  Object.assign(doc, { defaultView: win, documentElement: new Element('html', doc), addEventListener: events.addEventListener.bind(events) });
  const make = (tag: string, parent?: Element, id?: string) => { const node = new Element(tag, doc, id); parent?.append(node); return node; };
  const dialog = make('dialog'), panel = make('section', dialog), scrim = make('div', dialog);
  panel.classes.add('panel'); scrim.classes.add('scrim');
  const back = make('button', panel, 'drawer-back'), close = make('button', panel, 'close-settings');
  const pages = new Map(drawerPages.map(name => {
    const title = make('span', panel); title.dataset.title = name;
    const page = make('section', panel); page.dataset.drawerPage = name; page.classes.add('page');
    make('button', page, `control-${name}`); return [name, page];
  }));
  const row = make('button', pages.get('root')); row.dataset.page = 'picture';
  const opener = make('button'); doc.activeElement = opener;
  const visible = vi.fn(), before = vi.fn(), drawer = bindDrawer(dialog as unknown as HTMLDialogElement, () => reduced, visible, before);
  const key = (key: string, target = doc.activeElement, shiftKey = false) => events.emit('keydown', { key, target, shiftKey, preventDefault: vi.fn() });
  const pointer = (type: string, x: number, y = 0, timeStamp = 1) => panel.emit(type, { pointerId: 8, button: 0, isPrimary: true, clientX: x, clientY: y, timeStamp, preventDefault() {} });
  const current = () => [...pages].find(([, page]) => page.classes.has('active'))?.[0];
  return { drawer, dialog, panel, scrim, pages, back, close, opener, row, visible, before, win, doc, key, pointer, current, events, windowEvents };
}
afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); vi.unstubAllGlobals(); });
it('uses a single modal, isolates inactive pages, restores focus and resets after closing', () => {
  const f = fixture(true); f.drawer.open(); expect(f.dialog.open).toBe(true); expect(f.current()).toBe('root');
  f.dialog.emit('click', { target: f.row }); expect(f.current()).toBe('picture');
  expect(f.pages.get('root')!.attrs.has('inert')).toBe(true); expect(f.pages.get('root')!.children[0].getAttribute('tabindex')).toBe('-1');
  f.key('GoBack'); expect(f.current()).toBe('root'); expect(f.doc.activeElement).toBe(f.row);
  expect(f.pages.get('root')!.children[0].getAttribute('tabindex')).toBeNull();
  f.drawer.push('sound'); f.drawer.close(); expect(f.dialog.open).toBe(false); expect(f.current()).toBe('root');
  expect(f.doc.activeElement).toBe(f.opener); expect(f.visible.mock.calls).toEqual([[true], [false]]);
});
it('supports keyboard entry, Tab wrapping, arrows and one-level back', () => {
  const f = fixture(true); f.key('ContextMenu'); expect(f.doc.activeElement.id).toBe('control-root');
  const first = f.pages.get('root')!.children[0]; f.key('ArrowDown', first); expect(f.doc.activeElement).toBe(f.row);
  f.key('Tab', f.row); expect(f.doc.activeElement).toBe(f.close);
  f.key('Tab', f.close, true); expect(f.doc.activeElement).toBe(f.row);
  f.drawer.push('picture'); f.drawer.push('resolution'); expect(f.current()).toBe('resolution');
  f.key('Backspace'); expect(f.current()).toBe('picture'); f.key('Escape'); expect(f.current()).toBe('root');
  f.key('Settings'); expect(f.dialog.open).toBe(false);
});
it.each([0, 80, 420])('lets a tap interrupt the opening transition at %s ms', at => {
  const f = fixture(); f.drawer.open(); vi.advanceTimersByTime(at);
  f.dialog.emit('click', { target: f.row }); vi.advanceTimersByTime(600);
  expect(f.current()).toBe('picture'); expect(f.dialog.classes.has('is-open')).toBe(true);
  expect(f.dialog.classes.has('is-opening')).toBe(false); expect(f.dialog.classes.has('is-moving')).toBe(false);
  expect([...f.pages.values()].every(node => !node.classes.has('participating'))).toBe(true);
  f.drawer.open(); expect(f.current()).toBe('picture');
});
it('reverses a close without a stale timer closing the reopened submenu', () => {
  const f = fixture(); f.drawer.open(); vi.advanceTimersByTime(600); f.drawer.push('about'); vi.advanceTimersByTime(600);
  f.drawer.close(); vi.advanceTimersByTime(110); f.drawer.open();
  f.panel.emit('transitionend', { target: f.panel, propertyName: 'transform' });
  vi.advanceTimersByTime(600); expect(f.dialog.open).toBe(true); expect(f.current()).toBe('about');
  expect(f.visible.mock.calls).toEqual([[true]]); f.drawer.close(); vi.advanceTimersByTime(400); expect(f.current()).toBe('root');
});
it('holds an opening press, then follows a drag and closes using cached layout', () => {
  const f = fixture(); f.drawer.open(); vi.advanceTimersByTime(100);
  f.win.getComputedStyle = () => ({ transform: 'matrix(1,0,0,1,90,0)', opacity: '.8', fontSize: '16px' });
  vi.stubGlobal('DOMMatrixReadOnly', class { m41 = 90; });
  f.pointer('pointerdown', 100); vi.advanceTimersByTime(16);
  expect(f.panel.style.transform).toBe('translate3d(90px,0,0)');
  f.pointer('pointermove', 340, 0, 200); vi.advanceTimersByTime(16);
  expect(f.panel.style.transform).toBe('translate3d(330px,0,0)'); expect(f.panel.getBoundingClientRect).toHaveBeenCalledOnce();
  vi.advanceTimersByTime(600); expect(f.panel.style.transform).toBe('translate3d(330px,0,0)');
  f.pointer('pointerup', 340, 0, 500); vi.advanceTimersByTime(400);
  expect(f.dialog.open).toBe(false); expect(f.panel.style.transform).toBeUndefined(); expect(f.scrim.style.opacity).toBeUndefined();
});
it('preserves vertical scrolling and settles a cancelled horizontal gesture', () => {
  const f = fixture(); f.drawer.open(); vi.advanceTimersByTime(600);
  f.pointer('pointerdown', 0); f.pointer('pointermove', 5, 90, 20); f.pointer('pointerup', 5, 90, 40);
  expect(f.panel.captured).toBeUndefined(); expect(f.dialog.open).toBe(true);
  f.pointer('pointerdown', 0); f.pointer('pointermove', 80, 0, 50); vi.advanceTimersByTime(16);
  f.pointer('pointercancel', 80, 0, 60); vi.advanceTimersByTime(450);
  expect(f.dialog.open).toBe(true); expect(f.panel.style.transform).toBeUndefined(); expect(f.dialog.classes.has('is-moving')).toBe(false);
});
it('swipes a submenu back and resizes between the three panel modes', () => {
  const f = fixture(); f.drawer.open(); vi.advanceTimersByTime(600); f.drawer.push('picture'); vi.advanceTimersByTime(600); f.drawer.push('codec'); vi.advanceTimersByTime(600);
  f.pointer('pointerdown', 0); f.pointer('pointermove', 250, 0, 200); vi.advanceTimersByTime(16); f.pointer('pointerup', 250, 0, 210); vi.advanceTimersByTime(600);
  expect(f.current()).toBe('picture'); expect(f.dialog.open).toBe(true);
  expect(f.panel.dataset.width).toBe('two-thirds'); f.win.innerWidth = 2000; f.windowEvents.emit('resize', {}); expect(f.panel.dataset.width).toBe('half');
  f.win.innerWidth = 430; f.windowEvents.emit('resize', {}); expect(f.panel.dataset.width).toBe('full');
});

it('synchronizes choice checks, disabled states, values and segmented selection', async () => {
  const { syncChoices } = await import('../receiver/drawer');
  const input = { id: 'frame-rate', value: 'FPS_45', disabled: true };
  const buttons = ['AUTO', 'FPS_45', 'FPS_60'].map(value => ({ dataset: { value }, disabled: false, textContent: value === 'AUTO' ? 'Automatic' : `${value.slice(4)} fps`, setAttribute: vi.fn() }));
  const value = { textContent: '' }, track = { style: { setProperty: vi.fn() } };
  const root = {
    querySelectorAll: (selector: string) => selector === 'input[type=hidden]' ? [input] : buttons,
    querySelector: (selector: string) => selector === '#frame-rate-segments' ? track : selector === '#frame-rate-value' ? value : null,
  };
  syncChoices(root as unknown as ParentNode);
  expect(buttons.map(button => button.disabled)).toEqual([true, true, true]);
  expect(buttons[1].setAttribute).toHaveBeenCalledWith('aria-pressed', 'true'); expect(value.textContent).toBe('45 fps');
  expect(track.style.setProperty).toHaveBeenCalledWith('--index', '1');
  input.disabled = false; input.value = 'AUTO'; syncChoices(root as unknown as ParentNode);
  expect(buttons.every(button => !button.disabled)).toBe(true); expect(value.textContent).toBe('Automatic');
});
