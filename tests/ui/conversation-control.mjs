// Current conversation UI; the recorded local-control.mjs probe remains immutable.
// Real native fixture supplies the owner cookie and a pending patch request.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { chromium } from 'playwright';
const [base, token, out, sessionId, mode] = process.argv.slice(2);
assert.ok(sessionId, 'Native control qualification requires the launched session id');
fs.mkdirSync(out, { recursive: true });
const browser = await chromium.launch();
const errors = [];
let page;
try {
  const context = await browser.newContext({ viewport: { width: 1280, height: 860 } });
  await context.addCookies([
    { name: 'semon_session', value: token, url: base, httpOnly: true, sameSite: 'Strict' },
  ]);
  page = await context.newPage();
  page.on('pageerror', (e) => errors.push(String(e)));
  if (mode === 'bootstrap') {
    await page.goto(base);
    const panel = page.getByRole('region', { name: 'Conversation controls' });
    const input = panel.getByRole('textbox', { name: 'Message to Codex', exact: true });
    await input.waitFor();
    await page.screenshot({ path: out + '/launch-desktop-light.png', fullPage: true });
    await page.setViewportSize({ width: 390, height: 844 });
    await page.emulateMedia({ colorScheme: 'dark' });
    const closeMenu = page.getByRole('button', { name: 'Close menu', exact: true });
    await page.keyboard.press('Escape');
    await closeMenu.waitFor({ state: 'hidden' });
    await page.screenshot({ path: out + '/launch-phone-dark.png', fullPage: true });
    assert.equal(
      await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
      true,
    );
    await input.fill('LOCAL_INPUT');
    const response = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === '/api/control' &&
        response.request().method() === 'POST',
    );
    await panel.getByRole('button', { name: 'Send', exact: true }).click();
    const reply = await response;
    assert.equal(reply.status(), 200);
    const command = reply.request().postDataJSON();
    const receipt = await reply.json();
    assert.equal(command.launch, true);
    assert.equal(command.thread, sessionId);
    assert.equal(receipt.snapshot.actions[command.id].delivery, 'accepted');
    await page.waitForFunction(
      () =>
        !Array.from(document.querySelectorAll('textarea')).some(
          (input) => input.value === 'LOCAL_INPUT',
        ),
    );
    assert.deepEqual(errors, []);
    console.log(
      'Native first input through isolated launch authority; no Catalog identity or seeded transcript',
    );
  } else {
    await page.goto(base + '/session/local/' + encodeURIComponent(sessionId));
    await page.locator('section[aria-label="Transcript"]').waitFor();
    await page.getByRole('button', { name: /^(Open Composer|Composer;)/ }).click();
    const panel = page.getByRole('region', { name: 'Conversation controls' });
    await panel
      .getByRole('button', { name: 'Deny', exact: true })
      .filter({ visible: true })
      .last()
      .waitFor();
    await page.screenshot({ path: out + '/desktop-light.png', fullPage: true });
    await page.setViewportSize({ width: 390, height: 844 });
    await page.emulateMedia({ colorScheme: 'dark' });
    await page
      .getByRole('button', { name: 'Close menu', exact: true })
      .waitFor({ state: 'hidden' });
    await page.screenshot({ path: out + '/phone-dark.png', fullPage: true });
    assert.equal(
      await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
      true,
    );
    await page
      .getByRole('dialog', { name: 'Session Composer', exact: true })
      .getByRole('button', { name: 'Close', exact: true })
      .click();
    await page.getByRole('button', { name: 'Back to Sessions', exact: true }).click();
    await page.locator('[data-session-row="list"][data-id="' + sessionId + '"]').click();
    await page.locator('section[aria-label="Transcript"]').waitFor();
    await page.getByRole('button', { name: /^(Open Composer|Composer;)/ }).click();
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
    await page.getByRole('button', { name: /^(Open Composer|Composer;)/ }).click();
    await panel.getByRole('textbox', { name: 'Message to Codex', exact: true }).waitFor();
    assert.deepEqual(errors, []);
    console.log(
      'Native patch denial through typed viewer; responsive screenshots; cookie refresh; resolved cards cannot answer',
    );
  }
} catch (error) {
  if (page) {
    fs.writeFileSync(out + '/failure.html', await page.content());
    fs.writeFileSync(out + '/page-errors.json', JSON.stringify(errors, null, 2));
    await page.screenshot({ path: out + '/failure.png', fullPage: true });
  }
  throw error;
} finally {
  await browser.close();
}
