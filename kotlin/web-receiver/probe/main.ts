// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

import { collectReceiverInfo } from '../shared/receiver-info';
import './style.css';
const body = document.querySelector<HTMLTableSectionElement>('#results')!;
const button = document.querySelector<HTMLButtonElement>('#copy')!;
const status = document.querySelector<HTMLElement>('#status')!;
let report: Record<string, unknown> = {};
function row(label: string, value: unknown): void {
  const tr = document.createElement('tr'), th = document.createElement('th'), td = document.createElement('td');
  th.scope = 'row'; th.textContent = label;
  td.textContent = typeof value === 'string' ? value : JSON.stringify(value);
  tr.append(th, td); body.append(tr);
}
async function run(): Promise<void> {
  report = await collectReceiverInfo();
  for (const [key, value] of Object.entries(report)) row(key, value);
  button.disabled = false; status.textContent = 'Probe complete. Power efficiency is a browser estimate, not proof of the active decoder.';
}
button.addEventListener('click', async () => {
  try { await navigator.clipboard.writeText(JSON.stringify(report, null, 2)); status.textContent = 'JSON copied.'; }
  catch {
    const textarea = document.createElement('textarea'); textarea.value = JSON.stringify(report, null, 2);
    document.body.append(textarea); textarea.select();
    status.textContent = document.execCommand('copy') ? 'JSON copied.' : 'Select and copy the JSON below.';
    if (status.textContent === 'JSON copied.') textarea.remove();
  }
});
void run().catch(e => { status.textContent = String(e); });
