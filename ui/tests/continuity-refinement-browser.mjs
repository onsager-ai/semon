import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { readFile, mkdir } from 'node:fs/promises';
import { chromium } from '../../tests/ui/node_modules/playwright/index.mjs';
import { auditText } from '../../tests/ui/text-audit.mjs';
const { outputFiles } = await build({
  absWorkingDir: new URL('../', import.meta.url).pathname,
  stdin: {
    contents:
      "export {renderCatalogList} from './src/lib/catalog'; export {renderSessionScreen} from './src/lib/transcript'; export {releaseScreen} from './src/lib/screens';",
    resolveDir: new URL('../', import.meta.url).pathname,
  },
  bundle: true,
  write: false,
  format: 'iife',
  globalName: 'Refinement',
  platform: 'browser',
});
const html = (
  await readFile(new URL('../../crates/semon-sessions/src/viewer.html', import.meta.url), 'utf8')
).replace('<script src="/viewer.js" defer></script>', '');
const css = await readFile(
  new URL('../../crates/semon-sessions/src/viewer.css', import.meta.url),
  'utf8',
);
const before = process.env.SEMON_REFINEMENT_BEFORE === '1';
const out = process.env.SEMON_REFINEMENT_OUT;
for (const width of [390, 820, 1280])
  for (const colorScheme of ['light', 'dark'])
    test(`source recovery and complete linked context at ${width}px ${colorScheme}`, async () => {
      const browser = await chromium.launch();
      try {
        const page = await browser.newPage({
          viewport: { width, height: 860 },
          colorScheme,
          hasTouch: width === 390,
        });
        await page.route('http://refinement.test/**', async (route) => {
          const path = new URL(route.request().url()).pathname;
          if (path.startsWith('/fonts/'))
            return route.fulfill({
              contentType: 'font/woff2',
              body: await readFile(
                new URL(
                  '../../crates/semon-sessions/src' +
                    path.replace(/-(latin(?:-ext)?)\.woff2$/, '/$1.woff2'),
                  import.meta.url,
                ),
              ),
            });
          return route.fulfill({
            contentType: path === '/viewer.css' ? 'text/css' : 'text/html',
            body: path === '/viewer.css' ? css : html,
          });
        });
        await page.goto('http://refinement.test/sessions');
        await page.addScriptTag({ content: outputFiles[0].text });
        await page.evaluate(() => {
          window.root = document.querySelector('#page');
          window.snapshot = {
            items: [],
            sourceLabel: 'Harbor fixture source',
            updating: false,
            note: '',
            harness: '',
            repo: '',
            query: '',
            metadataSearch: true,
            more: false,
            compatibilityHref: '/sessions?compat=1',
          };
          window.actions = [];
          window.host = {
            session(item) {
              actions.push(['session', item.key]);
            },
            filters(values) {
              actions.push(['filters', values]);
            },
            clearFilters() {
              snapshot.query = snapshot.harness = snapshot.repo = '';
              actions.push(['clear']);
              Refinement.renderCatalogList(root, snapshot, host);
              root.querySelector('input').focus();
            },
            more() {
              actions.push(['more']);
            },
            retry() {
              actions.push(['retry']);
              snapshot.items = [
                {
                  key: 'harbor',
                  name: 'Harbor retry investigation',
                  harness: 'claude',
                  repo: '/workspace/harbor',
                  branch: 'main',
                  model: 'Harness default',
                  freshness: { state: 'cached' },
                },
              ];
              Refinement.renderCatalogList(root, snapshot, host);
            },
          };
          Refinement.renderCatalogList(root, snapshot, host);
        });
        await page.evaluate(() => document.fonts.ready);
        async function capture(state) {
          if (!out) return;
          await mkdir(out, { recursive: true });
          await page.screenshot({ path: `${out}/${state}-${width}-${colorScheme}.png` });
        }
        await capture('empty-source');
        if (before)
          assert.equal(
            await page
              .getByText('No recorded sessions match these filters.', { exact: true })
              .count(),
            1,
          );
        else {
          await page.getByText(/No recorded sessions in this source yet\./).waitFor();
          assert.equal(
            await page.getByRole('button', { name: 'Clear filters', exact: true }).count(),
            0,
          );
          const refresh = page.getByRole('button', { name: 'Refresh history', exact: true });
          await refresh.focus();
          await refresh.press('Enter');
          await page.getByRole('button', { name: /Harbor retry investigation/ }).waitFor();
          assert.deepEqual(await page.evaluate(() => actions), [['retry']]);
        }
        await page.evaluate(() => {
          snapshot.items = [];
          snapshot.query = 'unmatched';
          Refinement.renderCatalogList(root, snapshot, host);
        });
        await page
          .getByText('No recorded sessions match these filters.', { exact: true })
          .waitFor();
        await page.getByRole('button', { name: 'Clear filters', exact: true }).click();
        assert.deepEqual(
          await page.evaluate(() => [snapshot.query, snapshot.harness, snapshot.repo]),
          ['', '', ''],
        );
        assert.equal(
          await page
            .getByRole('textbox', { name: 'Search session details' })
            .evaluate((node) => document.activeElement === node),
          true,
        );
        for (const state of ['updating', 'discovering', 'searchIndexIncomplete', 'searchPartial']) {
          await page.evaluate((key) => {
            snapshot[key] = true;
            Refinement.renderCatalogList(root, snapshot, host);
          }, state);
          assert.equal(await page.getByText(/No recorded sessions/).count(), 0);
          assert.equal(
            await page.getByRole('button', { name: 'Refresh history', exact: true }).count(),
            0,
          );
          await page.evaluate((key) => {
            snapshot[key] = false;
          }, state);
        }
        await page.evaluate(() => {
          host.retry();
          snapshot.note = 'History observation failed.';
          Refinement.renderCatalogList(root, snapshot, host);
        });
        await page.getByRole('button', { name: /Harbor retry investigation/ }).waitFor();
        await page.getByRole('button', { name: 'Retry', exact: true }).waitFor();
        assert.equal(await page.getByText(/No recorded sessions/).count(), 0);
        await capture('retained-error');
        await page.evaluate(() => {
          window.linked = {
            kind: 'child',
            key: 'linked-harbor',
            entryKey: 'linked-harbor',
            handoff: 'handoff-harbor',
            id: 'child-harbor',
            name: 'Harbor linked session with a long investigation name and complete retained context',
            state: 'err',
            stateLabel: 'Failed',
            meta: 'Claude Code · /workspace/harbor/services/very-long-repository-name · feature/retry-backoff-investigation · Run harbor-2026-10-09',
            brief:
              'Investigate retry backoff while preserving complete original task text. '.repeat(5),
            result: 'Recorded failure: the retry limit was reached. '.repeat(5),
            failed: true,
            trace: 'harbor-run',
            traceLabel: 'Open Harbor run',
            chevron: 'm9 5 7 7-7 7',
          };
          window.transcript = {
            id: 'harbor',
            name: 'Harbor investigation',
            blocks: [{ kind: 'loose', key: 'context', entries: [linked] }],
            order: [],
          };
          window.transcriptHost = {
            committed() {},
            trace(id) {
              actions.push(['trace', id]);
            },
            session(id) {
              actions.push(['open', id]);
            },
            machine() {},
            sender() {},
            toolAll() {},
            script() {},
            image() {},
            background() {},
            pager() {},
            jump() {},
          };
          Refinement.renderSessionScreen(root, transcript, transcriptHost);
        });
        const toggle = page.getByRole('button', { name: /^Expand .* details$/ });
        assert.equal(await toggle.getAttribute('aria-expanded'), 'false');
        assert.equal(await page.locator('.cc-details').isVisible(), false);
        await page.getByText('Failed', { exact: true }).waitFor();
        await capture('linked-collapsed');
        const priorActions = await page.evaluate(() => actions.length);
        await toggle.focus();
        await toggle.press('Space');
        assert.equal(
          await page.evaluate(() => actions.length),
          priorActions,
          'Disclosure must not navigate',
        );
        assert.equal(await page.locator('.cc-toggle').getAttribute('aria-expanded'), 'true');
        assert.equal(
          await page.locator('.cc-toggle').evaluate((node) => document.activeElement === node),
          true,
        );
        await capture('linked-expanded');
        if (before)
          assert.equal(
            await page.locator('.cc-details').getByText('Claude Code', { exact: false }).count(),
            0,
          );
        else {
          assert.equal(
            await page.locator('.cc-context-name').textContent(),
            await page.evaluate(() => linked.name),
          );
          assert.equal(
            (await page.locator('.cc-context-meta').textContent()).replace(/\s+/g, ' '),
            await page.evaluate(() => linked.meta),
          );
          for (const selector of [
            '.cc-context-name',
            '.cc-context-meta',
            '.cc-brief',
            '.cc-result',
          ]) {
            assert.equal(
              await page
                .locator(selector)
                .evaluate(
                  (node) =>
                    node.scrollWidth <= node.clientWidth + 1 &&
                    getComputedStyle(node).whiteSpace !== 'nowrap',
                ),
              true,
              selector + ' must wrap fully',
            );
          }
          const audit = await auditText(page);
          assert.equal(audit.smallCount, 0, JSON.stringify(audit.small));
          assert.equal(audit.lowCount, 0, JSON.stringify(audit.low));
        }
        await page.evaluate(() => {
          transcript.blocks = [
            {
              kind: 'loose',
              key: 'context',
              entries: [{ ...linked, result: linked.result + ' Current observation retained.' }],
            },
          ];
          Refinement.renderSessionScreen(root, transcript, transcriptHost);
        });
        assert.equal(
          await page.locator('.cc-toggle').getAttribute('aria-expanded'),
          'true',
          'Keyed expansion survives an accepted update',
        );
        await page.getByRole('button', { name: 'Open Harbor run', exact: true }).click();
        await page.getByRole('button', { name: /^Open Harbor linked session/ }).click();
        assert.deepEqual((await page.evaluate(() => actions)).slice(-2), [
          ['trace', 'harbor-run'],
          ['open', 'child-harbor'],
        ]);
        if (width === 390) {
          assert.ok((await page.locator('.cc-toggle').boundingBox()).height >= 44);
          assert.ok((await page.locator('.cc-name').boundingBox()).height >= 44);
        }
        assert.equal(
          await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth),
          true,
        );
        await page.evaluate(() => {
          window.stale = document.querySelector('.cc-toggle');
          Refinement.releaseScreen(root);
          stale.click();
        });
      } finally {
        await browser.close();
      }
    });
