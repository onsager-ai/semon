// The visual system's rules for text (docs/design/overhaul.md, principles P3 and P7), on every screen that is drawn, at 390 and
// 1280 px in light and dark:
//   - no text is smaller than 12 px;
//   - every text meets WCAG AA against the colour behind it: 4.5:1, or 3:1 for large text (24 px, or 18.66 px and bold).
// "Faint" is not a text colour, and its ratio is about 2.6:1, so it fails here wherever it is used for words. Text that holds no
// letter or digit (a separator such as "·" or "›") is a rule, not words, and isn't measured. Disabled controls and text that is
// not drawn (hidden, closed drawer, under 2 px) are skipped.
import { served, data, reporter, goto, ENV } from '../lib.mjs';
import path from 'node:path';

import { auditText as audit } from '../text-audit.mjs';

const tokenColors = (page) =>
  page.evaluate(() => {
    const probe = document.createElement('span');
    probe.style.position = 'fixed';
    probe.style.visibility = 'hidden';
    document.body.append(probe);
    probe.style.color = 'var(--ink)';
    const ink = getComputedStyle(probe).color;
    probe.style.color = 'var(--accent)';
    const accent = getComputedStyle(probe).color;
    probe.remove();
    return { ink, accent };
  });

const styledText = (page, selector) =>
  page.evaluate((sel) => {
    const el = document.querySelector(sel);
    if (!el) return null;
    const style = getComputedStyle(el);
    return { color: style.color, weight: Number(style.fontWeight), text: el.textContent.trim() };
  }, selector);

const legacyHues = (page) =>
  page.evaluate(() => {
    const old = new Set(['rgb(59, 77, 191)', 'rgb(152, 164, 243)']),
      hits = [];
    for (const el of document.querySelectorAll('*')) {
      const style = getComputedStyle(el),
        values = [
          style.color,
          style.backgroundColor,
          style.borderTopColor,
          style.borderRightColor,
          style.borderBottomColor,
          style.borderLeftColor,
        ];
      if (values.some((value) => old.has(value)))
        hits.push({
          tag: el.tagName.toLowerCase(),
          className: typeof el.className === 'string' ? el.className : '',
          values: values.filter((value) => old.has(value)),
        });
      if (hits.length === 8) break;
    }
    return hits;
  });

export default async function tokensCheck(browser) {
  const D = await data(),
    r = reporter('tokens'),
    results = {};
  const harbor = D.SESS.harbor ? 'harbor' : Object.keys(D.SESS)[0];
  const parent =
    Object.values(D.SESS).find(
      (s) => Object.values(D.SESS).filter((c) => c.parent === s.id).length >= 5,
    )?.id ?? harbor;
  const sender = D.H.find((event) => event.reason === 'not linked yet' && D.SESS[event.from]);
  if (!sender) throw new Error('The fixture needs a sender-only relationship card');
  const turn = D.turns.find((t) => t.sent.length);
  const screens = [
    ['home', { v: 'home' }],
    ['sessions', { v: 'sessions' }],
    ['analytics', { v: 'analytics' }],
    ['machines', { v: 'machines' }],
    ['session-' + harbor, { v: 'session', id: harbor }],
    ['session-' + parent, { v: 'session', id: parent }],
    ['sender-card', { v: 'session', id: sender.from }],
    ...(turn ? [['trace', { v: 'trace', sid: turn.sid, turn: turn.id }]] : []),
  ];
  for (const [size, dark] of [
    ['phone', false],
    ['phone', true],
    ['desktop', false],
    ['desktop', true],
  ]) {
    const width = size === 'phone' ? 390 : 1280,
      scheme = dark ? 'dark' : 'light';
    const tag = size + (dark ? '-dark' : '-light'),
      rec = (results[tag] = {}),
      page = await served(browser, { size, dark });
    const tokens = await tokenColors(page);
    r.expect(
      tokens.accent === tokens.ink,
      tag + ': --accent does not compute to --ink: ' + JSON.stringify(tokens),
    );
    const judge = (where, a) => {
      rec[where] = { texts: a.texts, small: a.smallCount, low: a.lowCount };
      r.expect(a.texts > 5, tag + ' ' + where + ': only ' + a.texts + ' texts were measured');
      r.expect(
        a.smallCount === 0,
        tag +
          ' ' +
          where +
          ': ' +
          a.smallCount +
          ' texts are under 12px: ' +
          JSON.stringify(a.small),
      );
      r.expect(
        a.lowCount === 0,
        tag +
          ' ' +
          where +
          ': ' +
          a.lowCount +
          ' texts are under AA contrast: ' +
          JSON.stringify(a.low),
      );
    };
    for (const [name, route] of screens) {
      await goto(page, route, D);
      await page.waitForTimeout(150);
      judge(name, await audit(page));
      if (route.v === 'home' || route.v === 'analytics' || route.v === 'session') {
        const hits = await legacyHues(page);
        r.expect(
          hits.length === 0,
          tag +
            ' ' +
            name +
            ': legacy accent colors remain in computed color, background, or border: ' +
            JSON.stringify(hits),
        );
      }
      if (name === 'home') {
        rec.homeUnreadDots = await page.locator('.ib .unread-dot').count();
        await page.screenshot({
          path: path.join(ENV.out, 'inkaccent-' + width + '-' + scheme + '-home.png'),
          fullPage: true,
        });
      }
      if (name === 'sender-card') {
        const card = page.locator(`.event[data-h="${sender.id}"]`);
        await card.getByText('not linked yet', { exact: true }).waitFor();
        r.expect(
          (await card.locator('.who-link').count()) === 0,
          tag + ': sender card has no target link',
        );
      }
      if (name === 'trace') {
        const crumb = await styledText(page, '.topbar .crumb');
        r.expect(
          !!crumb && crumb.color === tokens.ink && crumb.weight >= 500,
          tag +
            ': Trace breadcrumb is missing, not ink, or below weight 500: ' +
            JSON.stringify(crumb),
        );
        await page.screenshot({
          path: path.join(ENV.out, 'inkaccent-' + width + '-' + scheme + '-trace.png'),
          fullPage: true,
        });
      }
    }
    // The phone's navigation drawer is closed everywhere else; open it on Home. On desktop the sidebar is drawn beside every screen.
    if (size === 'phone') {
      await goto(page, { v: 'home' }, D);
      await page.click('#lead-btn');
      await page.waitForTimeout(320);
      judge('drawer', await audit(page));
    }
    r.expect(page.errors.length === 0, tag + ': page errors ' + page.errors.join('; '));
    await page.context().close();
  }
  r.results = results;
  return r.done();
}
