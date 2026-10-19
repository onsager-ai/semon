import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { chromium } from '../../tests/ui/node_modules/playwright/index.mjs';
const { outputFiles } = await build({
  absWorkingDir: new URL('../', import.meta.url).pathname,
  stdin: {
    contents:
      "export {renderSessionScreen,updateSessionRuntime,updateSessionControl,updateSessionPager} from './src/lib/transcript';",
    resolveDir: new URL('../', import.meta.url).pathname,
  },
  bundle: true,
  write: false,
  format: 'iife',
  globalName: 'RuntimeLeaf',
  platform: 'browser',
});
for (const width of [390, 1280])
  test(`readonly runtime leaf survives missing native identity without walking content at ${width}px`, async () => {
    const browser = await chromium.launch();
    try {
      const page = await browser.newPage({ viewport: { width, height: 900 } });
      await page.route('http://runtime.test/**', (route) =>
        route.fulfill({
          contentType: 'text/html',
          body: '<!doctype html><header id="topbar"></header><div id="page"></div>',
        }),
      );
      await page.goto('http://runtime.test/');
      await page.addScriptTag({ content: outputFiles[0].text });
      await page.evaluate(() => {
        window.root = document.querySelector('#page');
        const blocks = [
          {
            kind: 'loose',
            key: 'source-entry',
            entries: [
              {
                kind: 'message',
                key: 'source-entry',
                entryKey: 'source-entry',
                flavor: 'assistant',
                text: 'Retained source text',
              },
            ],
          },
        ];
        window.control = {
          snapshot: {
            thread: 'native',
            generation: 'g',
            activeTurn: null,
            connected: true,
            capabilities: {
              input: true,
              steer: false,
              interrupt: false,
              commandApproval: false,
              fileApproval: false,
              questions: false,
            },
            reason: null,
            requests: [],
            actions: {},
          },
          busy: false,
          uncertain: false,
          note: '',
          send: async () => false,
          interrupt() {},
          answer() {},
          reconnect() {},
        };
        RuntimeLeaf.renderSessionScreen(
          root,
          {
            composerTarget: document.querySelector('#topbar'),
            id: 'catalog',
            name: 'Session',
            blocks,
            order: [],
            control,
            runtime: { observation: null, reason: null, delivery: 'updating' },
          },
          {
            committed() {},
            trace() {},
            session() {},
            machine() {},
            sender() {},
            toolAll() {},
            script() {},
            image() {},
            background() {},
            pager() {},
            jump() {},
          },
        );
        blocks.map = () => {
          throw Error('Runtime update walked transcript blocks');
        };
      });
      await page.getByRole('button', { name: 'Open Composer' }).click();
      const draft = page.locator('textarea');
      await draft.fill('Retained draft');
      await draft.focus();
      const input = await draft.elementHandle(),
        body = await page.locator('[data-entry-key="source-entry"]').elementHandle();
      await page.evaluate(() =>
        RuntimeLeaf.updateSessionRuntime(root, {
          observation: {
            state: 'ended',
            phase: 'released',
            freshness: 'current',
            presence: 'absent',
          },
          reason: null,
          delivery: 'current',
        }),
      );
      await page
        .getByRole('region', { name: 'Environment status' })
        .getByText('This session has ended.', { exact: true })
        .waitFor();
      assert.equal(await draft.inputValue(), 'Retained draft');
      assert.equal(await page.evaluate((node) => node === document.activeElement, input), true);
      assert.equal(
        await page.evaluate(
          (node) => node === document.querySelector('[data-entry-key="source-entry"]'),
          body,
        ),
        true,
      );
      await page.evaluate(() =>
        RuntimeLeaf.updateSessionPager(root, {
          sid: 'catalog',
          where: 'before',
          text: 'Load earlier records',
          disabled: false,
          busy: false,
        }),
      );
      const pager = page.getByRole('button', { name: 'Load earlier records', exact: true });
      await pager.waitFor();
      const pagerNode = await pager.elementHandle();
      await page.evaluate(() =>
        RuntimeLeaf.updateSessionPager(root, {
          sid: 'catalog',
          where: 'before',
          text: 'Loading earlier records',
          disabled: true,
          busy: true,
        }),
      );
      await page.getByRole('button', { name: 'Loading earlier records', exact: true }).waitFor();
      assert.equal(
        await page.evaluate(
          (node) => node === document.querySelector('[data-pager-sid="catalog"]'),
          pagerNode,
        ),
        true,
      );
      assert.equal(await page.evaluate((node) => node === document.activeElement, input), true);
      await page.evaluate(() =>
        RuntimeLeaf.updateSessionRuntime(root, {
          observation: {
            state: 'ended',
            phase: 'released',
            freshness: 'current',
            presence: 'absent',
          },
          reason: 'Observation temporarily failed.',
          delivery: 'stale',
        }),
      );
      await page.getByText('Last environment observation is stale.', { exact: true }).waitFor();
      await page
        .getByRole('region', { name: 'Environment status' })
        .getByText('This session has ended.', { exact: true })
        .waitFor();
      assert.equal(await draft.inputValue(), 'Retained draft');
      assert.equal(await page.locator('button[type="submit"]').isEnabled(), false);
    } finally {
      await browser.close();
    }
  });

for (const width of [390, 1280])
  test(`unlinked mirrored history has no runtime warning block at ${width}px`, async () => {
    const browser = await chromium.launch();
    try {
      const page = await browser.newPage({ viewport: { width, height: 900 } });
      await page.goto('about:blank');
      await page.addScriptTag({ content: outputFiles[0].text });
      await page.evaluate(() => {
        const root = document.createElement('div');
        document.body.append(root);
        RuntimeLeaf.renderSessionScreen(
          root,
          {
            id: 'mirrored',
            name: 'Mirrored history',
            order: [],
            blocks: [
              {
                kind: 'loose',
                key: 'body',
                entries: [
                  {
                    kind: 'message',
                    key: 'body',
                    entryKey: 'body',
                    flavor: 'assistant',
                    text: 'Readable history',
                  },
                ],
              },
            ],
            runtime: {
              observation: null,
              delivery: 'current',
              reason: 'This history is mirrored; no owned managed environment is linked.',
            },
          },
          {
            committed() {},
            trace() {},
            session() {},
            machine() {},
            sender() {},
            toolAll() {},
            script() {},
            image() {},
            background() {},
            pager() {},
            jump() {},
          },
        );
      });
      assert.equal(await page.getByRole('region', { name: 'Environment status' }).count(), 0);
      assert.equal(await page.getByText('Readable history', { exact: true }).count(), 1);
    } finally {
      await browser.close();
    }
  });
