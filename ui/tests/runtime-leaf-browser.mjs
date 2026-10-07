import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { chromium } from '../../tests/ui/node_modules/playwright/index.mjs';
const { outputFiles } = await build({
  absWorkingDir: new URL('../', import.meta.url).pathname,
  stdin: {
    contents:
      "export {renderSessionScreen,updateSessionRuntime,updateSessionControl} from './src/lib/transcript';",
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
        route.fulfill({ contentType: 'text/html', body: '<!doctype html><div id="page"></div>' }),
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
            connected: false,
            capabilities: {
              input: false,
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
      await page.getByText('This session has ended.', { exact: true }).waitFor();
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
      await page.getByText('This session has ended.', { exact: true }).waitFor();
      assert.equal(await draft.inputValue(), 'Retained draft');
      assert.equal(await page.locator('button[type="submit"]').isEnabled(), false);
    } finally {
      await browser.close();
    }
  });
