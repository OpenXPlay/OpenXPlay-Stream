// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-25

export const drawerPages = ['root', 'picture', 'resolution', 'frame-rate', 'codec', 'sound', 'plugins', 'about'] as const;
export type DrawerPage = typeof drawerPages[number];
export function drawerWidth(width: number, height: number): 'half' | 'two-thirds' | 'full' {
  if (width <= height || width < 600) return 'full';
  return width / height >= 2 ? 'half' : 'two-thirds';
}
const menus: DrawerPage[] = ['picture', 'sound', 'plugins', 'about'];
const choices: DrawerPage[] = ['resolution', 'frame-rate', 'codec'];
export class DrawerStack {
  private pages: DrawerPage[] = ['root'];
  get current(): DrawerPage { return this.pages[this.pages.length - 1]; }
  get previous(): DrawerPage | undefined { return this.pages[this.pages.length - 2]; }
  get depth(): number { return this.pages.length; }
  push(page: DrawerPage): boolean {
    if (!(this.depth === 1 && menus.includes(page) || this.current === 'picture' && choices.includes(page))) return false;
    this.pages.push(page); return true;
  }
  pop(): boolean { if (this.depth === 1) return false; this.pages.pop(); return true; }
  close(): void { this.pages = ['root']; }
}
export function drawerKey(key: string): 'toggle' | 'back' | undefined {
  if (key === 'ContextMenu' || key === 'Settings') return 'toggle';
  if (['Escape', 'BrowserBack', 'GoBack', 'Backspace'].includes(key)) return 'back';
}
export function horizontalPan(dx: number, dy: number): boolean { return Math.abs(dx) > 10 && Math.abs(dx) > 1.5 * Math.abs(dy); }
export function dragOffset(dx: number, width: number, rem: number): number {
  return dx < 0 ? -Math.min(-dx * .15, rem * 1.5) : Math.min(dx, width);
}
export function drawerRelease(offset: number, width: number, velocity: number): { complete: boolean; duration: number } {
  const complete = offset > width * .3 || velocity > .45;
  return { complete, duration: complete ? Math.max(160, Math.min(280, (width - Math.max(0, offset)) / Math.max(velocity, 1.2))) : 320 };
}
export function fallbackSettings(features?: readonly string[]): boolean { return !features?.includes('host-ui'); }
export function settingsAnchor(width: number, edgeRight: number, edgeTop: number, gap: number) {
  return { x: width - edgeRight, y: edgeTop, w: 0, h: 0, side: 'right' as const, faded: true, gap };
}
