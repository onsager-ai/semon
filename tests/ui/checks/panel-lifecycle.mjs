import { served, data, goto, reporter } from '../lib.mjs';

// Exercise the served generated asset: native dialog close must step over its
// history entry, release the live hold and return focus without leaving a route.
export default async function panelLifecycle(browser) {
  const D = await data(), r = reporter('panel-lifecycle');
  for (const size of ['phone', 'desktop']) for (const dark of [false, true]) {
    const page = await served(browser, { size, dark });
    try {
      await goto(page, { v: 'session', id: 'harbor' }, D);
      for (const dismissal of ['button', 'escape', 'back', 'backdrop']) {
        const tag = `${size}/${dark ? 'dark' : 'light'}/${dismissal}`, url = page.url();
        await page.click('#more-btn');
        await page.waitForSelector('dialog.session-menu[open]');
        const opened = await page.evaluate(() => {
          const dialog = document.querySelector('dialog.session-menu');
          window.oldPanelClose = dialog.querySelector('.panel-h .ibtn');
          return { sheet: history.state?.sheet, focus: document.activeElement === dialog.querySelector('.panel-b'), expanded: document.querySelector('#more-btn').getAttribute('aria-expanded') };
        });
        r.expect(opened.sheet === 1 && opened.focus && opened.expanded === 'true', tag + ': modal/history/focus did not commit');
        if (dismissal === 'button') await page.click('dialog.session-menu .panel-h .ibtn');
        else if (dismissal === 'escape') await page.keyboard.press('Escape');
        else if (dismissal === 'back') await page.evaluate(() => history.back());
        else await page.evaluate(() => document.querySelector('dialog.session-menu').dispatchEvent(new MouseEvent('click', { bubbles: true })));
        await page.waitForFunction(() => !document.querySelector('dialog.session-menu') && !history.state?.sheet);
        const closed = await page.evaluate(() => ({ focus: document.activeElement === document.querySelector('#more-btn'), expanded: document.querySelector('#more-btn').getAttribute('aria-expanded'), held: document.documentElement.classList.contains('panel-open') }));
        r.expect(page.url() === url && closed.focus && closed.expanded === 'false' && !closed.held, tag + ': close lost route/focus or retained its live hold');
        await page.click('#more-btn');
        await page.waitForSelector('dialog.session-menu[open]');
        await page.evaluate(() => oldPanelClose.click());
        r.expect(await page.locator('dialog.session-menu[open]').count() === 1, tag + ': stale close control affected the next panel');
        await page.keyboard.press('Escape');
        await page.waitForFunction(() => !document.querySelector('dialog.session-menu') && !history.state?.sheet);
      }
      r.expect(page.errors.length === 0, `${size}/${dark}: ${page.errors.join('; ')}`);
    } finally { await page.context().close(); }
  }
  return r.done();
}
