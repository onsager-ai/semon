import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile, mkdir } from 'node:fs/promises';
import { build } from 'esbuild';
import { chromium } from '../../tests/ui/node_modules/playwright/index.mjs';
import { auditText } from '../../tests/ui/text-audit.mjs';
const { outputFiles } = await build({
  absWorkingDir: new URL('../', import.meta.url).pathname,
  stdin: {
    contents:
      "export {renderSessionScreen,updateSessionControl,updateSessionRuntime,updateSessionJump,updateSessionName} from './src/lib/transcript'; export {createLocalControl} from './src/app/localControl'; export {EffectScope} from './src/app/effects';",
    resolveDir: new URL('../', import.meta.url).pathname,
  },
  bundle: true,
  write: false,
  format: 'iife',
  globalName: 'ViewerFixes',
});
const html = (
  await readFile(new URL('../../crates/semon-sessions/src/viewer.html', import.meta.url), 'utf8')
)
  .replace('<script src="/viewer.js" defer></script>', '')
  .replace('<link rel="stylesheet" href="/shell.css">', '');
const css = (
  await Promise.all(
    ['viewer', 'shell'].map((name) =>
      readFile(new URL(`../../crates/semon-sessions/src/${name}.css`, import.meta.url), 'utf8'),
    ),
  )
).join('\n');
const out = new URL('../../tests/ui/out-staging-viewer/', import.meta.url);
await mkdir(out, { recursive: true });
for (const width of [390, 820, 1280])
  for (const colorScheme of ['light', 'dark'])
    test(`approved reading and availability states ${width} ${colorScheme}`, async () => {
      const browser = await chromium.launch();
      try {
        const page = await browser.newPage({
          viewport: { width, height: 860 },
          colorScheme,
          hasTouch: width === 390,
        });
        await page.route('http://viewer-fixes.test/**', async (route) => {
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
        await page.goto('http://viewer-fixes.test/s/session');
        await page.addScriptTag({ content: outputFiles[0].text });
        await page.evaluate(() => {
          window.root = document.querySelector('#page');
          window.host = {
            committed() {},
            jump() {
              window.jumps++;
            },
            pager() {},
          };
          window.jumps = 0;
          window.scope = new ViewerFixes.EffectScope();
          window.control = ViewerFixes.createLocalControl(scope, () =>
            ViewerFixes.updateSessionControl(
              root,
              control.view('native', 'source:native') ??
                control.readOnly('native', 'source:native'),
            ),
          );
          window.native = {
            thread: 'native',
            generation: 'g',
            activeTurn: null,
            connected: true,
            capabilities: {
              input: true,
              steer: true,
              interrupt: false,
              commandApproval: false,
              fileApproval: false,
              questions: false,
            },
            reason: null,
            requests: [],
            actions: {},
          };
          window.snapshot = {
            id: 'native',
            name: 'Initial session',
            order: [],
            control: control.readOnly('native', 'source:native'),
            blocks: [
              {
                kind: 'loose',
                key: 'recorded',
                entries: [
                  {
                    kind: 'thought',
                    key: 'thought',
                    entryKey: 'source:native:slot:1',
                    mode: 'readable',
                    label: 'Thinking',
                    text: 'Recorded thinking details.',
                  },
                  { kind: 'thought', key: 'pending', mode: 'pending' },
                  {
                    kind: 'thought',
                    key: 'masked',
                    mode: 'masked',
                    label: 'Thinking hidden by the harness',
                  },
                  {
                    kind: 'group',
                    key: 'group',
                    lone: false,
                    background: false,
                    failed: 1,
                    stack: 'M4 5h16v14H4z',
                    chevron: 'm9 5 7 7-7 7',
                    summary: [
                      {
                        text: 'Ran 3 commands, read 4 files, edited 3 files, searched 2 times, GitHub · fetch pull request 1 time',
                      },
                    ],
                    entries: [
                      {
                        kind: 'tool',
                        key: 'failed-tool',
                        step: {
                          className: 'step err',
                          running: false,
                          background: false,
                          waiting: false,
                          verb: 'GitHub · fetch pull request',
                          label: 'A recorded request',
                          named: false,
                          status: 'exit 1',
                          icon: 'M4 5h16v14H4z',
                          chevron: 'm9 5 7 7-7 7',
                          data: {
                            name: 'mcp__codex_apps__github_fetch_pull_request',
                            in: '{"number":123}',
                            out: 'Full recorded failure information',
                            ok: false,
                          },
                        },
                      },
                    ],
                  },
                  ...Array.from({ length: 16 }, (_, i) => ({
                    kind: 'message',
                    key: 'message-' + i,
                    entryKey: 'message-' + i,
                    flavor: 'assistant',
                    text:
                      'Recorded output ' +
                      i +
                      '\n\n' +
                      'Readable conversation context. '.repeat(10),
                  })),
                ],
              },
            ],
          };
          ViewerFixes.renderSessionScreen(root, snapshot, host);
          window.target = document.createElement('div');
          target.className = 'jump-wrap';
          ViewerFixes.updateSessionJump(root, true, 2, false, target);
        });
        const row = page.locator('.composer-unavailable'),
          input = page.getByLabel('Message to Codex');
        assert.equal(await row.isVisible(), true);
        assert.equal(await input.isVisible(), false);
        await row.press('Enter');
        assert.equal(await row.getAttribute('aria-expanded'), 'true');
        assert.equal(await input.getAttribute('readonly'), '');
        await row.click();
        assert.equal(await input.isVisible(), false);
        assert.equal(
          await page.locator('.think-disclosure').getAttribute('aria-expanded'),
          'false',
        );
        assert.equal(await page.locator('.think-text').count(), 0);
        await page.locator('.think-disclosure').press('Enter');
        assert.equal(await page.locator('.think-text').innerText(), 'Recorded thinking details.');
        await page.evaluate(() => {
          snapshot = {
            ...snapshot,
            name: 'Meaningful late name',
            blocks: [
              {
                kind: 'loose',
                key: 'older',
                entries: [{ kind: 'message', key: 'older', flavor: 'user', text: 'Earlier page' }],
              },
              ...snapshot.blocks,
            ],
          };
          ViewerFixes.renderSessionScreen(root, snapshot, host);
        });
        assert.equal(await page.locator('.think-disclosure').getAttribute('aria-expanded'), 'true');
        const summary = await page.locator('.tsum .tt').evaluate((node) => ({
          single: getComputedStyle(node).whiteSpace === 'nowrap',
          ellipsis: getComputedStyle(node).textOverflow === 'ellipsis',
          full: node.textContent,
          rect: node.getBoundingClientRect().height,
        }));
        assert.equal(summary.single, true);
        assert.equal(summary.ellipsis, true);
        assert.match(summary.full, /read 4 files, edited 3 files/);
        assert.ok(summary.rect <= 24);
        assert.equal(await page.locator('.tsum .tf').isVisible(), true);
        await page.locator('.tsum').click();
        await page.locator('.step > button').click();
        assert.match(await page.locator('.out').innerText(), /Full recorded failure information/);
        await page.evaluate(() => {
          control.adopt(native);
          ViewerFixes.updateSessionControl(root, control.view('native', 'source:native'));
        });
        const compact = await input.boundingBox();
        await input.focus();
        await input.fill('Retained draft\nSecond line\nThird line');
        await page.waitForTimeout(50);
        const expanded = await input.boundingBox();
        assert.ok(expanded.height > compact.height);
        await page.evaluate(() => {
          window.savedBody = document.querySelector('.turns');
          window.savedInput = document.querySelector('textarea');
          window.savedScroll = document.querySelector('#main').scrollTop;
          ViewerFixes.updateSessionName(root, 'Renamed without transcript change');
        });
        assert.equal(
          await page.evaluate(
            () =>
              document.querySelector('.turns') === savedBody &&
              document.querySelector('textarea') === savedInput,
          ),
          true,
        );
        assert.equal(await input.inputValue(), 'Retained draft\nSecond line\nThird line');
        await page.evaluate(() => {
          control.adopt({
            ...native,
            connected: false,
            capabilities: { ...native.capabilities, input: false, steer: false },
            reason: 'The controller is disconnected.',
          });
          ViewerFixes.updateSessionControl(root, control.view('native', 'source:native'));
        });
        assert.equal(await input.isVisible(), true);
        assert.equal(await input.getAttribute('readonly'), '');
        await page.locator('#page h1').evaluate((node) => {
          node.tabIndex = -1;
          node.focus();
        });
        await page.waitForTimeout(40);
        assert.equal(await input.isVisible(), false);
        assert.match(await row.innerText(), /controller is disconnected.*Draft saved/);
        await row.click();
        assert.equal(await input.inputValue(), 'Retained draft\nSecond line\nThird line');
        await page.evaluate(() => {
          control.adopt(native);
          ViewerFixes.updateSessionControl(root, control.view('native', 'source:native'));
        });
        assert.equal(await input.isVisible(), true);
        assert.equal(await input.getAttribute('readonly'), null);
        await page.evaluate(() =>
          ViewerFixes.updateSessionRuntime(root, {
            observation: {
              state: 'unavailable',
              phase: null,
              freshness: 'unavailable',
              presence: 'unknown',
            },
            loading: false,
            reason: 'Runtime observation unavailable',
          }),
        );
        assert.equal(await input.getAttribute('readonly'), null);
        assert.equal(await input.isVisible(), true);
        assert.equal(await input.inputValue(), 'Retained draft\nSecond line\nThird line');
        await input.focus();
        await page.waitForTimeout(30);
        const geometry = await page.evaluate(() => {
          const b = document.querySelector('#jump-bottom').getBoundingClientRect(),
            c = document.querySelector('.sh-composer').getBoundingClientRect(),
            d = document.querySelector('.session-dock').getBoundingClientRect();
          return {
            clear: b.bottom <= c.top,
            center: Math.abs((b.left + b.right - d.left - d.right) / 2) < 1,
            inside: b.top >= 0 && b.bottom <= innerHeight,
            header: !!document.querySelector('#topbar #jump-bottom'),
            overflow: document.documentElement.scrollWidth > innerWidth + 1,
          };
        });
        assert.deepEqual(geometry, {
          clear: true,
          center: true,
          inside: true,
          header: false,
          overflow: false,
        });
        await auditText(page, { label: `viewer ${width} ${colorScheme}` });
        await page.screenshot({
          path: new URL(`${width}-${colorScheme}-expanded.png`, out).pathname,
        });
        await page.evaluate(() => {
          const note = document.createElement('p');
          note.className = 'livenote';
          note.textContent = 'Reconnecting. Your conversation is retained.';
          const slot = document.createElement('div');
          slot.className = 'viewer-status-slot';
          slot.append(note);
          document.body.append(slot);
        });
        await page.waitForTimeout(50);
        const notice = await page.locator('.livenote').boundingBox();
        const jump = await page.locator('#jump-bottom').boundingBox();
        assert.ok(jump.y + jump.height <= notice.y, 'jump must remain above a reconnect notice');
        await page.locator('.viewer-status-slot').evaluate((node) => node.remove());
        await page.evaluate(() => {
          Object.defineProperty(visualViewport, 'height', { configurable: true, get: () => 560 });
          visualViewport.dispatchEvent(new Event('resize'));
        });
        await page.waitForTimeout(50);
        assert.ok(
          (await page.locator('.session-dock').boundingBox()).y +
            (await page.locator('.session-dock').boundingBox()).height <=
            560,
        );
        await page.evaluate(() => ViewerFixes.updateSessionJump(root, false, 0, false, target));
        assert.equal(await page.locator('#jump-bottom').isVisible(), false);
      } finally {
        await browser.close();
      }
    });
