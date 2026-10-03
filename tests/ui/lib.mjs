// Shared by the browser checks: the served viewer's address, pages opened on it, the served data in the mockup's shapes,
// and the report every check writes. Environment:
//   SEMON_BASE   http://127.0.0.1:PORT of a running `semon sessions --serve`
//   SEMON_TOKEN  its token (the ?t= it printed)
//   SEMON_NOW    the fixture's pinned now (epoch ms): the browser's clock stands there too
//   SEMON_UI_OUT where reports, screenshots and diffs go (default: ./out)
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';

export const ENV = {
  base: (process.env.SEMON_BASE ?? '').replace(/\/$/, ''),
  token: process.env.SEMON_TOKEN ?? '',
  // The extras fixture's server (fixture.mjs --extras).
  extraBase: (process.env.SEMON_EXTRA_BASE ?? '').replace(/\/$/, ''),
  extraToken: process.env.SEMON_EXTRA_TOKEN ?? '',
  accountBase: (process.env.SEMON_ACCOUNT_BASE ?? '').replace(/\/$/, ''),
  accountToken: process.env.SEMON_ACCOUNT_TOKEN ?? '',
  now: Number(process.env.SEMON_NOW ?? Date.now()),
  out: path.resolve(process.env.SEMON_UI_OUT ?? 'out'),
};
fs.mkdirSync(ENV.out, { recursive: true });

export const launch = () =>
  chromium.launch({
    args: ['--disable-gpu', '--font-render-hinting=none'],
    ...(process.platform === 'linux'
      ? {
          env: {
            ...process.env,
            FONTCONFIG_FILE: fileURLToPath(new URL('./fontconfig.conf', import.meta.url)),
          },
        }
      : {}),
  });

// The three screens every check and comparison covers.
export const VIEWPORTS = {
  phone: {
    viewport: { width: 390, height: 844 },
    deviceScaleFactor: 2,
    isMobile: true,
    hasTouch: true,
  },
  desktop: {
    viewport: { width: 1280, height: 860 },
    deviceScaleFactor: 1,
    isMobile: false,
    hasTouch: false,
  },
};

export async function context(browser, { size = 'phone', dark = false, viewport } = {}) {
  return browser.newContext({
    ...VIEWPORTS[size],
    ...(viewport ? { viewport } : {}),
    colorScheme: dark ? 'dark' : 'light',
    timezoneId: 'UTC',
    locale: 'en-US',
    reducedMotion: 'no-preference',
  });
}

// A page on the served viewer at `path` (such as "/" or "/s/claude/harbor"), signed in with the token. Only the served
// origin is reachable. `page.errors` collects page errors. `extras: true` opens the extras fixture's server.
export async function served(browser, opts = {}) {
  const base =
    opts.base ?? (opts.account ? ENV.accountBase : opts.extras ? ENV.extraBase : ENV.base);
  const token =
    opts.token ?? (opts.account ? ENV.accountToken : opts.extras ? ENV.extraToken : ENV.token);
  if (!base) throw new Error(opts.extras ? 'SEMON_EXTRA_BASE is not set' : 'SEMON_BASE is not set');
  const ctx = await context(browser, opts);
  const page = await ctx.newPage();
  page.errors = [];
  page.on('pageerror', (e) => page.errors.push(e.message.split('\n')[0]));
  page.setDefaultTimeout(opts.timeout ?? 5000);
  await page.clock.setFixedTime(ENV.now);
  await page.route(/.*/, (r) =>
    r
      .request()
      .url()
      .startsWith(base + '/')
      ? r.continue()
      : r.abort(),
  );
  await page.goto(
    base + (opts.path ?? '/') + ((opts.path ?? '/').includes('?') ? '&' : '?') + 't=' + token,
    { waitUntil: 'load' },
  );
  await settled(page);
  return page;
}

// The screen a route draws has its title in the bar, its transcript loaded, and its fonts in.
export function titleOf(route, D) {
  return (
    {
      home: 'Home',
      analytics: 'Analytics',
      sessions: 'Sessions',
      machines: 'Machines',
      trace: 'Trace',
    }[route.v] ?? (route.v === 'machine' ? D.MACHINE[route.id] : D.SESS[route.id]?.name)
  );
}
export async function settled(page) {
  await page.waitForFunction(() => {
    const panel = document.querySelector('#page');
    // The HTML shell can already contain a title and loading text while boot's
    // first model fetch is pending. Navigation before history is adopted can
    // be overwritten by boot when that request finishes.
    return (
      history.state?.v &&
      document.querySelector('#topbar .t, #topbar #find') &&
      panel?.childElementCount > 0 &&
      !panel.hasAttribute('aria-busy') &&
      (history.state.v !== 'session' || panel.querySelector("section[aria-label='Transcript']"))
    );
  });
  await page.evaluate(() => document.fonts.ready);
}

