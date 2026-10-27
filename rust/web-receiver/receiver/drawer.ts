// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

import { DrawerStack, drawerWidth, drawerKey, horizontalPan, dragOffset, drawerRelease, type DrawerPage } from './drawer-core';
import { sheetKey } from './sheet';

export function syncChoices(root: ParentNode): void {
  for (const input of root.querySelectorAll<HTMLInputElement>('input[type=hidden]')) {
    const buttons = [...root.querySelectorAll<HTMLButtonElement>(`[data-choice="${input.id}"]`)];
    for (const button of buttons) {
      button.disabled = input.disabled; button.setAttribute('aria-pressed', String(button.dataset.value === input.value));
    }
    const selected = buttons.findIndex(button => button.dataset.value === input.value);
    const track = root.querySelector<HTMLElement>(`#${input.id}-segments`);
    track?.style.setProperty('--count', String(buttons.length)); track?.style.setProperty('--index', String(Math.max(selected, 0)));
    const value = root.querySelector(`#${input.id === 'relay-video-codec' ? 'codec' : input.id}-value`);
    if (value) value.textContent = buttons[selected]?.textContent ?? '';
  }
  const hint = root.querySelector<HTMLElement>('#native-hint');
  if (hint) hint.hidden = root.querySelector<HTMLInputElement>('#resolution')?.value !== 'native';
}

