// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

import { readFileSync } from 'node:fs';
import { gzipSync } from 'node:zlib';
import { defineConfig } from 'vitest/config';
// A relay release sets OXP_APP_VERSION so the page shows the version of the release it ships in.
const version = process.env.OXP_APP_VERSION
  || JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf8')).version;
export default defineConfig({
  define: { __APP_VERSION__: JSON.stringify(version) },
  base: '/',
  plugins: [{
    name: 'receiver-bundle-budget',
    generateBundle: { order: 'post', handler(_options, bundle) {
      // Count the page, shared code and media worker; exclude probe and audio worklet.
      const bytes = Object.entries(bundle).reduce((sum, [name, file]) => {
        if (name.includes('probe') || name.includes('audio-worklet')) return sum;
        return sum + gzipSync(file.type === 'chunk' ? file.code : file.source).byteLength;
      }, 0);
      if (bytes > 120 * 1024) this.error(`Receiver gzip budget exceeded: ${bytes} bytes`);
      this.info(`Receiver gzip: ${bytes} / ${120 * 1024} bytes (worklet excluded)`);
    } },
  }],
  worker: { format: 'es' },
  build: { outDir: 'dist', rollupOptions: { input: { receiver: 'receiver/index.html', probe: 'probe/index.html' } } },
  test: { environment: 'node', include: ['tests/**/*.test.ts'] },
});