// Stop mocked polls before disposing their responses. ignoreErrors also releases
// handlers deliberately held behind a gate by an embedding test.
export async function closePage(page) {
  await page.unrouteAll({ behavior: 'ignoreErrors' }).catch(() => {});
  await page
    .context()
    .close()
    .catch(() => {});
}

// Goes to a route the way the mockup's check scripts did: a history entry and a popstate, then waits for its screen.
export async function goto(page, route, D) {
  await page.evaluate((r) => {
    history.pushState(r, '');
    dispatchEvent(new PopStateEvent('popstate', { state: r }));
  }, route);
  const title = titleOf(route, D);
  // A session's title is in the bar as soon as it is opened; its page is ready once it is no longer aria-busy.
  if (title) {
    try {
      await page.waitForFunction(
        (t) =>
          document.querySelector('#topbar .t')?.textContent === t &&
          !document.querySelector('#page').hasAttribute('aria-busy'),
        title,
      );
    } catch (error) {
      let state;
      try {
        state = await page.evaluate(() => ({
          title: document.querySelector('#topbar .t')?.textContent ?? null,
          busy: document.querySelector('#page')?.hasAttribute('aria-busy') ?? null,
          pathname: location.pathname,
        }));
      } catch (diagnosticError) {
        state = { diagnosticError: diagnosticError.message };
      }
      const pageErrors = 'errors' in page ? `; page.errors=${JSON.stringify(page.errors)}` : '';
      throw new Error(
        `goto timed out waiting for title ${JSON.stringify(title)}; actual #topbar .t=${JSON.stringify(state.title ?? null)}; #page has aria-busy=${state.busy ?? 'unavailable'}; location.pathname=${state.pathname ?? 'unavailable'}${pageErrors}; wait error=${error.message}`,
      );
    }
  }
  if (route.v === 'session')
    await page.waitForFunction(
      () => !!document.querySelector("#page section[aria-label='Transcript']"),
    );
  // Analytics draws whole once the server's answer for its range is in (/api/analytics).
  if (route.v === 'analytics')
    await page.waitForFunction(() =>
      document.querySelector('.analytics-metrics[data-analytics-ready]'),
    );
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(60);
}

// The served data in the mockup's shapes: SESS, H, MACHINE, MACHINE_UP, NOW, every transcript in full as TX, and the
// server's turn index.
export async function data({ extras = false } = {}) {
  const base = extras ? ENV.extraBase : ENV.base,
    token = extras ? ENV.extraToken : ENV.token;
  const get = async (p) => {
    const r = await fetch(base + p + (p.includes('?') ? '&' : '?') + 't=' + token);
    if (!r.ok) throw new Error(p + ': ' + r.status);
    return r.json();
  };
  const model = await get('/api/model');
  const SESS = model.sessions,
    H = model.handoffs,
    TX = {},
    TXM = {};
  for (const [id, s] of Object.entries(SESS)) s.id = id;
  for (const sid of Object.keys(SESS)) {
    let page = await get('/api/tx?sid=' + encodeURIComponent(sid)),
      entries = page.entries;
    while (page.from > 0) {
      page = await get('/api/tx?sid=' + encodeURIComponent(sid) + '&before=' + page.from);
      entries = page.entries.concat(entries);
    }
    TX[sid] = entries;
    TXM[sid] = { calls: page.calls, errors: page.errors };
  }
  const machines = model.machines ?? [model.machine];
  return {
    model,
    SESS,
    H,
    TX,
    TXM,
    turns: model.turns,
    NOW: model.now,
    MACHINE: Object.fromEntries(machines.map((m) => [m.id, m.name])),
    MACHINE_UP: Object.fromEntries(machines.map((m) => [m.id, m.up])),
    MACHINE_LAST: Object.fromEntries(
      machines.filter((m) => m.last != null).map((m) => [m.id, m.last]),
    ),
    ADMIN: model.admin ?? null,
  };
}

// The filters of Analytics and Sessions live in a sheet the Filter button opens; its Selects are in the page even while it is shut.
// A choice applies when the sheet closes ("Done"). `filterSheet` opens it, `doneFilterSheet` closes it by "Done", and `pickFilter`
// chooses one value (a Select's option: click its button, then the option) and closes the sheet.
export const FILTER_BUTTON = '.facet-filters .facet-btn',
  FILTER_SHEET = '.facet-filters dialog.filters-sheet';
