// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-15

import { parseGearPosition, selectLanguage, type StatusState } from './receiver/ui';
export const previewStates: (StatusState | 'playingSoundBlocked')[] = ['code', 'incorrect', 'connecting', 'waiting', 'playing', 'reconnecting', 'error', 'unsupported', 'soundBlocked', 'playingSoundBlocked', 'microphoneDenied'];
export const previewSizes = [[800, 480], [1024, 600], [1920, 720], [360, 800], [412, 915], [915, 412]];
export function previewOptions(params: URLSearchParams) {
  const state = params.get('state');
  const [side, y] = (params.get('gear') ?? '').split(':');
  const flag = (key: string) => params.has(key) && !['0', 'false', 'off'].includes(params.get(key)!.toLowerCase());
  const size = /^(\d{1,4})x(\d{1,4})$/i.exec(params.get('size') ?? '');
  const dimensions = size?.slice(1).map(Number);
  return {
    state: previewStates.find(value => value === state) ?? 'code',
    language: selectLanguage([params.get('lang') ?? 'en']),
    gear: parseGearPosition({ side, y: y?.trim() ? Number(y) : NaN }),
    sheet: flag('sheet'), large: flag('large'), motion: flag('motion'),
    sizes: dimensions?.every(value => value > 0 && value <= 8192) ? [dimensions] : previewSizes,
  };
}
