// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-25

import { DrawerStack, drawerKey, drawerRelease, drawerWidth, dragOffset, horizontalPan, type DrawerPage } from './drawer-core';
import { sheetKey } from './sheet';

export function syncChoices(root: ParentNode): void {
  for (const input of root.querySelectorAll<HTMLInputElement>('input[type=hidden]')) {
    const buttons = [...root.querySelectorAll<HTMLButtonElement>(`[data-choice="${input.id}"]`)];
    for (const button of buttons) { button.setAttribute('aria-pressed', String(button.dataset.value === input.value)); button.disabled = input.disabled; }
    const selected = buttons.findIndex(button => button.dataset.value === input.value);
    const track = root.querySelector<HTMLElement>(`#${input.id}-segments`);
    if (track) { track.style.setProperty('--count', String(buttons.length)); track.style.setProperty('--index', String(Math.max(0, selected))); }
    const value = root.querySelector<HTMLElement>(`#${input.id === 'relay-video-codec' ? 'codec' : input.id}-value`);
    if (value) value.textContent = buttons[selected]?.textContent ?? '';
  }
  const native = root.querySelector<HTMLElement>('#native-hint');
  if (native) native.hidden = root.querySelector<HTMLInputElement>('#resolution')?.value !== 'native';
}
export function bindChoices(root: HTMLElement): void {
  root.addEventListener('click', event => {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>('[data-choice]');
    if (!button || button.disabled) return;
    const input = root.querySelector<HTMLInputElement>(`#${button.dataset.choice}`)!;
    if (input.disabled) return;
    input.value = button.dataset.value!;
    input.dispatchEvent(new Event('change', { bubbles: true })); syncChoices(root);
  });
}

