// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-15

import { drawerKey } from './drawer-core';

/** Native modal semantics keep the page inert; this also supports head-unit D-pads. */
export function sheetKey(event: Pick<KeyboardEvent, 'key' | 'shiftKey' | 'preventDefault'>, controls: HTMLElement[], target: HTMLElement, close: () => void): void {
  if (drawerKey(event.key) === 'back') { event.preventDefault(); close(); return; }
  if (event.key === 'Enter' && target.matches('input[type=checkbox]')) { event.preventDefault(); target.click(); return; }
  const index = controls.indexOf(target);
  if (event.key === 'Tab') {
    event.preventDefault();
    const next = index < 0 ? event.shiftKey ? controls.length - 1 : 0 : (index + (event.shiftKey ? controls.length - 1 : 1)) % controls.length;
    controls[next]?.focus(); return;
  }
  if (!['ArrowDown', 'ArrowUp', 'ArrowLeft', 'ArrowRight'].includes(event.key)) return;
  const group = target.closest('[role=group]');
  const horizontal = event.key === 'ArrowLeft' || event.key === 'ArrowRight';
  const candidates = horizontal && group ? controls.filter(control => group.contains(control)) : controls;
  const current = candidates.indexOf(target);
  if (current < 0) { event.preventDefault(); candidates[0]?.focus(); return; }
  event.preventDefault();
  candidates[(current + (event.key === 'ArrowDown' || event.key === 'ArrowRight' ? 1 : candidates.length - 1)) % candidates.length]?.focus();
}