export const filterSheet = async (page) => {
  await page.click(FILTER_BUTTON);
  await page.waitForFunction((sel) => document.querySelector(sel)?.open === true, FILTER_SHEET);
};
// The choices apply in the dialog's close event, a task after it closes: the helper waits two frames for the page to draw them.
export const doneFilterSheet = async (page) => {
  await page.click(FILTER_SHEET + ' .fdone');
  await page.waitForFunction((sel) => document.querySelector(sel)?.open === false, FILTER_SHEET);
  await page.evaluate(
    () => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))),
  );
};
export async function pickFilter(page, label, value) {
  const root = FILTER_SHEET + ' .sh-select[data-label="' + label + '"]';
  await filterSheet(page);
  await page.click(root + ' .sh-select-trigger');
  await page.locator(root + ' [role="option"][data-value="' + value + '"]').click();
  await page.waitForFunction(
    (sel) =>
      document.querySelector(sel + ' .sh-select-trigger').getAttribute('aria-expanded') === 'false',
    root,
  );
  await doneFilterSheet(page);
}

// The wide-transcript switch lives in the session menu: open it on the current session, set it, close the menu.
export async function wide(page, on) {
  await page.click('#more-btn');
  const sw = page.locator('dialog.session-menu [role="menuitemcheckbox"]');
  if ((await sw.getAttribute('aria-checked')) !== String(on)) await sw.click();
  await page.keyboard.press('Escape');
  await page.waitForTimeout(150);
}

// Each check writes out/<name>.json and fails when any of its assertions failed.
export function reporter(name) {
  const failures = [];
  const rep = {
    // A check may replace `results` whole; the report reads it when done.
    results: {},
    expect(ok, what) {
      if (!ok) failures.push(what);
      return ok;
    },
    done() {
      fs.writeFileSync(
        path.join(ENV.out, name + '.json'),
        JSON.stringify({ results: rep.results, failures }, null, 1),
      );
      console.log('== ' + name + ': ' + (failures.length ? failures.length + ' failed' : 'ok'));
      console.log(JSON.stringify(rep.results));
      for (const f of failures) console.log('  FAIL ' + f);
      return failures.length === 0;
    },
  };
  return rep;
}

// Elements past the screen's edge that no overflow-clipping ancestor hides, plus 1000 if the page itself scrolls sideways
// (the mockup check scripts' over()).
export const overflow = (page) =>
  page.evaluate(() => {
    document.body.style.overflowX = 'visible';
    const vw = document.documentElement.clientWidth;
    let n = 0;
    const clipped = (e) => {
      for (let a = e.parentElement; a && a !== document.body; a = a.parentElement) {
        const o = getComputedStyle(a).overflowX;
        if (o === 'hidden' || o === 'clip' || o === 'auto' || o === 'scroll') {
          const r = a.getBoundingClientRect();
          if (r.right <= vw + 0.5) return true;
        }
      }
      return false;
    };
    for (const e of document.querySelectorAll('.page *, .topbar *')) {
      const r = e.getBoundingClientRect();
      if (r.width && r.height && (r.right > vw + 0.5 || r.left < -0.5) && !clipped(e)) n++;
    }
    const sw = document.documentElement.scrollWidth;
    document.body.style.overflowX = '';
    if (sw > vw) n += 1000;
    return n;
  });

// The WCAG contrast ratio of the first element `selector` matches: its text colour over the background it is drawn on (its own
// and its ancestors' backgrounds composited, over white), in the page's colour scheme. Null when a colour can't be read.
export const contrastOf = (page, selector) =>
  page.evaluate((selector) => {
    const parse = (value) => {
      let m = /^rgba?\(([^)]+)\)$/.exec(value);
      if (m) {
        const p = m[1]
          .split(/[ ,/]+/)
          .filter(Boolean)
          .map(Number);
        return [p[0], p[1], p[2], p[3] ?? 1];
      }
      m = /^color\(srgb ([^)]+)\)$/.exec(value);
      if (m) {
        const p = m[1].split(/[ /]+/).filter(Boolean).map(Number);
        return [p[0] * 255, p[1] * 255, p[2] * 255, p[3] ?? 1];
      }
      return null;
    };
    const node = document.querySelector(selector);
    if (!node) return null;
    const fg = parse(getComputedStyle(node).color);
    if (!fg) return null;
    const layers = [];
    for (let e = node; e; e = e.parentElement) {
      const c = parse(getComputedStyle(e).backgroundColor);
      if (c && c[3] > 0) {
        layers.push(c);
        if (c[3] >= 1) break;
      }
    }
    let base = [255, 255, 255];
    for (const [r, g, b, a] of layers.reverse())
      base = [r * a + base[0] * (1 - a), g * a + base[1] * (1 - a), b * a + base[2] * (1 - a)];
    const over = (c) => [0, 1, 2].map((i) => c[i] * (fg[3] ?? 1) + base[i] * (1 - (fg[3] ?? 1)));
    const lum = ([r, g, b]) => {
      const f = (v) => {
        v /= 255;
        return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
      };
      return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
    };
    const a = lum(over(fg)),
      b = lum(base);
    return Math.round(((Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05)) * 100) / 100;
  }, selector);