export function bindDrawer(dialog: HTMLDialogElement, reduced: () => boolean, visibility: (open: boolean) => void, beforeOpen: () => void = () => {}) {
  const doc = dialog.ownerDocument, win = doc.defaultView!;
  const panel = dialog.querySelector<HTMLElement>('.panel')!, scrim = dialog.querySelector<HTMLElement>('.scrim')!;
  const pages = [...dialog.querySelectorAll<HTMLElement>('[data-drawer-page]')];
  const titles = [...dialog.querySelectorAll<HTMLElement>('[data-title]')];
  const rootItems = [...dialog.querySelectorAll<HTMLElement>('.root-item')];
  const back = dialog.querySelector<HTMLButtonElement>('#drawer-back')!;
  const tabStops = new Map<HTMLElement, string | null>();
  const stack = new DrawerStack(), openers = new Map<DrawerPage, HTMLElement>();
  let opener: HTMLElement | null = null, closing = false, moving = false;
  let timer: ReturnType<typeof setTimeout> | undefined, frame = 0, gestureFrame = 0;
  let finishMotion: (() => void) | undefined, motionTarget: HTMLElement | undefined;
  let drag: { id: number; x: number; y: number; width: number; rem: number; opening: boolean; startOffset: number; startOpacity: number; offset: number; lastX: number; time: number; velocity: number; active: boolean } | undefined;
  let suppressClick = false, keyboard = false;
  // Focus a control only for keyboard and D-pad use; a touch-opened drawer focuses the page so no ring appears.
  doc.addEventListener('keydown', () => { keyboard = true; }, true);
  doc.addEventListener('pointerdown', () => { keyboard = false; }, true);
  const page = (name = stack.current) => pages.find(node => node.dataset.drawerPage === name)!;
  const focusables = (scope: ParentNode) => [...scope.querySelectorAll<HTMLElement>('button, input, a[href]')].filter(node => !node.closest('[hidden]') && !node.matches(':disabled') && node.getClientRects().length > 0);
  const focusPage = (target?: HTMLElement) => (target ?? (keyboard ? focusables(page())[0] : undefined) ?? page()).focus({ preventScroll: true });
  const resize = () => { panel.dataset.width = drawerWidth(win.innerWidth, win.innerHeight); };
  const hidden = (node: HTMLElement, value: boolean) => {
    node.toggleAttribute('inert', value); node.setAttribute('aria-hidden', String(value));
    // Chromium 94 predates inert; keep transitioning pages out of keyboard navigation too.
    if (node.hasAttribute('data-drawer-page')) for (const control of node.querySelectorAll<HTMLElement>('button, input, a[href]')) {
      if (value) { if (!tabStops.has(control)) tabStops.set(control, control.getAttribute('tabindex')); control.tabIndex = -1; }
      else if (tabStops.has(control)) {
        const original = tabStops.get(control); if (original === null) control.removeAttribute('tabindex'); else control.setAttribute('tabindex', original!);
        tabStops.delete(control);
      }
    }
  };
  function cancelMotion(): void {
    win.cancelAnimationFrame(frame); clearTimeout(timer); finishMotion = undefined; motionTarget = undefined;
  }
  function clean(): void {
    cancelMotion(); moving = false;
    dialog.classList.remove('is-moving', 'settling', 'is-dragging', 'is-opening');
    panel.style.removeProperty('--motion-duration');
    panel.style.removeProperty('--motion-curve');
    panel.style.removeProperty('transform'); scrim.style.removeProperty('opacity');
    for (const node of pages) {
      node.style.removeProperty('transform'); node.style.removeProperty('opacity'); node.classList.remove('participating');
      hidden(node, node !== page());
    }
  }
  function movement(duration: number, finish: () => void = () => {}, target = panel): void {
    clearTimeout(timer); moving = true; dialog.classList.add('is-moving');
    motionTarget = target; finishMotion = () => { clean(); finish(); };
    if (reduced()) { finishMotion(); return; }
    timer = setTimeout(() => finishMotion?.(), duration + 80);
  }
  panel.addEventListener('transitionend', event => {
    if (event.target === motionTarget && event.propertyName === 'transform') finishMotion?.();
  });
  function stopDrag(): boolean {
    const gesture = drag; drag = undefined; win.cancelAnimationFrame(gestureFrame);
    if (gesture && panel.hasPointerCapture(gesture.id)) panel.releasePointerCapture(gesture.id);
    dialog.classList.remove('is-dragging'); return gesture?.active ?? false;
  }
  function header(): void {
    panel.classList.toggle('has-back', stack.depth > 1); back.disabled = stack.depth === 1; back.setAttribute('aria-hidden', String(back.disabled));
    for (const title of titles) {
      const index = pages.findIndex(node => node.dataset.drawerPage === title.dataset.title), current = pages.indexOf(page());
      title.className = title.dataset.title === stack.current ? 'active' : index < current ? 'before' : 'after';
      hidden(title, title.dataset.title !== stack.current);
    }
  }
  function states(): void {
    for (const node of pages) {
      node.classList.toggle('active', node === page());
      node.classList.toggle('before', node.dataset.drawerPage === stack.previous);
      node.classList.toggle('after', node !== page() && node.dataset.drawerPage !== stack.previous);
      hidden(node, node !== page());
    }
    header();
  }
  function navigate(next: DrawerPage | undefined, trigger?: HTMLElement, interactive = false): void {
    const opening = dialog.classList.contains('is-opening');
    if (closing || moving && next && !interactive && !opening) return;
    const interrupted = moving && !opening;
    const outgoing = page(), previous = stack.current;
    if (next ? !stack.push(next) : !stack.pop()) return;
    cancelMotion();
    if (opening) {
      stopDrag(); dialog.classList.add('is-open'); dialog.classList.remove('is-opening');
      panel.style.removeProperty('transform'); scrim.style.removeProperty('opacity');
    }
    if (next && trigger) openers.set(next, trigger);
    const incoming = page();
    dialog.classList.toggle('popping', !next);
    incoming.classList.add('participating'); outgoing.classList.add('participating');
    if (!interactive && !interrupted) {
      incoming.classList.toggle('before', !next); incoming.classList.toggle('after', !!next); incoming.classList.remove('active');
      void incoming.offsetWidth;
    }
    const run = () => {
      states();
      for (const node of [incoming, outgoing]) { node.style.removeProperty('transform'); node.style.removeProperty('opacity'); }
      dialog.classList.remove('is-dragging');
      movement(interactive ? 280 : next ? 400 : 360, () => {}, outgoing);
      focusPage(next ? undefined : openers.get(previous));
    };
    if (interactive || interrupted || reduced()) run(); else { dialog.classList.add('is-moving'); frame = win.requestAnimationFrame(run); }
  }
  function close(duration = 280, momentum = false): void {
    if (!dialog.open || closing) return;
    win.cancelAnimationFrame(frame); stopDrag(); closing = true;
    if (momentum) { panel.style.setProperty('--motion-duration', `${duration}ms`); panel.style.setProperty('--motion-curve', 'cubic-bezier(.2,.6,.35,1)'); }
    dialog.classList.remove('is-dragging', 'is-open'); dialog.classList.add('is-closing');
    panel.style.removeProperty('transform'); scrim.style.removeProperty('opacity');
    movement(duration, () => {
      closing = false; dialog.close(); dialog.classList.remove('is-closing'); stack.close(); states(); visibility(false);
      if (opener?.isConnected && opener.getClientRects().length) opener.focus({ preventScroll: true });
    });
  }
  function open(): void {
    if (dialog.open && !closing) return;
    const reversing = closing;
    cancelMotion(); closing = false; moving = true; panel.style.removeProperty('--motion-duration'); panel.style.removeProperty('--motion-curve');
    beforeOpen();
    if (!reversing) { opener = doc.activeElement as HTMLElement; stack.close(); states(); resize(); dialog.showModal(); visibility(true); }
    dialog.classList.remove('is-closing'); dialog.classList.add('is-moving', 'is-opening');
    void panel.offsetWidth;
    const run = () => { dialog.classList.add('is-open'); movement(480, () => {}, rootItems[rootItems.length - 1] ?? panel); focusPage(); };
    if (reduced()) run(); else frame = win.requestAnimationFrame(run);
  }
  const pop = () => { const interactive = stopDrag(); if (stack.depth > 1) navigate(undefined, undefined, interactive); else close(); };
  dialog.querySelector<HTMLButtonElement>('#close-settings')!.onclick = () => close();
  back.onclick = pop; scrim.onclick = () => close();
  dialog.addEventListener('cancel', event => { event.preventDefault(); pop(); });
  dialog.addEventListener('click', event => {
    if (suppressClick) { event.preventDefault(); event.stopImmediatePropagation(); suppressClick = false; }
  }, true);
  dialog.addEventListener('click', event => {
    const button = (event.target as HTMLElement).closest<HTMLElement>('[data-page]');
    if (button) navigate(button.dataset.page as DrawerPage, button);
  });
  doc.addEventListener('keydown', event => {
    if (drawerKey(event.key) === 'toggle') { event.preventDefault(); if (dialog.open && !closing) close(); else open(); return; }
    if (!dialog.open) return;
    const controls = [...(back.disabled ? [] : [back]), dialog.querySelector<HTMLElement>('#close-settings')!, ...focusables(page())];
    sheetKey(event, controls, event.target as HTMLElement, pop);
  });
  panel.addEventListener('pointerdown', event => {
    const opening = dialog.classList.contains('is-opening');
    if (event.button !== 0 || !event.isPrimary || closing || moving && !opening || drag) return;
    const holdOpening = opening && stack.depth === 1;
    const transform = holdOpening ? win.getComputedStyle(panel).transform : 'none';
    const startOffset = transform && transform !== 'none' ? new DOMMatrixReadOnly(transform).m41 : 0;
    const startOpacity = holdOpening ? parseFloat(win.getComputedStyle(scrim).opacity) : 1;
    drag = { id: event.pointerId, x: event.clientX, y: event.clientY, width: panel.getBoundingClientRect().width,
      rem: parseFloat(win.getComputedStyle(doc.documentElement).fontSize), opening: holdOpening, startOffset, startOpacity, offset: startOffset, lastX: event.clientX, time: event.timeStamp, velocity: 0, active: false };
    if (holdOpening) { cancelMotion(); dialog.classList.add('is-open'); gestureFrame = win.requestAnimationFrame(paintDrag); }
  });
  function resumeOpening(): void {
    win.cancelAnimationFrame(gestureFrame); gestureFrame = 0; dialog.classList.remove('is-dragging');
    panel.style.removeProperty('transform'); scrim.style.removeProperty('opacity');
    movement(440);
  }
  function paintDrag(): void {
    gestureFrame = 0; if (!drag || !drag.active && !drag.opening) return;
    dialog.classList.add('is-dragging');
    const offset = drag.offset;
    if (stack.depth === 1) {
      panel.style.transform = `translate3d(${offset}px,0,0)`;
      scrim.style.opacity = String(Math.max(0, Math.min(1, drag.startOpacity - (Math.max(0, offset) - drag.startOffset) / drag.width)));
    }
    else {
      page().style.transform = `translate3d(${offset}px,0,0)`;
      page(stack.previous).style.transform = `translate3d(${-drag.width * .25 + offset * .25}px,0,0)`;
      page(stack.previous).style.opacity = String(Math.max(0, offset) / drag.width);
    }
  }
  panel.addEventListener('pointermove', event => {
    if (!drag || drag.id !== event.pointerId) return;
    const dx = event.clientX - drag.x, dy = event.clientY - drag.y;
    if (!drag.active) {
      if (Math.abs(dy) > 10 && !horizontalPan(dx, dy)) { const opening = drag.opening; drag = undefined; if (opening) resumeOpening(); return; }
      if (!horizontalPan(dx, dy)) return;
      if (dialog.classList.contains('is-opening')) {
        cancelMotion(); dialog.classList.remove('is-opening'); dialog.classList.add('is-open');
      }
      drag.active = true; suppressClick = true; panel.setPointerCapture(event.pointerId);
      dialog.classList.add('is-dragging', 'is-moving');
      if (stack.previous) page(stack.previous).classList.add('participating');
    }
    event.preventDefault();
    const elapsed = event.timeStamp - drag.time;
    if (elapsed > 0) drag.velocity = (event.clientX - drag.lastX) / elapsed;
    drag.lastX = event.clientX; drag.time = event.timeStamp;
    drag.offset = dragOffset(drag.startOffset + dx, drag.width, drag.rem);
    if (!gestureFrame) gestureFrame = win.requestAnimationFrame(paintDrag);
  });
  function release(event: PointerEvent, cancelled: boolean): void {
    if (!drag || drag.id !== event.pointerId) return;
    const gesture = drag;
    win.cancelAnimationFrame(gestureFrame); if (gesture.active) paintDrag(); drag = undefined;
    if (panel.hasPointerCapture(event.pointerId)) panel.releasePointerCapture(event.pointerId);
    if (!gesture.active) { if (gesture.opening) resumeOpening(); return; }
    const decision = drawerRelease(gesture.offset, gesture.width, event.timeStamp - gesture.time > 100 ? 0 : gesture.velocity);
    dialog.classList.remove('is-dragging');
    if (!cancelled && decision.complete) {
      if (stack.depth === 1) close(decision.duration, true);
      else { panel.style.setProperty('--motion-duration', '280ms'); navigate(undefined, undefined, true); }
    } else {
      dialog.classList.add('settling'); panel.style.setProperty('--motion-duration', '320ms');
      panel.style.removeProperty('transform'); scrim.style.removeProperty('opacity');
      for (const node of pages) { node.style.removeProperty('transform'); node.style.removeProperty('opacity'); }
      movement(320, () => {}, stack.depth === 1 ? panel : page());
    }
    win.setTimeout(() => { suppressClick = false; }, 0);
  }
  panel.addEventListener('pointerup', event => release(event, false));
  panel.addEventListener('pointercancel', event => release(event, true));
  panel.addEventListener('lostpointercapture', event => release(event, true));
  win.addEventListener('resize', () => { if (drag) { stopDrag(); clean(); } resize(); });
  states(); resize(); bindChoices(dialog);
  return { open, close, push: (next: DrawerPage) => navigate(next), pop };
}
