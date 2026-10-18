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
export function bindSheet(sheet: HTMLDialogElement, opener: HTMLElement, closeButton: HTMLElement, backdrop: HTMLElement, reduced: () => boolean): { open(): void; close(): void } {
  let timer: ReturnType<typeof setTimeout>;
  let start: { x: number; y: number; id: number } | undefined;
  const controls = () => [...sheet.querySelectorAll<HTMLElement>('button, input, select, a[href]')].filter(control => !control.closest('[hidden]') && !control.matches(':disabled') && control.getClientRects().length > 0);
  const close = () => {
    if (!sheet.open || sheet.classList.contains('closing')) return;
    sheet.classList.add('closing'); backdrop.classList.remove('open');
    const finish = () => { sheet.close(); sheet.classList.remove('closing'); backdrop.hidden = true; opener.focus(); };
    if (reduced()) finish(); else timer = setTimeout(finish, 300);
  };
  const open = () => {
    clearTimeout(timer); sheet.classList.remove('closing'); backdrop.hidden = false;
    backdrop.classList.add('open'); sheet.showModal(); controls()[0]?.focus();
  };
  closeButton.onclick = close;
  sheet.addEventListener('cancel', event => { event.preventDefault(); close(); });
  sheet.addEventListener('keydown', event => sheetKey(event, controls(), event.target as HTMLElement, close));
  sheet.addEventListener('click', event => { if (event.target === sheet && (event.clientX < sheet.getBoundingClientRect().left || event.clientY < sheet.getBoundingClientRect().top)) close(); });
  // Start swipes on the header/handle only so scrolling the sheet stays native.
  const handle = sheet.querySelector<HTMLElement>('header')!;
  handle.addEventListener('pointerdown', event => {
    if (event.pointerType !== 'touch' || (event.target as HTMLElement).closest('button')) return;
    start = { x: event.clientX, y: event.clientY, id: event.pointerId }; handle.setPointerCapture(event.pointerId);
  });
  handle.addEventListener('pointerup', event => {
    if (start?.id === event.pointerId && event.clientY - start.y > 56 && Math.abs(event.clientX - start.x) < (event.clientY - start.y) / 2) close();
    start = undefined;
  });
  handle.addEventListener('pointercancel', () => { start = undefined; });
  return { open, close };
}
