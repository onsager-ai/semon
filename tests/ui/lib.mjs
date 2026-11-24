// Shared by the browser checks: the served viewer's address, pages opened on it, the served data in the mockup's shapes,
// and the report every check writes. Environment:
//   SEMON_BASE   http://127.0.0.1:PORT of a running `semon sessions --serve`
//   SEMON_TOKEN  its token (the ?t= it printed)
//   SEMON_NOW    the fixture's pinned now (epoch ms): the browser's clock stands there too
//   SEMON_UI_OUT where reports and screenshots go (default: ./out)
import fs from 'node:fs';
import path from 'node:path';
import { chromium } from 'playwright';
import { launchOptions } from './launch-options.mjs';
import { sample } from './fixture.mjs';

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

export const launch = () => chromium.launch(launchOptions());

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

// A page on the served viewer at `path` (such as "/" or "/session/local/harbor"), signed in with the token. Only the served
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
  try {
    await settled(page);
  } catch (error) {
    const dom = await page
      .evaluate(() => ({
        historyState: history.state,
        pathname: location.pathname,
        pageText: document.querySelector('#page')?.innerText.slice(0, 600) ?? null,
      }))
      .catch((diagnosticError) => ({ unavailable: diagnosticError.message }));
    throw new Error(
      `initial viewer load did not settle; DOM=${JSON.stringify(dom)}; ` +
        `page errors=${JSON.stringify(page.errors)}; cause=${error.message}`,
      { cause: error },
    );
  }
  return page;
}

