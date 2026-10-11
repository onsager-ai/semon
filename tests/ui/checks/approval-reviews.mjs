// Native role metadata controls visibility; names cannot grant review semantics.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { nativeFixture } from '../native-fixture.mjs';
import { logs } from '../native-order-logs.mjs';
import { closePage, overflow } from '../lib.mjs';

function prepare(root, now) {
  const files = logs(root);
  for (let n = 0; n < 65; n++) files.create('review-recent-' + n, now - 1000 - n * 100);
  const directory = path.join(root, 'codex/sessions/2026/09/24');
  fs.mkdirSync(directory, { recursive: true });
  for (const [id, parent, role, nickname] of [
    ['approval-root', null, 'user', null],
    ['approval-review', 'approval-root', 'guardian_review', 'reviewer'],
    ['approval-worker', 'approval-review', 'subagent', 'worker'],
    ['approval-name-only', 'approval-root', 'subagent', 'Approval review helper'],
  ]) {
    const at = now - 3600000;
    const record = (offset, type, payload) => ({
      timestamp: new Date(at + offset).toISOString(),
      type,
      payload,
    });
    fs.writeFileSync(
      path.join(directory, 'rollout-' + id + '.jsonl'),
      [
        record(0, 'session_meta', {
          id,
          cwd: '/work/approvals',
          originator: 'codex_exec',
          thread_source: role,
          ...(parent ? { parent_thread_id: parent } : {}),
          ...(nickname ? { agent_nickname: nickname, agent_path: 'root/' + nickname } : {}),
        }),
        record(1000, 'event_msg', { type: 'user_message', message: 'Coordinate ' + id }),
        record(2000, 'turn_context', { model: 'gpt-6-luna' }),
        record(3000, 'response_item', {
          type: 'message',
          role: 'assistant',
          content: [{ type: 'output_text', text: 'Completed ' + id }],
        }),
        record(4000, 'event_msg', {
          type: 'token_count',
          info: {
            total_token_usage: {
              input_tokens: 1000,
              cached_input_tokens: 200,
              output_tokens: 50,
              reasoning_output_tokens: 10,
              total_tokens: 1060,
            },
          },
        }),
      ]
        .map((value) => JSON.stringify(value))
        .join('\n') + '\n',
    );
  }
}

export default async function approvalReviews(browser) {
  const fixture = await nativeFixture({ prepare });
  try {
    const source = (await (await fixture.request('/api/session-capabilities')).json()).source_key;
    const query = new URLSearchParams({
      machine: source,
      scope: 'retained_history',
      q: 'approval-',
      limit: '100',
    });
    const catalog = await (await fixture.request('/api/sessions?' + query)).json();
    assert.equal(catalog.items.length, 4);
    const review = catalog.items.find((item) => item.key === 'approval-review');
    assert.equal(review.kind, 'codex-run');
    assert.equal(review.presentation_kind, 'Approval review');
    assert.notEqual(
      catalog.items.find((item) => item.key === 'approval-name-only').presentation_kind,
      'Approval review',
    );
    const costs = catalog.items.map((item) => [item.key, item.cost]);
    for (const width of [390, 820, 1280])
      for (const dark of [false, true]) {
        const page = await fixture.open(browser, {
          size: width === 390 ? 'phone' : 'desktop',
          dark,
          viewport: { width, height: 860 },
        });
        const retired = [];
        page.on('request', (request) => {
          if (/^\/api\/(model|tx|events)(?:\/|$)/.test(new URL(request.url()).pathname))
            retired.push(request.url());
        });
        try {
          await page.goto(fixture.base + '/sessions?' + query);
          const list = (key) =>
            page.locator('.session-list .nrow[data-id=' + JSON.stringify(key) + ']');
          await list('approval-root').waitFor();
          assert.equal(
            await list('approval-review').count(),
            0,
            'reviews are hidden by native kind',
          );
          assert.equal(
            await list('approval-name-only').count(),
            1,
            'ordinary sessions remain visible',
          );
          const toggle = page.locator('[data-show-approval-reviews]');
          await toggle.click();
          assert.equal(await toggle.getAttribute('aria-pressed'), 'true');
          await list('approval-review').waitFor();
          await toggle.click();
          await page.goto(
            fixture.base + '/s/codex/approval-root?' + new URLSearchParams({ machine: source }),
          );
          await page.locator('[data-entry-key]').first().waitFor({ state: 'attached' });
          if (width === 390) await page.locator('#topbar .lead').click();
          const tree = (key) =>
            page.locator(
              '#lanes .treeitem[data-id=' + JSON.stringify(JSON.stringify([source, key])) + ']',
            );
          await tree('approval-root').waitFor({ state: 'attached' });
          assert.equal(await tree('approval-review').count(), 0);
          const rootToggle = tree('approval-root').locator(':scope > .tree-row .tree-toggle');
          if ((await rootToggle.getAttribute('aria-expanded')) === 'false')
            await rootToggle.click();
          await tree('approval-worker').waitFor();
          assert.equal(
            await tree('approval-worker').evaluate(
              (node) => node.parentElement.closest('.treeitem')?.dataset.id,
            ),
            JSON.stringify([source, 'approval-root']),
            'a hidden review forwards descendants to its visible parent',
          );
          await page.goto(
            fixture.base + '/s/codex/approval-worker?' + new URLSearchParams({ machine: source }),
          );
          await page.locator('[data-entry-key]').first().waitFor({ state: 'attached' });
          if (width === 390) await page.locator('#topbar .lead').click();
          await tree('approval-review').waitFor();
          assert.equal(
            await tree('approval-worker')
              .locator(':scope > .tree-row .srow')
              .getAttribute('aria-current'),
            'page',
            'the selected review path survives outside the recent 60 rows',
          );
          await page.goto(
            fixture.base + '/s/codex/approval-review?' + new URLSearchParams({ machine: source }),
          );
          await page.locator('[data-entry-key]').first().waitFor({ state: 'attached' });
          assert.ok((await page.locator('#topbar').innerText()).includes('Approval review'));
          assert.equal(await overflow(page), 0);
          assert.deepEqual(retired, []);
          assert.deepEqual(page.errors, []);
        } finally {
          await closePage(page);
        }
      }
    const after = await (await fixture.request('/api/sessions?' + query)).json();
    assert.deepEqual(
      after.items.map((item) => [item.key, item.cost]),
      costs,
      'visibility never changes accounting',
    );
  } finally {
    await fixture.close();
  }
  console.log(
    'Native approval visibility, forwarding, selected paths and accounting passed at all six layouts',
  );
  return true;
}
