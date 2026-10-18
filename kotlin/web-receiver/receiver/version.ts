// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-15

// The commit identifies a page the head-unit browser kept from an older relay.
export const version = __APP_COMMIT__ ? `${__APP_VERSION__} (${__APP_COMMIT__})` : __APP_VERSION__;
export const build = __APP_BUILD__;

// Keep every target for this tab, including when a relay is rolled back to an older build.
export function shouldReload(receiverBuild: string | undefined, storage: () => Pick<Storage, 'getItem' | 'setItem'>, pageBuild = build): boolean {
  if (!receiverBuild || receiverBuild === pageBuild) return false;
  try {
    const session = storage(), key = `oxp-receiver-reloaded:${receiverBuild}`;
    if (session.getItem(key) !== null) return false;
    session.setItem(key, '1');
    return session.getItem(key) === '1';
  } catch { return false; }
}
