// Fixture-backed native Copilot persistence, never a personal/native runtime home.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { start } from './copied-usage.mjs';
import { served, reporter } from '../lib.mjs';
const repo = fileURLToPath(new URL('../../../', import.meta.url));
export default async function copilotCheck(browser) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'semon-copilot-browser-'));
  const report = reporter('copilot');
  let server;
  const originals = new Map(),
    families = [];
  try {
    for (const version of ['1.0.90', '1.0.91'])
      for (const name of [
        'headless-tools.events.jsonl',
        'lifecycle/initial.events.jsonl',
        'boundaries/cancel.events.jsonl',
        'subagent/task.events.jsonl',
      ]) {
        const bytes = fs.readFileSync(
          path.join(repo, 'tests/fixtures/compatibility/copilot-' + version, name),
        );
        const header = JSON.parse(bytes.toString().split('\n')[0]);
        const id = header.data.sessionId;
        const file = path.join(root, 'copilot/session-state', id, 'events.jsonl');
        fs.mkdirSync(path.dirname(file), { recursive: true });
        fs.writeFileSync(file, bytes);
        originals.set(file, bytes);
        families.push({ version, name, id, file });
      }
    server = await start(root);
    const url = (route) =>
      server.base + route + (route.includes('?') ? '&' : '?') + 't=' + server.token;
    const model = await (await fetch(url('/api/model'))).json();
    assert.equal(
      Object.values(model.sessions).filter((s) => s.harness === 'copilot').length,
      families.length,
    );
    for (const fixture of families) {
      const session = model.sessions[fixture.id];
      assert.equal(session.copilot.version, fixture.version);
      assert.equal(session.cost.usd, null);
      assert.equal(session.copilot.logical_parent, null);
      assert.equal(session.copilot.approvals, null);
      const tx = await (await fetch(url('/api/tx?sid=' + fixture.id))).json();
      assert.ok(tx.entries.length);
      if (fixture.name === 'headless-tools.events.jsonl') {
        const calls = tx.entries.filter((e) => e.k === 'tool');
        assert.equal(calls.length, 2);
        assert.equal(calls.find((e) => e.native.tool_call_id === 'mock-call-1').ok, false);
        assert.equal(calls.find((e) => e.native.tool_call_id === 'mock-call-0').ok, true);
      }
      if (fixture.name === 'boundaries/cancel.events.jsonl')
        for (const call of tx.entries.filter((e) => e.k === 'tool')) {
          assert.equal(call.ok, null);
          assert.equal(call.unfinished, undefined);
        }
      for (const size of ['phone', 'desktop'])
        for (const dark of [false, true]) {
          const page = await served(browser, {
            ...server,
            size,
            dark,
            path: '/s/copilot/' + fixture.id,
          });
          try {
            await page.context().grantPermissions(['clipboard-read', 'clipboard-write'], {
              origin: server.base,
            });
            assert.ok(await page.locator("section[aria-label='Transcript']").count());
            await page.click('#more-btn');
            await page.waitForFunction(() => document.querySelector('dialog.session-menu')?.open);
            await page.getByRole('menuitem', { name: 'Copy resume command', exact: true }).click();
            await page.waitForFunction(
              async (expected) => (await navigator.clipboard.readText()) === expected,
              'copilot --resume=' + fixture.id,
            );
            assert.equal(
              await page.getByRole('menuitem', { name: 'Copied', exact: true }).count(),
              1,
            );
            assert.equal(await page.locator('dialog.session-menu .cost-big').textContent(), '—');
            const mark = page.locator('dialog.session-menu .hicon[data-harness="copilot"]');
            assert.equal(await mark.count(), 1);
            assert.equal(
              await mark.evaluate((el) => getComputedStyle(el).backgroundColor),
              'rgb(255, 255, 255)',
            );
            await page.waitForFunction(() =>
              [...document.querySelectorAll('dialog.session-menu .hicon img')].every(
                (img) => img.complete && img.naturalWidth > 0,
              ),
            );
            assert.equal(
              await mark
                .locator('img')
                .first()
                .evaluate((el) => getComputedStyle(el).filter),
              'none',
            );

            assert.match(
              await page.locator('dialog.session-menu .cost-note').first().textContent(),
              /Copilot usage is a cumulative saved snapshot.*unknown/,
            );
            assert.deepEqual(page.errors, []);
          } finally {
            await page.context().close();
          }
        }
    }
    // Discover an append on the running viewer, then compare a server restart.
    const resumed = families.find(
      (f) => f.version === '1.0.91' && f.name === 'lifecycle/initial.events.jsonl',
    );
    const bytes = fs.readFileSync(
      path.join(repo, 'tests/fixtures/compatibility/copilot-1.0.91/lifecycle/resumed.events.jsonl'),
    );
    const updated = async () => {
      const json = await (await fetch(url('/api/model'))).json();
      return json.sessions[resumed.id].copilot.usage?.tokens.input === 22;
    };
    fs.writeFileSync(resumed.file, bytes);
    originals.set(resumed.file, bytes);
    const deadline = Date.now() + 5000;
    while (!(await updated())) {
      assert.ok(Date.now() < deadline, 'live Copilot append did not arrive');
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    server.proc.kill();
    server = await start(root);
    assert.ok(await updated());
    for (const [file, bytes] of originals) assert.deepEqual(fs.readFileSync(file), bytes);
    report.results = {
      versions: 2,
      nativeFamilies: families.length,
      screens: families.length * 4,
      exactCalls: true,
      liveResumeInput: 22,
      unknownLineage: true,
      readOnly: true,
      nativeResumeClipboard: true,
    };
    return report.done();
  } finally {
    server?.proc.kill();
    fs.rmSync(root, { recursive: true, force: true });
  }
}
