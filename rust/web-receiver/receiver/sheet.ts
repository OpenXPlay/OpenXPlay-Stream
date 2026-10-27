// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-15

/** Native modal semantics keep the page inert; this also supports head-unit D-pads. */
export function sheetKey(event: Pick<KeyboardEvent, 'key' | 'shiftKey' | 'preventDefault'>, controls: HTMLElement[], target: HTMLElement, close: () => void): void {
  if (['Escape', 'BrowserBack', 'GoBack', 'Backspace'].includes(event.key)) { event.preventDefault(); close(); return; }
  if (event.key === 'Enter' && target.matches('input[type=checkbox]')) { event.preventDefault(); target.click(); return; }
  const index = controls.indexOf(target);
  if (event.key === 'Tab') {
    if (index < 0 || (event.shiftKey && index === 0) || (!event.shiftKey && index === controls.length - 1)) {
      event.preventDefault(); controls[event.shiftKey ? controls.length - 1 : 0]?.focus();
    }
    return;
  }
  if (target.matches('select')) return;
  if (!['ArrowDown', 'ArrowUp', 'ArrowLeft', 'ArrowRight'].includes(event.key)) return;
  const group = target.closest('[role=group]');
  const horizontal = event.key === 'ArrowLeft' || event.key === 'ArrowRight';
  const candidates = horizontal && group ? controls.filter(control => group.contains(control)) : controls;
  const current = candidates.indexOf(target);
  if (current < 0) return;
  event.preventDefault();
  candidates[(current + (event.key === 'ArrowDown' || event.key === 'ArrowRight' ? 1 : candidates.length - 1)) % candidates.length]?.focus();
}
