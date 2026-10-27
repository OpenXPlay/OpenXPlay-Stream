// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

export const drawerPages = ['root', 'picture', 'resolution', 'codec', 'frame-rate', 'sound', 'plugins', 'about'] as const;
export type DrawerPage = typeof drawerPages[number];
export function drawerWidth(width: number, height: number): 'half' | 'two-thirds' | 'full' {
  return width < 600 || width <= height ? 'full' : width >= height * 2 ? 'half' : 'two-thirds';
}
export class DrawerStack {
  private path: DrawerPage[] = ['root'];
  get current(): DrawerPage { return this.path[this.path.length - 1]; }
  get previous(): DrawerPage | undefined { return this.path[this.path.length - 2]; }
  get depth(): number { return this.path.length; }
  push(next: DrawerPage): boolean {
    const allowed = this.current === 'root' ? ['picture', 'sound', 'plugins', 'about'] : this.current === 'picture' ? ['resolution', 'codec', 'frame-rate'] : [];
    if (!allowed.includes(next)) return false;
    this.path.push(next); return true;
  }
  pop(): boolean { if (this.depth < 2) return false; this.path.pop(); return true; }
  reset(): void { this.path = ['root']; }
}
export function drawerKey(key: string): 'toggle' | 'back' | undefined {
  if (['Settings', 'ContextMenu'].includes(key)) return 'toggle';
  if (['Escape', 'Backspace', 'GoBack', 'BrowserBack'].includes(key)) return 'back';
}
export function horizontalPan(dx: number, dy: number): boolean { return Math.abs(dx) > 10 && Math.abs(dx) > Math.abs(dy) * 1.5; }
export function dragOffset(offset: number, width: number, rem: number): number {
  return offset >= 0 ? Math.min(offset, width) : -Math.min(-offset * .15, 1.5 * rem);
}
export function drawerRelease(offset: number, width: number, velocity: number): { complete: boolean; duration: number } {
  const complete = offset > width * .3 || velocity > .45;
  const duration = Math.min(280, Math.max(160, (width - Math.max(0, offset)) / Math.max(1.2, velocity)));
  return { complete, duration: complete ? duration : 320 };
}
export function fallbackSettings(features?: readonly string[]): boolean { return features?.includes('host-ui') !== true; }
export function settingsAnchor(width: number, right: number, top: number, gap: number) {
  return { x: width - right, y: top, w: 0, h: 0, gap, side: 'right' as const, faded: true };
}