// Capture geometry only when a selected, real Playwright click fails. The
// successful click path, timeout and scrolling remain unchanged.
export async function clickWithFailureDiagnostics(page, selector, label, click) {
  try {
    await click();
  } catch (error) {
    let evidence;
    try {
      evidence = await page.evaluate(async (selector) => {
        const round = (value) => Math.round(value * 10) / 10;
        const box = (node) => {
          if (!node) return null;
          const rect = node.getBoundingClientRect();
          return {
            left: round(rect.left),
            top: round(rect.top),
            right: round(rect.right),
            bottom: round(rect.bottom),
            width: round(rect.width),
            height: round(rect.height),
          };
        };
        const identify = (node) =>
          node instanceof Element
            ? node.tagName.toLowerCase() +
              (node.id ? '#' + node.id : '') +
              [...node.classList]
                .slice(0, 12)
                .map((name) => '.' + name)
                .join('')
                .slice(0, 200)
            : (node?.nodeName ?? String(node)).slice(0, 100);
        const describe = (node) => {
          if (!node) return null;
          const style = getComputedStyle(node);
          const rect = box(node);
          return {
            node: identify(node),
            expanded: node.getAttribute?.('aria-expanded'),
            disabled: node.matches?.(':disabled') ?? false,
            box: rect,
            display: style.display,
            visibility: style.visibility,
            position: style.position,
            zIndex: style.zIndex,
            pointerEvents: style.pointerEvents,
            overflow: [style.overflowX, style.overflowY],
            clipPath: style.clipPath,
            readingClip: style.getPropertyValue('--reading-clip').trim(),
            scrollMargin: [style.scrollMarginTop, style.scrollMarginBottom],
            scrollPadding: [style.scrollPaddingTop, style.scrollPaddingBottom],
            scroll: [
              Math.round(node.scrollTop ?? 0),
              node.scrollHeight ?? 0,
              node.clientHeight ?? 0,
            ],
          };
        };
        const matches = document.querySelectorAll(selector);
        const candidates = [...matches].slice(0, 16);
        const target =
          candidates.find((node) => {
            const style = getComputedStyle(node);
            return node.getClientRects().length && style.visibility !== 'hidden';
          }) ??
          candidates[0] ??
          null;
        const main = document.querySelector('#main');
        const topbar = document.querySelector('#topbar');
        const dock = document.querySelector('.session-dock');
        const transcript = target?.closest('.transcript') ?? document.querySelector('.transcript');
        const reader = matchMedia('(max-width: 760px)').matches ? document.documentElement : main;
        const jump = document.querySelector('.session-jump-slot .jump, .jump');
        const ancestors = [];
        for (let node = target; node && ancestors.length < 12; node = node.parentElement) {
          ancestors.push(describe(node));
          if (node === main || node === document.documentElement) break;
        }
        const intersects = (left, right) =>
          !!left &&
          !!right &&
          left.left < right.right &&
          left.right > right.left &&
          left.top < right.bottom &&
          left.bottom > right.top;
        const snapshot = () => {
          const targetRect = box(target);
          const topbarRect = box(topbar);
          const dockRect = box(dock);
          const transcriptRect = box(transcript);
          const clip = transcript
            ? parseFloat(getComputedStyle(transcript).getPropertyValue('--reading-clip')) || 0
            : 0;
          const readerGap = reader
            ? Math.max(0, reader.scrollHeight - reader.clientHeight - reader.scrollTop)
            : null;
          const center = targetRect
            ? {
                x: (targetRect.left + targetRect.right) / 2,
                y: (targetRect.top + targetRect.bottom) / 2,
              }
            : null;
          return {
            target: describe(target),
            scroll: {
              windowY: Math.round(window.scrollY),
              readerGap: readerGap === null ? null : Math.round(readerGap),
              nearEndByGap: readerGap === null ? null : readerGap <= 80,
              jumpButton: describe(jump),
              activeElement: identify(document.activeElement),
              document: describe(document.scrollingElement),
              main: describe(main),
            },
            topbar: describe(topbar),
            dock: describe(dock),
            transcript: describe(transcript),
            targetAncestorChain: ancestors,
            overlap: {
              topbar: intersects(targetRect, topbarRect),
              dock: intersects(targetRect, dockRect),
              clippedByTranscriptBottom:
                !!targetRect &&
                !!transcriptRect &&
                targetRect.bottom > transcriptRect.bottom - clip &&
                targetRect.top < transcriptRect.bottom,
            },
            hitStack:
              center &&
              center.x >= 0 &&
              center.x < innerWidth &&
              center.y >= 0 &&
              center.y < innerHeight
                ? document.elementsFromPoint(center.x, center.y).slice(0, 8).map(identify)
                : [],
          };
        };
        const frames = [snapshot()];
        for (let index = 0; index < 3; index++) {
          await new Promise((resolve) => {
            const timer = setTimeout(resolve, 100);
            requestAnimationFrame(() => {
              clearTimeout(timer);
              resolve();
            });
          });
          frames.push(snapshot());
        }
        return { selector, candidateCount: matches.length, frames };
      }, selector);
    } catch (diagnosticError) {
      evidence = {
        unavailable: String(diagnosticError?.message ?? diagnosticError).slice(0, 1000),
      };
    }
    console.error(
      '[served click geometry failure] ' +
        JSON.stringify({ label, error: String(error.message ?? error).slice(0, 8000), evidence }),
    );
    throw error;
  }
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
      !panel.querySelector('[aria-busy="true"]') &&
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
const routeSegment = (value) =>
  encodeURIComponent(value).replace(
    /[!'()*]/g,
    (character) => '%' + character.charCodeAt(0).toString(16).toUpperCase(),
  );

export async function goto(page, route, D) {
  const sid = route.id ?? route.sid,
    session = D.SESS[sid],
    source = session ? D.SOURCES?.[session.machine] : undefined,
    key = D.KEYS?.[sid],
    machineKey = D.SOURCES?.[route.id] ?? route.id,
    paths = { home: '/', analytics: '/analytics', sessions: '/sessions', machines: '/machines' },
    destination =
      route.v === 'session'
        ? '/session/' + routeSegment(source ?? 'local') + '/' + routeSegment(key)
        : route.v === 'trace'
          ? '/session/' +
            routeSegment(source ?? 'local') +
            '/' +
            routeSegment(key) +
            '/trace/' +
            routeSegment(route.turn)
          : route.v === 'machine'
            ? '/machines/' + encodeURIComponent(machineKey)
            : paths[route.v];
  if (['session', 'trace'].includes(route.v)) {
    if (!session) throw new Error(`Unknown session fixture: ${String(sid)}`);
    if (!source) throw new Error(`Missing source mapping for session fixture: ${String(sid)}`);
    if (!key) throw new Error(`Missing catalog-key mapping for session fixture: ${String(sid)}`);
  }
  const url = new URL(destination, page.url());
  if (route.v === 'session' && route.turn) url.hash = routeSegment(route.turn);
  await page.evaluate(
    ({ r, url }) => {
      history.pushState(r, '', url);
      dispatchEvent(new PopStateEvent('popstate', { state: r }));
    },
    {
      r:
        route.v === 'machine'
          ? { ...route, id: machineKey }
          : ['session', 'trace'].includes(route.v)
            ? { ...route, ...(route.id ? { id: key } : { sid: key }) }
            : route,
      url: url.pathname + url.search + url.hash,
    },
  );
  // The root destination opens Sessions.
  const title = route.v === 'home' ? 'Sessions' : titleOf(route, D);
  // A session's title is in the bar as soon as it is opened; its page is ready once it is no longer aria-busy.
  if (title) {
    try {
      await page.waitForFunction(
        (t) =>
          document.querySelector('#topbar .t')?.textContent === t &&
          !document.querySelector('#page').hasAttribute('aria-busy') &&
          !document.querySelector('#page [aria-busy="true"]'),
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
      () =>
        !!document.querySelector("#page section[aria-label='Transcript']") &&
        !!document.querySelector('#page [data-entry-key]'),
    );
  // Analytics draws whole once the server's answer for its range is in (/api/analytics).
  if (route.v === 'analytics')
    await page.waitForFunction(() =>
      document.querySelector('.analytics-metrics[data-analytics-ready]'),
    );
  if (route.v === 'trace') {
    try {
      await page.waitForFunction(() => document.querySelector('#page .trace-summary'));
    } catch (error) {
      throw Error(
        'Trace ' +
          sid +
          ' turn ' +
          route.turn +
          ': ' +
          (await page.locator('#page').innerText()) +
          '; ' +
          error.message,
      );
    }
  }
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(60);
}

// Test expectations in the reference fixture's shapes, assembled exclusively
// from source-qualified native publications and generation-fenced body reads.
export async function fixtureRequest(base, token) {
  const landing = await fetch(base + '/?t=' + token, { redirect: 'manual' });
  const cookie = landing.headers
    .getSetCookie()
    .map((value) => value.split(';')[0])
    .join('; ');
  return (path) => fetch(base + path, { headers: cookie ? { cookie } : {} });
}

async function waitForFixtureRead(request, path, ready, timeout = 10_000) {
  const deadline = Date.now() + timeout;
  let last;
  do {
    const response = await request(path);
    const value = await response.json();
    if (response.ok && ready(value)) return value;
    last = { status: response.status, observations: value.observations, error: value.error };
    if (!response.ok && ![404, 409, 503].includes(response.status)) break;
    await new Promise((resolve) => setTimeout(resolve, 50));
  } while (Date.now() < deadline);
  throw Error('Fixture publication witness missing: ' + path + ' ' + JSON.stringify(last));
}

export function waitForFixtureHome(request) {
  return waitForFixtureRead(
    request,
    '/api/viewer/home',
    (home) => home.observations?.length === 0 && home.recent?.length > 0,
  );
}

export function waitForFixtureTranscript(request, machine, sid, witness, timeout) {
  const query = new URLSearchParams({ machine, sid, scope: 'current' });
  return waitForFixtureRead(
    request,
    '/api/session-transcript?' + query,
    (page) =>
      page.projection === 7 &&
      page.source_key === machine &&
      page.catalog_key === sid &&
      page.read_scope === 'current' &&
      page.session?.key === sid &&
      page.entries.some((entry) => entry.address && witness(entry)),
    timeout,
  );
}

// Exhaustive fixture census is test work; production readers remain selected.
export async function data(options = {}) {
  // Reacquire the entire census when its qualified observation changes.
  for (let attempt = 0; ; attempt++) {
    try {
      return await nativeFixtureData(options);
    } catch (error) {
      if (attempt >= 2 || error.cause?.nativeObservationChanged !== true) throw error;
    }
  }
}
async function nativeFixtureData({ extras = false, base: sourceBase, token: sourceToken } = {}) {
  const base = sourceBase ?? (extras ? ENV.extraBase : ENV.base),
    token = sourceToken ?? (extras ? ENV.extraToken : ENV.token);
  const request = await fixtureRequest(base, token);
  const overview = await waitForFixtureHome(request);
  const get = async (path) => {
    const response = await request(path);
    const value = await response.json();
    if (!response.ok)
      throw Error('Native fixture ' + path + ': ' + response.status + ' ' + JSON.stringify(value), {
        cause: { nativeObservationChanged: response.status === 409 && value.error?.retryable },
      });
    return value;
  };
  const inventory = [];
  let cursor = null;
  do {
    const page = await get(
      '/api/session-sources' + (cursor ? '?cursor=' + encodeURIComponent(cursor) : ''),
    );
    inventory.push(...page.items);
    cursor = page.next_cursor;
  } while (cursor);
  const labelCounts = new Map();
  for (const source of inventory)
    labelCounts.set(source.label, (labelCounts.get(source.label) ?? 0) + 1);
  // Reference machine IDs are presentation labels. Keep source authority in
  // SOURCES rather than using internal directory keys in filenames/routes.
  const machineIds = new Map(
    inventory.map((source, index) => [
      source.source_key,
      labelCounts.get(source.label) === 1 ? source.label : source.label + '@' + (index + 1),
    ]),
  );
  const records = [],
    nativeKeys = new Set();
  for (const source of inventory) {
    cursor = null;
    do {
      const query = new URLSearchParams({
        machine: source.source_key,
        scope: 'current',
        limit: '100',
      });
      if (cursor) query.set('cursor', cursor);
      const page = await waitForFixtureRead(
        request,
        '/api/sessions?' + query,
        (value) => value.completeness?.state === 'complete',
      );
      if (page.machine !== source.source_key) throw Error('Native fixture Catalog source changed');
      for (const item of page.items) {
        nativeKeys.add(item.key);
        records.push({ source, item });
      }
      cursor = page.next_cursor;
    } while (cursor);
  }
  const expected = Object.keys(sample().SESS);
  if (extras) expected.push('attach', 'backlog', 'bgcmd', 'archive');
  const missing = expected.filter((key) => !nativeKeys.has(key));
  if (missing.length) throw Error('Current fixture Catalog missing keys: ' + missing.join(', '));
  records.sort(
    (a, b) =>
      a.item.key.localeCompare(b.item.key) ||
      a.source.source_key.localeCompare(b.source.source_key),
  );
  const counts = new Map();
  for (const { item } of records) counts.set(item.key, (counts.get(item.key) ?? 0) + 1);
  const ids = new Map(
    records.map(({ source, item }) => [
      JSON.stringify([source.source_key, item.key]),
      counts.get(item.key) === 1 ? item.key : item.key + '@' + machineIds.get(source.source_key),
    ]),
  );
  const qualify = (source, key) =>
    key == null || key === 'you' ? key : (ids.get(JSON.stringify([source, key])) ?? key);
  const SESS = {},
    TX = {},
    TXM = {},
    turns = new Map(),
    handoffs = new Map();
  const headers = new Map(),
    previews = new Map();
  const copyPresent = (value) =>
    Object.fromEntries(Object.entries(value).filter(([, field]) => field !== null));
  for (const { source, item } of records) {
    const sid = qualify(source.source_key, item.key),
      entries = [];
    const windows = [],
      cursors = new Set(),
      entryIds = new Set();
    let cursor = null,
      latest;
    do {
      const query = new URLSearchParams({
        machine: source.source_key,
        scope: 'current',
        sid: item.key,
      });
      if (cursor !== null) query.set('cursor', cursor);
      const page = await get('/api/session-transcript?' + query);
      latest ??= page;
      if (
        page.projection !== 7 ||
        page.identity.source_key !== source.source_key ||
        page.identity.catalog_key !== item.key ||
        page.identity.read_scope !== 'current' ||
        page.identity.generation !== latest.identity.generation ||
        page.binding !== latest.binding ||
        page.position !== (cursor === null ? 'latest' : 'earlier') ||
        (page.earlier_cursor !== null &&
          (typeof page.earlier_cursor !== 'string' || cursors.has(page.earlier_cursor)))
      )
        throw Error('Native fixture transcript identity, generation or cursor changed: ' + sid);
      for (const entry of page.entries) {
        if (
          !/^[a-f0-9]{64}$/.test(entry.entry_id) ||
          entry.address?.entry_id !== entry.entry_id ||
          entry.address.binding !== page.binding ||
          entryIds.has(entry.entry_id)
        )
          throw Error('Native fixture transcript entry address missing or repeated: ' + sid);
        entryIds.add(entry.entry_id);
      }
      windows.push(page);
      cursor = page.earlier_cursor;
      if (cursor !== null) cursors.add(cursor);
    } while (cursor !== null);
    for (const page of windows.reverse()) {
      const context = page.relationship_context;
      for (const [key, header] of Object.entries(context.sessions))
        headers.set(JSON.stringify([source.source_key, key]), header);
      for (const [key, version] of Object.entries(context.previews))
        previews.set(JSON.stringify([source.source_key, key]), version);
      for (const turn of context.turns) {
        const key = JSON.stringify([source.source_key, turn.id]);
        // Earlier windows carry the recorded boundary. Later continuations may
        // carry only an incomplete turn header.
        if (!turns.has(key) || turn.at != null)
          turns.set(key, { ...copyPresent(turn), sid: qualify(source.source_key, turn.sid) });
      }
      for (const edge of context.handoffs)
        handoffs.set(JSON.stringify([source.source_key, edge.id]), {
          ...copyPresent(edge),
          from: qualify(source.source_key, edge.from),
          to: qualify(source.source_key, edge.to),
        });
      for (const entry of page.entries)
        entries.push({
          ...entry,
          slot: entries.length,
          ...(entry.ret && { ret: { ...entry.ret, to: qualify(source.source_key, entry.ret.to) } }),
        });
    }
    const details = latest.session?.details;
    const header = headers.get(JSON.stringify([source.source_key, item.key]));
    if (!details) throw Error('Native fixture selected metadata unavailable: ' + sid);
    SESS[sid] = {
      ...copyPresent(item),
      ...copyPresent(header ?? {}),
      ...copyPresent(details),
      id: sid,
      machine: machineIds.get(source.source_key),
      parent: item.parent == null ? undefined : qualify(source.source_key, item.parent),
      kind: item.presentation_kind ?? undefined,
    };
    previews.set(JSON.stringify([source.source_key, item.key]), latest.identity.generation);
    // The reference census uses the supported default 24-hour observation
    // window. Native retained evidence itself remains complete.
    const cutoff = overview.now - 24 * 60 * 60 * 1000;
    SESS[sid].busy = (details.busy ?? [])
      .filter(([, end]) => end >= cutoff)
      .map(([start, end]) => [Math.max(start, cutoff), end]);
    TX[sid] = entries;
    TXM[sid] = { calls: details.calls, errors: details.errors };
  }
  // Native publications hold addresses, never prompt/answer bodies. Resolve
  // bounded groups through their original owner and content generation.
  for (const { source, item } of records) {
    const address = JSON.stringify([source.source_key, item.key]),
      generation = previews.get(address);
    if (!generation) throw Error('Native fixture content generation unavailable: ' + item.key);
    const edges = [...handoffs.entries()].filter(([key, edge]) => {
      const [owner] = JSON.parse(key);
      return (
        owner === source.source_key &&
        (edge.brief_session === item.key || edge.result_session === item.key)
      );
    });
    const ownerTurns = [...turns.entries()].filter(
      ([key, turn]) =>
        JSON.parse(key)[0] === source.source_key &&
        turn.sid === qualify(source.source_key, item.key),
    );
    const selections = [
      ...edges.map(([key, edge]) => ({ key, event: edge.id })),
      ...ownerTurns.map(([key, turn]) => ({ key, turn: turn.id })),
    ];
    for (let start = 0; start < Math.max(1, selections.length); start += 8) {
      const group = selections.slice(start, start + 8);
      const query = new URLSearchParams({
        machine: source.source_key,
        scope: 'current',
        sid: item.key,
        generation,
        events: JSON.stringify(group.flatMap((value) => (value.event ? [value.event] : []))),
        turns: JSON.stringify(group.flatMap((value) => (value.turn ? [value.turn] : []))),
      });
      const detail = await get('/api/session-home?' + query);
      if (
        detail.identity.source_key !== source.source_key ||
        detail.identity.catalog_key !== item.key ||
        detail.identity.read_scope !== 'current' ||
        detail.generation !== generation ||
        detail.incomplete
      )
        throw Error(
          'Native fixture preview identity, generation or completeness changed: ' + item.key,
        );
      const activity = detail.session.activity;
      SESS[qualify(source.source_key, item.key)].activity =
        activity == null
          ? undefined
          : [
              activity[0],
              activity[1],
              Math.floor((overview.now - activity[3]) / 1000),
              activity[3],
            ];
      for (const value of group) {
        if (value.turn) {
          if (!(value.turn in detail.turns))
            throw Error('Native fixture turn prompt unavailable: ' + value.turn);
          turns.get(value.key).text = detail.turns[value.turn];
        } else {
          const edge = handoffs.get(value.key),
            text = detail.events[value.event];
          if (!text) throw Error('Native fixture handoff text unavailable: ' + value.event);
          if (edge.brief_session === item.key) {
            edge.brief = text.brief;
            if (text.answer != null) edge.answer = text.answer;
          }
          if (edge.result_session === item.key && text.result != null) edge.result = text.result;
        }
      }
    }
  }
  const machines = overview.machines.map((source) => ({
    id: machineIds.get(source.key),
    name: source.label,
    up: source.up,
    last: source.last,
  }));
  // Home owns current wait observations that need not belong to a transcript
  // turn. Borrow those published events; do not infer them from tool text.
  for (const row of [...overview.inbox, ...overview.answered]) {
    const key = JSON.stringify([row.source_key, row.event.id]);
    if (!handoffs.has(key))
      handoffs.set(key, {
        ...copyPresent(row.event),
        kind: 'toyou',
        from: qualify(row.source_key, row.catalog_key),
        to: 'you',
      });
  }
  const H = [...handoffs.values()].sort(
      (a, b) => a.at - b.at || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0),
    ),
    nativeTurns = [...turns.values()];
  const model = {
    sessions: SESS,
    handoffs: H,
    turns: nativeTurns,
    now: overview.now,
    machine: machines[0],
    machines,
  };
  return {
    model,
    overview,
    SESS,
    H,
    TX,
    TXM,
    nativeKeys,
    turns: nativeTurns,
    NOW: overview.now,
    SOURCES: Object.fromEntries(
      inventory.map((source) => [machineIds.get(source.source_key), source.source_key]),
    ),
    KEYS: Object.fromEntries(
      records.map(({ source, item }) => [qualify(source.source_key, item.key), item.key]),
    ),
    MACHINE: Object.fromEntries(machines.map((machine) => [machine.id, machine.name])),
    MACHINE_UP: Object.fromEntries(machines.map((machine) => [machine.id, machine.up])),
    MACHINE_LAST: Object.fromEntries(
      machines
        .filter((machine) => machine.last != null)
        .map((machine) => [machine.id, machine.last]),
    ),
    ADMIN: null,
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