export function bindDrawer(dialog: HTMLDialogElement, reduced: () => boolean, visibility: (open: boolean) => void, beforeOpen: () => void = () => {}) {
  const doc = dialog.ownerDocument, win = doc.defaultView!;
  const panel = dialog.querySelector<HTMLElement>('.panel')!, scrim = dialog.querySelector<HTMLElement>('.scrim')!;
  const back = dialog.querySelector<HTMLButtonElement>('#drawer-back')!, closeButton = dialog.querySelector<HTMLButtonElement>('#close-settings')!;
  const pages = [...dialog.querySelectorAll<HTMLElement>('[data-drawer-page]')];
  const stack = new DrawerStack(), openers = new Map<DrawerPage, HTMLElement>(), tabStops = new Map<HTMLElement, string | null>();
  let opener: HTMLElement | null = null, keyboard = false, suppressClick = false;
  let phase: 'opening' | 'closing' | 'page' | 'settling' | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined, frame = 0, dragFrame = 0;
  let finish: (() => void) | undefined, target: HTMLElement | undefined;
  let drag: { id: number; x: number; y: number; width: number; rem: number; start: number; opacity: number; offset: number; lastX: number; at: number; velocity: number; active: boolean; opening: boolean } | undefined;
  const page = (name = stack.current) => pages.find(node => node.dataset.drawerPage === name)!;
  const controls = (root: ParentNode) => [...root.querySelectorAll<HTMLElement>('button, input, a[href]')].filter(node => !node.closest('[hidden]') && !node.matches(':disabled') && node.getClientRects().length > 0);
  const focus = (node?: HTMLElement) => (node ?? (keyboard ? controls(page())[0] : undefined) ?? page()).focus({ preventScroll: true });
  const resize = () => { panel.dataset.width = drawerWidth(win.innerWidth, win.innerHeight); };
  function states(): void {
    for (const node of pages) {
      const active = node === page();
      node.classList.toggle('active', active); node.classList.toggle('before', node.dataset.drawerPage === stack.previous);
      node.toggleAttribute('inert', !active); node.setAttribute('aria-hidden', String(!active));
      // Keep older Chromium versions without inert out of hidden pages as well.
      for (const control of node.querySelectorAll<HTMLElement>('button, input, a[href]')) {
        if (!active) { if (!tabStops.has(control)) tabStops.set(control, control.getAttribute('tabindex')); control.tabIndex = -1; }
        else if (tabStops.has(control)) {
          const value = tabStops.get(control); if (value === null) control.removeAttribute('tabindex'); else control.setAttribute('tabindex', value!);
          tabStops.delete(control);
        }
      }
    }
    back.disabled = stack.depth === 1; back.setAttribute('aria-hidden', String(back.disabled)); panel.classList.toggle('has-back', !back.disabled);
    for (const title of dialog.querySelectorAll<HTMLElement>('[data-title]')) {
      const active = title.dataset.title === stack.current;
      title.classList.toggle('active', active); title.setAttribute('aria-hidden', String(!active));
    }
  }
  function cancel(): void { clearTimeout(timer); win.cancelAnimationFrame(frame); finish = undefined; target = undefined; }
  function clean(): void {
    cancel(); phase = undefined;
    dialog.classList.remove('is-moving', 'is-opening', 'is-dragging', 'popping');
    panel.style.removeProperty('--duration'); panel.style.removeProperty('--curve'); panel.style.removeProperty('transform'); scrim.style.removeProperty('opacity');
    for (const node of pages) { node.classList.remove('participating'); node.style.removeProperty('transform'); node.style.removeProperty('opacity'); }
  }
  function motion(duration: number, node = panel, done: () => void = () => {}): void {
    clearTimeout(timer); dialog.classList.add('is-moving'); target = node;
    finish = () => { clean(); done(); };
    if (reduced()) finish(); else timer = setTimeout(() => finish?.(), duration + 80);
  }
  panel.addEventListener('transitionend', event => { if (event.target === target && event.propertyName === 'transform') finish?.(); });
  function stopDrag(): void {
    const old = drag; drag = undefined; win.cancelAnimationFrame(dragFrame); dragFrame = 0;
    if (old && panel.hasPointerCapture(old.id)) panel.releasePointerCapture(old.id);
  }
  function navigate(next?: DrawerPage, trigger?: HTMLElement, interactive = false): void {
    if (phase === 'closing' || phase === 'page' && next) return;
    const outgoing = page(), previous = stack.current;
    if (next ? !stack.push(next) : !stack.pop()) return;
    stopDrag(); cancel(); dialog.classList.remove('is-opening', 'is-dragging'); dialog.classList.add('is-open');
    panel.style.removeProperty('transform'); scrim.style.removeProperty('opacity');
    if (next && trigger) openers.set(next, trigger);
    const incoming = page(); phase = 'page';
    outgoing.classList.add('participating'); incoming.classList.add('participating'); dialog.classList.toggle('popping', !next);
    if (!interactive) { incoming.classList.toggle('before', !next); incoming.classList.remove('active'); void incoming.offsetWidth; }
    const run = () => {
      states(); for (const node of [outgoing, incoming]) { node.style.removeProperty('transform'); node.style.removeProperty('opacity'); }
      motion(next ? 400 : 360, outgoing); focus(next ? undefined : openers.get(previous));
    };
    if (interactive || reduced()) run(); else frame = win.requestAnimationFrame(run);
  }
  function close(duration = 280, momentum = false): void {
    if (!dialog.open || phase === 'closing') return;
    stopDrag(); cancel(); phase = 'closing';
    panel.style.setProperty('--duration', `${duration}ms`);
    if (momentum) panel.style.setProperty('--curve', 'cubic-bezier(.2,.6,.35,1)');
    dialog.classList.remove('is-open', 'is-dragging', 'is-opening'); dialog.classList.add('is-closing');
    panel.style.removeProperty('transform'); scrim.style.removeProperty('opacity');
    motion(duration, panel, () => {
      dialog.close(); dialog.classList.remove('is-closing'); stack.reset(); states(); visibility(false);
      if (opener?.isConnected && opener.getClientRects().length) opener.focus({ preventScroll: true });
    });
  }
  function open(): void {
    if (dialog.open && phase !== 'closing') return;
    cancel(); beforeOpen();
    if (!dialog.open) { opener = doc.activeElement as HTMLElement; stack.reset(); states(); resize(); dialog.showModal(); visibility(true); }
    phase = 'opening'; dialog.classList.remove('is-closing'); dialog.classList.add('is-opening', 'is-moving');
    panel.style.removeProperty('--duration'); panel.style.removeProperty('--curve');
    void panel.offsetWidth;
    const run = () => { dialog.classList.add('is-open'); motion(500, dialog.querySelector<HTMLElement>('.root-last') ?? panel); focus(); };
    if (reduced()) run(); else frame = win.requestAnimationFrame(run);
  }
  const pop = () => { if (stack.depth === 1) close(); else navigate(); };
  closeButton.onclick = () => close(); back.onclick = pop; scrim.onclick = () => close();
  dialog.addEventListener('cancel', event => { event.preventDefault(); pop(); });
  dialog.addEventListener('click', event => {
    if (suppressClick) { event.preventDefault(); event.stopImmediatePropagation(); }
  }, true);
  dialog.addEventListener('click', event => {
    const button = (event.target as HTMLElement).closest<HTMLElement>('[data-page]');
    if (button) navigate(button.dataset.page as DrawerPage, button);
    const choice = (event.target as HTMLElement).closest<HTMLButtonElement>('[data-choice]');
    if (!choice || choice.disabled) return;
    const input = dialog.querySelector<HTMLInputElement>(`#${choice.dataset.choice}`)!;
    if (input.disabled) return;
    input.value = choice.dataset.value!; input.dispatchEvent(new Event('change', { bubbles: true })); syncChoices(dialog);
  });
  doc.addEventListener('keydown', event => {
    keyboard = true;
    if (drawerKey(event.key) === 'toggle') { event.preventDefault(); if (dialog.open && phase !== 'closing') close(); else open(); return; }
    if (dialog.open) sheetKey(event, [...(back.disabled ? [] : [back]), closeButton, ...controls(page())], event.target as HTMLElement, pop);
  });
  doc.addEventListener('pointerdown', () => { keyboard = false; }, true);
  function paint(): void {
    dragFrame = 0; if (!drag || !drag.active && !drag.opening) return;
    dialog.classList.add('is-dragging');
    if (stack.depth === 1 || !drag.active) {
      panel.style.transform = `translate3d(${drag.offset}px,0,0)`;
      scrim.style.opacity = String(Math.max(0, Math.min(1, drag.opacity - (drag.offset - drag.start) / drag.width)));
    } else {
      page().style.transform = `translate3d(${drag.offset}px,0,0)`;
      page(stack.previous).style.transform = `translate3d(${(drag.offset - drag.width) * .25}px,0,0)`;
      page(stack.previous).style.opacity = String(Math.max(0, drag.offset / drag.width));
    }
  }
  function resume(): void {
    dialog.classList.remove('is-dragging'); panel.style.removeProperty('transform'); scrim.style.removeProperty('opacity');
    motion(440);
  }
  panel.addEventListener('pointerdown', event => {
    if (event.button !== 0 || event.isPrimary === false || drag || phase && phase !== 'opening') return;
    const opening = phase === 'opening', style = win.getComputedStyle(panel);
    const offset = opening && style.transform !== 'none' ? new DOMMatrixReadOnly(style.transform).m41 : 0;
    drag = { id: event.pointerId, x: event.clientX, y: event.clientY, width: panel.getBoundingClientRect().width,
      rem: parseFloat(win.getComputedStyle(doc.documentElement).fontSize), start: offset, offset, opacity: opening ? Number(win.getComputedStyle(scrim).opacity) : 1,
      lastX: event.clientX, at: event.timeStamp, velocity: 0, active: false, opening };
    if (opening) { cancel(); dialog.classList.add('is-open'); dragFrame = win.requestAnimationFrame(paint); }
  });
  panel.addEventListener('pointermove', event => {
    if (!drag || drag.id !== event.pointerId) return;
    const dx = event.clientX - drag.x, dy = event.clientY - drag.y;
    if (!drag.active) {
      if (Math.abs(dy) > 10 && !horizontalPan(dx, dy)) { const opening = drag.opening; stopDrag(); if (opening) resume(); return; }
      if (!horizontalPan(dx, dy)) return;
      drag.active = true; suppressClick = true; cancel(); phase = undefined;
      dialog.classList.remove('is-opening'); dialog.classList.add('is-moving', 'is-dragging');
      if (stack.previous) { drag.start = 0; panel.style.removeProperty('transform'); scrim.style.removeProperty('opacity'); page(stack.previous).classList.add('participating'); }
      panel.setPointerCapture(event.pointerId);
    }
    event.preventDefault();
    const elapsed = event.timeStamp - drag.at;
    if (elapsed > 0) drag.velocity = (event.clientX - drag.lastX) / elapsed;
    drag.lastX = event.clientX; drag.at = event.timeStamp; drag.offset = dragOffset(drag.start + dx, drag.width, drag.rem);
    if (!dragFrame) dragFrame = win.requestAnimationFrame(paint);
  });
  function release(event: PointerEvent, cancelled: boolean): void {
    if (!drag || drag.id !== event.pointerId) return;
    const gesture = drag; if (gesture.active) paint(); stopDrag();
    if (!gesture.active) { if (gesture.opening) resume(); return; }
    const decision = drawerRelease(gesture.offset, gesture.width, event.timeStamp - gesture.at > 100 ? 0 : gesture.velocity);
    dialog.classList.remove('is-dragging');
    if (!cancelled && decision.complete) { if (stack.depth === 1) close(decision.duration, true); else navigate(undefined, undefined, true); }
    else {
      phase = 'settling'; panel.style.setProperty('--duration', '320ms'); panel.style.removeProperty('transform'); scrim.style.removeProperty('opacity');
      for (const node of pages) { node.style.removeProperty('transform'); node.style.removeProperty('opacity'); }
      motion(320, stack.depth === 1 ? panel : page());
    }
    win.setTimeout(() => { suppressClick = false; }, 0);
  }
  for (const name of ['pointerup', 'pointercancel', 'lostpointercapture']) panel.addEventListener(name, event => release(event as PointerEvent, name !== 'pointerup'));
  win.addEventListener('resize', () => { if (drag) { stopDrag(); clean(); } resize(); });
  states(); resize();
  return { open, close, pop, push: (next: DrawerPage) => navigate(next) };
}
