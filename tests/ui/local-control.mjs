// Real native fixture supplies the owner cookie and a pending patch request.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { chromium } from 'playwright';
const [base, token, out] = process.argv.slice(2);
fs.mkdirSync(out, { recursive: true });
const browser = await chromium.launch();
const errors = [];
try {
  const context = await browser.newContext({ viewport: { width: 1280, height: 860 } });
  await context.addCookies([
    { name: 'semon_session', value: token, url: base, httpOnly: true, sameSite: 'Strict' },
  ]);
  const page = await context.newPage();
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.goto(base);
  const panel = page.getByRole('region', { name: 'Conversation controls' });
  await panel
    .getByRole('button', { name: 'Deny', exact: true })
    .filter({ visible: true })
    .last()
    .waitFor();
  await page.screenshot({ path: out + '/desktop-light.png', fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.emulateMedia({ colorScheme: 'dark' });
  await page.getByRole('button', { name: 'Close menu', exact: true }).waitFor({ state: 'hidden' });
  await page.screenshot({ path: out + '/phone-dark.png', fullPage: true });
  assert.equal(
    await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
    true,
  );
  await page
    .getByRole('link', { name: /Codex run/ })
    .filter({ visible: true })
    .first()
    .click();
  await page.locator('section[aria-label="Transcript"]').waitFor();
  await panel
    .getByRole('button', { name: 'Deny', exact: true })
    .filter({ visible: true })
    .last()
    .waitFor();
  await panel.scrollIntoViewIfNeeded();
  await page.screenshot({ path: out + '/session-phone-dark.png' });
  // Find the current open request; earlier resolved cards remain history only.
  await panel
    .locator('button:not([disabled])')
    .filter({ hasText: /^Deny$/ })
    .last()
    .click();
  await panel.getByRole('status').filter({ hasText: 'Response sent' }).waitFor();
  await page.waitForFunction(
    () =>
      !Array.from(document.querySelectorAll('.local-control article button')).some(
        (b) => !b.disabled,
      ),
  );
  await page.reload();
  await panel.getByRole('textbox', { name: 'Message to Codex', exact: true }).waitFor();
  assert.deepEqual(errors, []);
  console.log(
    'Native patch denial through typed viewer; responsive screenshots; cookie refresh; resolved cards cannot answer',
  );
} finally {
  await browser.close();
}
