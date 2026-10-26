// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-15

import { drawerPages } from './receiver/drawer-core';
import { selectLanguage, type StatusState } from './receiver/ui';
export const previewStates: (StatusState | 'playingSoundBlocked')[] = ['connecting', 'waiting', 'playing', 'reconnecting', 'error', 'unsupported', 'soundBlocked', 'playingSoundBlocked', 'microphoneDenied'];
export const previewSizes = [[1920, 720], [1280, 720], [800, 1280]];
export function previewOptions(params: URLSearchParams) {
  const state = params.get('state');
  const sheet = params.get('sheet');
  const flag = (key: string) => params.has(key) && !['0', 'false', 'off'].includes(params.get(key)!.toLowerCase());
  const size = /^(\d{1,4})x(\d{1,4})$/i.exec(params.get('size') ?? '');
  const dimensions = size?.slice(1).map(Number);
  return {
    state: previewStates.find(value => value === state) ?? 'connecting',
    language: selectLanguage([params.get('lang') ?? 'en']),
    sheet: drawerPages.find(page => page === sheet) ?? (flag('sheet') ? 'root' : ''), fallback: flag('fallback'), large: flag('large'), motion: flag('motion'),
    sizes: dimensions?.every(value => value > 0 && value <= 8192) ? [dimensions] : previewSizes,
  };
}
