import { served, reporter, closePage } from '../lib.mjs';

/** Exercise the generated, Rust-served asset, including the host history/live boundary. */
export default async function accountLifecycle(browser) {
  const report = reporter('account-lifecycle');
  for (const size of ['phone', 'desktop'])
    for (const dark of [false, true]) {
      const label = `${size}-${dark ? 'dark' : 'light'}`;
      const page = await served(browser, { account: true, size, dark });
      try {
        const selector =
          size === 'phone' ? '#account-drawer .account-trigger' : '#topbar .account-trigger';
        const trigger = page.locator(selector);
        if (size === 'phone') await page.locator('#lead-btn').click();
        for (let i = 0; i < 3; i++) {
          await trigger.click();
          report.expect(
            (await page.locator('.account-popover').count()) === 1,
            label + ': exactly one open menu',
          );
          report.expect(
            await page.evaluate(
              () =>
                document.activeElement ===
                document.querySelector('.account-popover .account-menu-row'),
            ),
            label + ': first workspace receives focus synchronously',
          );
          if (size === 'desktop') {
            await page.evaluate(() => {
              window.__accountTrigger = document.querySelector('#topbar .account-trigger');
              window.__accountWorkspace = document.querySelector('.account-workspace-form button');
              window.__accountSwitch = document.querySelector('[data-pref="wide"]');
            });
            await page.locator('[data-pref="wide"]').click();
            report.expect(
              await page.evaluate(
                () =>
                  window.__accountTrigger === document.querySelector('#topbar .account-trigger') &&
                  window.__accountWorkspace ===
                    document.querySelector('.account-workspace-form button') &&
                  window.__accountSwitch === document.activeElement,
              ),
              label + ': Display updates retain trigger/workspace/switch nodes and focus',
            );
          }
          await page.keyboard.press('Escape');
          await page.waitForFunction(() => !history.state?.sheet);
          report.expect(
            (await page.locator('.account-popover').count()) === 0 &&
              (await trigger.getAttribute('aria-expanded')) === 'false',
            label + ': Escape closes and resets aria',
          );
          report.expect(
            await trigger.evaluate((node) => node === document.activeElement),
            label + ': focus returns to trigger',
          );
          if (size === 'phone')
            report.expect(
              await page.evaluate(() => document.body.classList.contains('drawer-open')),
              label + ': Escape leaves the drawer open',
            );
        }
        if (size === 'phone') {
          await trigger.click();
          await page.setViewportSize({ width: 1280, height: 860 });
          await page.waitForFunction(
            () => !history.state?.sheet && !document.querySelector('.account-popover'),
          );
          await page.locator('#topbar .account-trigger').click();
          report.expect(
            (await page.locator('.account-popover').count()) === 1,
            label + ': resizing closes the phone sheet before opening desktop chrome',
          );
          await page.keyboard.press('Escape');
          await page.setViewportSize({ width: 390, height: 844 });
          await page.locator('#lead-btn').click();
          await trigger.click();
          await page.goBack();
          report.expect(
            (await page.locator('.account-popover').count()) === 0 &&
              !(await page.evaluate(() => !!history.state?.sheet)),
            label + ': Back consumes only the account sheet',
          );
          await trigger.click();
          await page.locator('.account-backdrop').click({ position: { x: 3, y: 3 } });
          await page.waitForFunction(() => !history.state?.sheet);
          report.expect(
            (await page.locator('.account-popover').count()) === 0,
            label + ': backdrop dismisses the menu and history entry',
          );
        } else {
          await trigger.click();
          await page.locator('#page').click({ position: { x: 8, y: 8 } });
          report.expect(
            (await page.locator('.account-popover').count()) === 0,
            label + ': outside click dismisses',
          );
        }
        await trigger.click();
        await page.evaluate(() =>
          dispatchEvent(new PageTransitionEvent('pageshow', { persisted: true })),
        );
        report.expect(
          (await page.locator('.account-popover').count()) === 0,
          label + ': persisted pageshow dismisses',
        );
        if (size === 'phone') {
          await page.goBack();
          await page.locator('#lead-btn').click();
        } // synthetic pageshow has not restored history

        // Account metadata belongs to the document/host; browsing has no model poller.
        const retired = [];
        page.on('request', (request) => {
          if (/\/api\/(model|tx|events)(?:[?/#]|$)/.test(request.url()))
            retired.push(request.url());
        });
        await trigger.click();
        const heldName = await page.locator('.account-name').textContent();
        await page.waitForTimeout(3200);
        report.expect(
          (await page.locator('.account-name').textContent()) === heldName,
          label + ': native account metadata remains stable during bounded overview updates',
        );
        await page.keyboard.press('Escape');
        report.expect(
          retired.length === 0,
          label + ': account browsing has no workspace model poller',
        );
        if (size === 'phone') {
          await page.waitForFunction(() => !history.state?.sheet);
          await page.route('**/account/profile', (route) =>
            route.fulfill({
              contentType: 'text/html',
              body: '<!doctype html><p>Profile destination</p>',
            }),
          );
          await trigger.click();
          await page.locator('.account-popover a[href="/account/profile"]').click();
          await page.waitForURL((url) => url.pathname === '/account/profile');
          await page.goBack();
          await page.waitForFunction(
            () => history.state?.v && document.querySelector('#topbar .t'),
          );
          report.expect(
            !(await page.evaluate(() => !!history.state?.sheet)) &&
              (await page.locator('.account-popover').count()) === 0,
            label + ': returning from a menu link has no extra sheet history entry',
          );
          if (!(await page.evaluate(() => document.body.classList.contains('drawer-open'))))
            await page.locator('#lead-btn').click();
          const posted = page.waitForResponse(
            (response) =>
              new URL(response.url()).pathname === '/workspaces/research' &&
              response.request().method() === 'POST',
          );
          await trigger.click();
          await page.locator('.account-workspace-form button').first().click();
          report.expect(
            (await posted).status() === 303,
            label + ': phone workspace remains a native same-origin POST',
          );
          await page.waitForURL((url) => url.pathname === '/' && url.search === '');
          await page.waitForFunction(
            () => history.state?.v && document.querySelector('#topbar .t'),
          );
        }
        report.expect(
          page.errors.length === 0,
          label + ': no browser errors: ' + page.errors.join(' | '),
        );
        report.results[label] = { retiredReads: retired.length, errors: page.errors };
      } finally {
        await closePage(page);
      }
    }
  return report.done();
}
