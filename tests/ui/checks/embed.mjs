// Typed consumers mount the one application and preserve native document ownership.
import assert from 'node:assert/strict';
import { build } from '../../../ui/node_modules/esbuild/lib/main.js';
import { served, closePage, overflow } from '../lib.mjs';

export default async function embedCheck(browser) {
  const bundle = await build({
    absWorkingDir: new URL('../../../ui', import.meta.url).pathname,
    entryPoints: ['src/application.ts'],
    bundle: true,
    write: false,
    format: 'iife',
    globalName: 'ViewerHostTest',
    platform: 'browser',
    tsconfig: 'tsconfig.json',
    define: { 'process.env.NODE_ENV': '"production"' },
  });
  for (const width of [390, 820, 1280])
    for (const dark of [false, true]) {
      const page = await served(browser, {
        size: width === 390 ? 'phone' : 'desktop',
        dark,
        viewport: { width, height: 860 },
      });
      try {
        const retired = [];
        page.on('request', (request) => {
          if (/\/api\/(model|tx|events)(?:[?/#]|$)/.test(request.url()))
            retired.push(request.url());
        });
        await page.route('**/viewer.js', (route) =>
          route.fulfill({ contentType: 'text/javascript', body: bundle.outputFiles[0].text }),
        );
        await page.reload();
        const result = await page.evaluate(async () => {
          const { mountViewerApplication, mountNativeShell } = ViewerHostTest;
          const first = mountViewerApplication({
            machinesPath: '/machines',
            catalogSources: false,
          });
          first.destroy();
          const body = document.createElement('section');
          body.id = 'native-host-body';
          body.innerHTML =
            '<h1>Native settings</h1><form method="post" action="/native/save"><input name="value" value="retained"><button type="submit">Save settings</button></form>';
          document.querySelector('.app').append(body);
          const form = body.querySelector('form'),
            input = body.querySelector('input');
          const shell = mountNativeShell({ chrome: false });
          const native = mountViewerApplication({
            machinesPath: '/machines',
            nativePage: { title: 'Native settings', nav: 'machines' },
            account: {
              name: '<script>literal</script>',
              login: 'fixture',
              initials: 'FT',
              avatar_href: null,
              workspaces: [],
              links: [{ label: 'Sign out', href: '/logout', method: 'post', danger: true }],
            },
          });
          await new Promise((resolve) =>
            requestAnimationFrame(() => requestAnimationFrame(resolve)),
          );
          const before = {
            sameForm: form === body.querySelector('form'),
            value: input.value,
            actions: form.getAttribute('action'),
            appCount: document.querySelectorAll('.app').length,
            unsafe: !!document.querySelector('#topbar script'),
            account: !!document.querySelector('.account-trigger'),
          };
          native.destroy();
          shell.destroy();
          const retained = form.isConnected && input.value === 'retained';
          body.remove();
          const app = mountViewerApplication({ machinesPath: '/machines', catalogSources: false });
          const newer = mountViewerApplication({
            machinesPath: '/machines',
            catalogSources: false,
          });
          app.destroy(); // an old handle cannot destroy the replacement owner
          await new Promise((resolve) =>
            requestAnimationFrame(() => requestAnimationFrame(resolve)),
          );
          const replacementAlive = !!document.querySelector('#nav a[data-go="sessions"]');
          newer.destroy();
          mountViewerApplication({ machinesPath: '/machines', catalogSources: false });
          return { before, retained, replacementAlive };
        });
        assert.deepEqual(result.before, {
          sameForm: true,
          value: 'retained',
          actions: '/native/save',
          appCount: 1,
          unsafe: false,
          account: true,
        });
        assert.equal(result.retained, true);
        assert.equal(result.replacementAlive, true);
        await page.locator('#page h1').filter({ hasText: 'Sessions' }).waitFor();
        assert.equal(await overflow(page), 0);
        assert.deepEqual(retired, []);
        assert.deepEqual(page.errors, []);
      } finally {
        await closePage(page);
      }
    }
  console.log('Typed full/native consumer ownership and remount passed');
  return true;
}
