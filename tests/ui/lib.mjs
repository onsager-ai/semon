// Shared by the browser checks: the served viewer's address, pages opened on it, the served data in the mockup's shapes,
// and the report every check writes. Environment:
//   SEMON_BASE   http://127.0.0.1:PORT of a running `semon sessions --serve`
//   SEMON_TOKEN  its token (the ?t= it printed)
//   SEMON_NOW    the fixture's pinned now (epoch ms): the browser's clock stands there too
//   SEMON_UI_OUT where reports, screenshots and diffs go (default: ./out)
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
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

export const launch = ({ baselineRaster = false } = {}) =>
  chromium.launch({
    args: [
      '--disable-gpu',
      '--font-render-hinting=none',
      // Deterministic rendering stages, while Playwright keeps normal frame
      // scheduling (enable-begin-frame-control requires a separate CDP driver).
      ...(baselineRaster
        ? [
            '--disable-skia-runtime-opts',
            '--run-all-compositor-stages-before-draw',
            '--disable-new-content-rendering-timeout',
            '--disable-threaded-animation',
            '--disable-threaded-scrolling',
            '--disable-checker-imaging',
            '--disable-image-animation-resync',
            '--disable-partial-raster',
          ]
        : []),
    ],
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

const CAPTURE_STABLE_MS = 200;
const CAPTURE_TIMEOUT_MS = 8000;
const CAPTURE_API_EVENT_LIMIT = 2000;
const HOME_POST_TAG_HEADER = 'x-semon-capture-id';
const HOME_POST_TAG_PATTERN = /^[0-9a-f]{16}-[1-9][0-9]{0,7}$/;
const HOME_POST_TAG_LIMIT = 128;
const CAPTURE_GENERATION_QUERY_KEYS = new Set(['generation', 'errors_generation']);
// Fail only selected content reads. Optional observations and native controls
// remain in the raw event log and retain their own existing suite assertions.
const SELECTED_READ_APIS = {
  candidate: new Set([
    '/api/session-anchor',
    '/api/session-annotations',
    '/api/session-capabilities',
    '/api/session-entry',
    '/api/session-home',
    '/api/session-identity',
    '/api/session-locate',
    '/api/session-source-candidate',
    '/api/session-source-candidates',
    '/api/session-sources',
    '/api/session-trace',
    '/api/session-transcript',
    '/api/sessions',
    '/api/viewer/chrome',
    '/api/viewer/analytics',
    '/api/viewer/home',
  ]),
  reference: new Set(['/api/analytics', '/api/model', '/api/tx']),
};

function selectedApiPath(url, origin) {
  try {
    const parsed = new URL(url);
    return parsed.origin === origin && parsed.pathname.startsWith('/api/') ? parsed.pathname : null;
  } catch {
    return null;
  }
}

function selectedApiQueryDigest(url, origin, { retryScope = false } = {}) {
  const parsed = new URL(url);
  if (parsed.origin !== origin) return null;
  const query = new URLSearchParams(
    [...parsed.searchParams.entries()]
      .filter(([key]) => key !== 't' && !(retryScope && CAPTURE_GENERATION_QUERY_KEYS.has(key)))
      .sort(([a], [b]) => a.localeCompare(b)),
  ).toString();
  return createHash('sha256').update(query).digest('hex');
}

function recordCaptureApiEvent(page, event) {
  if (page.captureApiEvents.length < CAPTURE_API_EVENT_LIMIT) page.captureApiEvents.push(event);
  else page.captureApiEventsTruncated++;
}

function homePostTagFromRequest(request) {
  if (request.method() !== 'POST') return { id: null, present: false };
  let url;
  try {
    url = new URL(request.url());
  } catch {
    return { id: null, present: false };
  }
  if (url.pathname !== '/api/viewer/home') return { id: null, present: false };
  try {
    const headers = request.headers(),
      entry = Object.entries(headers).find(([name]) => name.toLowerCase() === HOME_POST_TAG_HEADER),
      value = entry?.[1];
    return {
      id: typeof value === 'string' && HOME_POST_TAG_PATTERN.test(value) ? value : null,
      present: !!entry,
    };
  } catch {
    return { id: null, present: false };
  }
}

function validCaptureRouteMarker(marker) {
  if (typeof marker !== 'string' || !marker.length || marker.length > 512) return false;
  try {
    const route = JSON.parse(marker);
    if (!route || typeof route !== 'object' || Array.isArray(route)) return false;
    if (
      !['home', 'analytics', 'sessions', 'machines', 'machine', 'session', 'trace'].includes(
        route.v,
      )
    )
      return false;
    if (route.v === 'machine') return typeof route.id === 'string' && !!route.id;
    if (route.v === 'session')
      return typeof (route.id ?? route.sid) === 'string' && !!(route.id ?? route.sid);
    if (route.v === 'trace')
      return (
        typeof (route.id ?? route.sid) === 'string' &&
        !!(route.id ?? route.sid) &&
        typeof route.turn === 'string' &&
        !!route.turn
      );
    return true;
  } catch {
    return false;
  }
}

// Exact tag evidence is diagnostic only unless the current navigation proves
// that the invocation came from its just-departed, already accepted route.
export function homePostTagDiagnostics(page) {
  if (!page.captureHomePostTagEnabled) return null;
  const invocations = page.captureHomePostTagInvocations ?? [],
    requests = page.captureHomePostTagRequests ?? [],
    invocationCounts = new Map(),
    requestCounts = new Map(),
    requestIdCounts = new Map(),
    retentionComplete =
      (page.captureHomePostTagInvocationsTruncated ?? 0) === 0 &&
      (page.captureHomePostTagRequestsTruncated ?? 0) === 0 &&
      (page.captureApiEventsTruncated ?? 0) === 0;
  for (const invocation of invocations)
    invocationCounts.set(invocation.id, (invocationCounts.get(invocation.id) ?? 0) + 1);
  for (const request of requests) {
    if (request.tagId)
      requestCounts.set(request.tagId, (requestCounts.get(request.tagId) ?? 0) + 1);
    requestIdCounts.set(request.requestId, (requestIdCounts.get(request.requestId) ?? 0) + 1);
  }
  return {
    scope:
      'candidate same-origin POST /api/viewer/home; string/URL input, plain init and string body',
    retentionComplete,
    invocations,
    invocationsTruncated: page.captureHomePostTagInvocationsTruncated ?? 0,
    requests: requests.map((request) => {
      const matching = invocations.filter((invocation) => invocation.id === request.tagId),
        allRawEvents = (page.captureApiEvents ?? []).filter(
          (event) => event.id === request.requestId,
        ),
        rawEvents = allRawEvents.slice(0, 8),
        rawStarts = rawEvents.filter((event) => event.event === 'started');
      const exact =
        retentionComplete &&
        allRawEvents.length <= 8 &&
        Number.isSafeInteger(request.requestId) &&
        request.requestId > 0 &&
        requestIdCounts.get(request.requestId) === 1 &&
        typeof request.tagId === 'string' &&
        HOME_POST_TAG_PATTERN.test(request.tagId) &&
        request.tagHeaderPresent === true &&
        requestCounts.get(request.tagId) === 1 &&
        invocationCounts.get(request.tagId) === 1 &&
        matching.length === 1 &&
        rawStarts.length === 1 &&
        rawStarts[0].method === 'POST' &&
        rawStarts[0].path === '/api/viewer/home' &&
        rawStarts[0].homePostTagHeaderPresent === true &&
        rawStarts[0].homePostTagId === request.tagId &&
        rawStarts[0].routeKey === request.routeKey &&
        validCaptureRouteMarker(matching[0].routeMarker);
      return {
        requestId: request.requestId,
        tagId: request.tagId,
        tagHeaderPresent: request.tagHeaderPresent,
        nodeRouteKey: request.routeKey,
        association: exact ? 'unique-exact-tag' : 'unknown',
        invocationRouteMarker: exact ? matching[0].routeMarker : null,
        rawEventsTruncated: allRawEvents.length > 8,
        rawEvents,
      };
    }),
    requestsTruncated: page.captureHomePostTagRequestsTruncated ?? 0,
    apiEventsTruncated: page.captureApiEventsTruncated ?? 0,
    priorAbortOwners: [...(page.captureHomePostOwnerProofs?.values() ?? [])],
  };
}

export function resolveHomePostPriorAbortOwners(page, routeKey = page.captureRouteKey) {
  const owners = new Map(),
    transition = page.captureHomePostRouteTransition,
    previous = transition?.from;
  if (
    !page.captureHomePostTagEnabled ||
    page.captureRole !== 'candidate' ||
    page.captureHomePostTagFlushComplete !== true ||
    !transition ||
    transition.fromAccepted !== true ||
    transition.to !== routeKey ||
    page.captureRouteKey !== routeKey ||
    previous === routeKey ||
    !validCaptureRouteMarker(previous) ||
    !validCaptureRouteMarker(routeKey)
  )
    return owners;
  const diagnostics = homePostTagDiagnostics(page);
  if (!diagnostics?.retentionComplete) return owners;
  for (const request of diagnostics.requests) {
    const rawEvents = request.rawEvents ?? [],
      rawStarts = rawEvents.filter((event) => event.event === 'started'),
      failures = rawEvents.filter((event) => event.event === 'failed'),
      hasNon2xxResponse = rawEvents.some(
        (event) => event.event === 'response' && !(event.status >= 200 && event.status < 300),
      );
    if (
      request.association !== 'unique-exact-tag' ||
      request.nodeRouteKey !== routeKey ||
      request.invocationRouteMarker !== previous ||
      rawStarts.length !== 1 ||
      rawStarts[0].routeKey !== routeKey ||
      failures.length !== 1 ||
      failures[0].method !== 'POST' ||
      failures[0].path !== '/api/viewer/home' ||
      failures[0].routeKey !== routeKey ||
      failures[0].homePostTagHeaderPresent !== true ||
      failures[0].homePostTagId !== request.tagId ||
      failures[0].error !== 'net::ERR_ABORTED' ||
      hasNon2xxResponse
    )
      continue;
    owners.set(request.requestId, {
      ownerRouteKey: previous,
      tagId: request.tagId,
      invocationRouteMarker: request.invocationRouteMarker,
      nodeRouteKey: request.nodeRouteKey,
    });
  }
  return owners;
}

function rememberHomePostPriorAbortOwners(page, owners) {
  page.captureHomePostOwners = owners;
  const proofs = page.captureHomePostOwnerProofs ?? new Map();
  for (const [requestId, owner] of owners)
    proofs.set(requestId, {
      requestId,
      tagId: owner.tagId,
      nodeRouteKey: owner.nodeRouteKey,
      invocationRouteMarker: owner.invocationRouteMarker,
      ownerRouteKey: owner.ownerRouteKey,
      error: 'net::ERR_ABORTED',
    });
  page.captureHomePostOwnerProofs = proofs;
}

// Node prepares the next capture before the browser dispatches its navigation.
// A previous Trace poll can start in that protocol gap. Bind it only to an
// already completed exact prior query; unmatched/current requests stay current.
export function captureRequestOwnership(page, tracked) {
  const transition = page.captureRouteTransition;
  if (
    page.captureRole !== 'candidate' ||
    !transition ||
    transition.from === transition.to ||
    tracked.routeKey !== transition.to ||
    tracked.method !== 'GET' ||
    tracked.path !== '/api/session-trace' ||
    !transition.expectedQueryDigest ||
    tracked.queryDigest === transition.expectedQueryDigest
  )
    return { routeKey: tracked.routeKey };
  const previous = page.captureApiEvents.findLast(
    (event) =>
      event.event === 'response' &&
      event.status === 200 &&
      event.ok &&
      event.id < tracked.id &&
      event.method === tracked.method &&
      event.path === tracked.path &&
      event.routeKey === transition.from &&
      event.queryDigest === tracked.queryDigest &&
      page.captureApiEvents.some(
        (finished) =>
          finished.event === 'finished' &&
          finished.id === event.id &&
          finished.method === event.method &&
          finished.path === event.path &&
          finished.routeKey === event.routeKey &&
          finished.queryDigest === event.queryDigest &&
          !finished.error,
      ),
  );
  return previous
    ? {
        routeKey: transition.from,
        captureRouteKeyAtStart: tracked.routeKey,
        ownerAttribution: {
          kind: 'completed-prior-trace-during-navigation',
          responseId: previous.id,
          expectedQueryDigest: transition.expectedQueryDigest,
        },
      }
    : { routeKey: tracked.routeKey };
}

export function failedSelectedApiEvents(page, routeKey = page.captureRouteKey) {
  const events = page.captureApiEvents.filter((event) => event.routeKey === routeKey),
    selected = SELECTED_READ_APIS[page.captureRole] ?? new Set();
  return events.filter((event, eventIndex) => {
    if (!selected.has(event.path)) return false;
    if (event.event === 'response') {
      if (event.ok || event.status === 304) return false;
      if (event.status === 409)
        return !events.some(
          (later) =>
            later.event === 'response' &&
            (later.ok || later.status === 304) &&
            later.path === event.path &&
            later.method === event.method &&
            later.retryScopeDigest === event.retryScopeDigest &&
            later.routeKey === event.routeKey &&
            later.id > event.id,
        );
      return true;
    }
    if (event.event !== 'failed') return false;
    const priorHomeOwner = page.captureHomePostOwners?.get(event.id);
    if (
      page.captureRole === 'candidate' &&
      event.method === 'POST' &&
      event.path === '/api/viewer/home' &&
      event.error === 'net::ERR_ABORTED' &&
      priorHomeOwner &&
      priorHomeOwner.ownerRouteKey !== event.routeKey &&
      event.routeKey === page.captureRouteKey
    )
      return false;
    // Chromium can report the reference poll's bodyless 304 as ERR_ABORTED.
    // The pinned transport explicitly accepts that status without reading a
    // body. Require the same recorded request, never a later unrelated success.
    if (
      page.captureRole === 'reference' &&
      event.method === 'GET' &&
      event.path === '/api/model' &&
      /ERR_ABORTED/.test(event.error ?? '') &&
      events.some(
        (response, responseIndex) =>
          responseIndex < eventIndex &&
          response.event === 'response' &&
          response.id === event.id &&
          response.method === event.method &&
          response.path === event.path &&
          response.routeKey === event.routeKey &&
          response.queryDigest === event.queryDigest &&
          response.status === 304 &&
          response.at <= event.at,
      )
    )
      return false;
    return !(
      /ERR_ABORTED|aborted|cancelled/i.test(event.error ?? '') &&
      event.routeKey !== page.captureRouteKey
    );
  });
}

function waitForSelectedApiIdle(page, timeoutMs) {
  if (!page.inFlightApiRequests.size) return Promise.resolve();
  return new Promise((resolve, reject) => {
    let finished = false;
    const cleanup = () => {
      if (finished) return;
      finished = true;
      clearTimeout(timer);
      page.captureApiIdleWaiters.delete(onIdle);
    };
    const onIdle = () => {
      cleanup();
      resolve();
    };
    const timer = setTimeout(() => {
      cleanup();
      reject(new Error('selected API requests did not become idle'));
    }, timeoutMs);
    page.captureApiIdleWaiters.add(onIdle);
    if (!page.inFlightApiRequests.size) onIdle();
  });
}

async function flushHomePostTagBindings(page, timeoutMs) {
  if (!page.captureHomePostTagEnabled) return true;
  let timer;
  try {
    return await Promise.race([
      page
        .evaluate(async (remainingMs) => {
          const flush = window.__semonFlushHomePostInvocationBindings;
          if (typeof flush !== 'function') return false;
          let timer;
          const result = await Promise.race([
            Promise.resolve(flush()).then(
              () => true,
              () => false,
            ),
            new Promise((resolve) => {
              timer = setTimeout(() => resolve(false), Math.max(1, remainingMs));
            }),
          ]);
          if (timer) clearTimeout(timer);
          return result;
        }, timeoutMs)
        .catch(() => false),
      new Promise((resolve) => {
        timer = setTimeout(() => resolve(false), Math.max(1, timeoutMs));
      }),
    ]);
  } catch {
    return false;
  } finally {
    if (timer) clearTimeout(timer);
  }
}

async function waitForStableCaptureDom(page, timeoutMs) {
  return page.evaluate(
    ({ stableMs, timeoutMs }) =>
      new Promise((resolve, reject) => {
        const state = window.__semonCaptureReadiness;
        if (!state) {
          reject(new Error('capture mutation observer was not installed'));
          return;
        }
        let stableTimer;
        const clearStableTimer = () => {
          if (stableTimer) clearTimeout(stableTimer);
        };
        const inspect = () => {
          const panel = document.querySelector('#page'),
            loading =
              !panel ||
              panel.hasAttribute('aria-busy') ||
              !!panel.querySelector('[aria-busy="true"], [data-loading="true"]'),
            selected =
              !!history.state?.v &&
              !!document.querySelector('#topbar .t, #topbar #find') &&
              !!panel?.childElementCount;
          return {
            selected,
            loading,
            errors: panel
              ? [...panel.querySelectorAll('[role="alert"], [data-error="true"]')]
                  .slice(0, 10)
                  .map((node) => node.textContent.slice(0, 200))
              : [],
            pathname: location.pathname,
            state: history.state?.v ?? null,
            mutationVersion: state.version,
            stableForMs: Math.max(0, performance.now() - state.lastMutationAt),
            viewport: [window.innerWidth, window.innerHeight],
          };
        };
        let complete = false;
        const finish = (error, snapshot) => {
          if (complete) return;
          complete = true;
          clearTimeout(deadline);
          clearStableTimer();
          state.waiters.delete(check);
          if (error) reject(error);
          else resolve(snapshot);
        };
        const check = () => {
          const snapshot = inspect();
          if (!snapshot.selected || snapshot.loading) {
            clearStableTimer();
            return;
          }
          const remaining = stableMs - snapshot.stableForMs;
          if (remaining <= 0) finish(null, snapshot);
          else {
            clearStableTimer();
            stableTimer = setTimeout(check, remaining);
          }
        };
        state.waiters.add(check);
        const deadline = setTimeout(
          () => finish(new Error('selected DOM did not become stable before the capture deadline')),
          timeoutMs,
        );
        check();
      }),
    { stableMs: CAPTURE_STABLE_MS, timeoutMs },
  );
}

export async function waitForCaptureReady(page, label = 'actual UI capture') {
  if (!page.captureReadinessEnabled)
    throw new Error(`${label}: served page did not enable capture request tracking`);
  const started = Date.now(),
    deadline = started + CAPTURE_TIMEOUT_MS;
  let cause;
  try {
    while (Date.now() < deadline) {
      await waitForSelectedApiIdle(page, Math.max(1, deadline - Date.now()));
      const quietFor = Date.now() - page.captureApiActivityAt;
      if (quietFor < CAPTURE_STABLE_MS)
        await new Promise((resolve) =>
          setTimeout(resolve, Math.min(deadline - Date.now(), CAPTURE_STABLE_MS - quietFor)),
        );
      const dom = await waitForStableCaptureDom(page, Math.max(1, deadline - Date.now()));
      if (
        page.inFlightApiRequests.size ||
        Date.now() - page.captureApiActivityAt < CAPTURE_STABLE_MS ||
        dom.stableForMs < CAPTURE_STABLE_MS
      )
        continue;
      if (page.captureHomePostTagEnabled) {
        const remaining = deadline - Date.now();
        page.captureHomePostTagFlushComplete =
          remaining > 0 && (await flushHomePostTagBindings(page, remaining));
        rememberHomePostPriorAbortOwners(
          page,
          page.captureHomePostTagFlushComplete ? resolveHomePostPriorAbortOwners(page) : new Map(),
        );
        if (
          Date.now() >= deadline ||
          page.inFlightApiRequests.size ||
          Date.now() - page.captureApiActivityAt < CAPTURE_STABLE_MS
        )
          continue;
      }
      const pageErrors = page.capturePageErrors.filter(
          (error) => error.routeKey === page.captureRouteKey,
        ),
        apiFailures = failedSelectedApiEvents(page);
      if (page.captureApiEventsTruncated)
        throw new Error(`${label}: API diagnostics truncated at ${CAPTURE_API_EVENT_LIMIT} events`);
      if (pageErrors.length || apiFailures.length || dom.errors.length)
        throw new Error(
          `${label}: selected page/reading errors: ` +
            JSON.stringify({
              pageErrors: pageErrors.slice(0, 10),
              apiFailures: apiFailures.slice(0, 20),
              domErrors: dom.errors.slice(0, 10),
            }),
        );
      page.captureLastReadyRouteKey = page.captureRouteKey;
      return dom;
    }
  } catch (error) {
    cause = error;
  }
  if (cause?.message.startsWith(`${label}: selected page/reading errors:`)) throw cause;
  if (cause?.message.startsWith(`${label}: API diagnostics truncated`)) throw cause;
  const [dom, pending] = await Promise.all([
    page
      .evaluate(() => {
        const panel = document.querySelector('#page'),
          state = window.__semonCaptureReadiness;
        return {
          selected:
            !!history.state?.v &&
            !!document.querySelector('#topbar .t, #topbar #find') &&
            !!panel?.childElementCount,
          loading:
            !panel ||
            panel.hasAttribute('aria-busy') ||
            !!panel.querySelector('[aria-busy="true"], [data-loading="true"]'),
          errors: panel
            ? [...panel.querySelectorAll('[role="alert"], [data-error="true"]')]
                .slice(0, 10)
                .map((node) => node.textContent.slice(0, 200))
            : [],
          pathname: location.pathname,
          state: history.state?.v ?? null,
          mutationVersion: state?.version ?? null,
          stableForMs: state ? Math.max(0, performance.now() - state.lastMutationAt) : 0,
          viewport: [window.innerWidth, window.innerHeight],
        };
      })
      .catch((error) => ({ unavailable: error.message })),
    Promise.resolve([...page.inFlightApiRequests.values()]),
  ]);
  const apiDiagnostics = page.captureApiEvents.slice(-30);
  throw new Error(
    `${label}: capture readiness timed out after ${Date.now() - started}ms; ` +
      `pending selected API=${JSON.stringify(pending)}; ` +
      `API quiet=${Date.now() - page.captureApiActivityAt}ms; DOM=${JSON.stringify(dom)}; ` +
      `page errors=${JSON.stringify(page.capturePageErrors.slice(-10))}; ` +
      `API diagnostics=${JSON.stringify(apiDiagnostics)}; ` +
      `diagnosticsTruncated=${page.captureApiEventsTruncated}; ` +
      `cause=${cause?.message ?? 'readiness deadline elapsed'}`,
    { cause },
  );
}

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
  page.capturePageErrors = [];
  page.on('pageerror', (e) => {
    const message = e.message.split('\n')[0];
    page.errors.push(message);
    if (page.captureReadinessEnabled)
      page.capturePageErrors.push({ message, routeKey: page.captureRouteKey, at: Date.now() });
  });
  if (opts.captureRequests) page.requests = [];
  if (opts.captureReadiness) {
    const origin = new URL(base).origin;
    page.captureReadinessEnabled = true;
    page.captureRole = opts.captureRole ?? 'unknown';
    page.captureRouteKey = 'initial';
    page.captureHomePostTagEnabled =
      opts.captureHomePostTag === true && page.captureRole === 'candidate';
    page.captureHomePostTagInvocations = [];
    page.captureHomePostTagInvocationsTruncated = 0;
    page.captureHomePostTagRequests = [];
    page.captureHomePostTagRequestsTruncated = 0;
    page.captureHomePostOwnerProofs = new Map();
    if (page.captureHomePostTagEnabled && typeof opts.onCaptureHomePostTagPage === 'function')
      opts.onCaptureHomePostTagPage(page);
    page.inFlightApiRequests = new Map();
    page.captureApiIdleWaiters = new Set();
    page.captureApiEvents = [];
    page.captureApiEventsTruncated = 0;
    page.captureApiRequestSequence = 0;
    page.captureApiActivityAt = Date.now();
    if (page.captureHomePostTagEnabled)
      await page.exposeFunction('__semonCaptureHomePostInvocation', (record) => {
        if (
          !record ||
          typeof record.id !== 'string' ||
          !HOME_POST_TAG_PATTERN.test(record.id) ||
          (record.routeMarker !== null &&
            (typeof record.routeMarker !== 'string' || record.routeMarker.length > 512))
        ) {
          page.captureHomePostTagInvocationsTruncated++;
          return;
        }
        if (page.captureHomePostTagInvocations.length < HOME_POST_TAG_LIMIT)
          page.captureHomePostTagInvocations.push({
            id: record.id,
            routeMarker: record.routeMarker,
          });
        else page.captureHomePostTagInvocationsTruncated++;
      });
    page.on('request', (request) => {
      const apiPath = selectedApiPath(request.url(), origin);
      if (page.requests) page.requests.push(request.url());
      if (apiPath) {
        const homePost =
          page.captureHomePostTagEnabled &&
          apiPath === '/api/viewer/home' &&
          request.method() === 'POST'
            ? homePostTagFromRequest(request)
            : null;
        const tracked = {
          id: ++page.captureApiRequestSequence,
          method: request.method(),
          path: apiPath,
          queryDigest: selectedApiQueryDigest(request.url(), origin),
          retryScopeDigest: selectedApiQueryDigest(request.url(), origin, { retryScope: true }),
          routeKey: page.captureRouteKey,
          startedAt: Date.now(),
          ...(homePost
            ? {
                homePostTagId: homePost.id,
                homePostTagHeaderPresent: homePost.present,
              }
            : {}),
        };
        if (homePost) {
          if (page.captureHomePostTagRequests.length < HOME_POST_TAG_LIMIT)
            page.captureHomePostTagRequests.push({
              requestId: tracked.id,
              tagId: homePost.id,
              tagHeaderPresent: homePost.present,
              routeKey: tracked.routeKey,
            });
          else page.captureHomePostTagRequestsTruncated++;
        }
        Object.assign(tracked, captureRequestOwnership(page, tracked));
        page.inFlightApiRequests.set(request, tracked);
        page.captureApiActivityAt = Date.now();
        recordCaptureApiEvent(page, { event: 'started', ...tracked });
      }
    });
    const completed = (request, event, details = {}) => {
      const tracked = page.inFlightApiRequests.get(request);
      if (tracked && page.inFlightApiRequests.delete(request)) {
        page.captureApiActivityAt = Date.now();
        if (event)
          recordCaptureApiEvent(page, {
            event,
            ...tracked,
            ...details,
            at: Date.now(),
            durationMs: Date.now() - tracked.startedAt,
          });
        recordCaptureApiEvent(page, {
          event: 'finished',
          ...tracked,
          ...details,
          at: Date.now(),
          durationMs: Date.now() - tracked.startedAt,
        });
        if (!page.inFlightApiRequests.size)
          for (const resolve of page.captureApiIdleWaiters) resolve();
      }
    };
    page.on('requestfinished', (request) => completed(request));
    page.on('requestfailed', (request) => {
      const tracked = page.inFlightApiRequests.get(request);
      if (tracked) {
        const error = request.failure()?.errorText ?? 'network error';
        recordCaptureApiEvent(page, { event: 'failed', ...tracked, error, at: Date.now() });
        completed(request, null, { error });
      }
    });
    page.on('response', (response) => {
      const apiPath = selectedApiPath(response.url(), origin);
      const tracked = page.inFlightApiRequests.get(response.request());
      if (apiPath && tracked)
        recordCaptureApiEvent(page, {
          event: 'response',
          ...tracked,
          status: response.status(),
          ok: response.ok(),
          at: Date.now(),
        });
    });
    await page.addInitScript((captureHomePostTag) => {
      const state = { version: 0, lastMutationAt: performance.now(), waiters: new Set() },
        observer = new MutationObserver(() => {
          state.version++;
          state.lastMutationAt = performance.now();
          for (const waiter of state.waiters) waiter();
        });
      observer.observe(document, {
        attributes: true,
        childList: true,
        characterData: true,
        subtree: true,
      });
      Object.defineProperty(window, '__semonCaptureReadiness', { value: state });
      if (!captureHomePostTag || window !== window.top) return;
      try {
        const bytes = new Uint8Array(8);
        crypto.getRandomValues(bytes);
        const nonce = [...bytes].map((value) => value.toString(16).padStart(2, '0')).join('');
        let sequence = 0;
        const pendingBindings = new Set(),
          nativeFetch = window.fetch,
          plainRecord = (value) => {
            if (!value || typeof value !== 'object' || Array.isArray(value)) return false;
            const prototype = Object.getPrototypeOf(value);
            if (prototype !== Object.prototype && prototype !== null) return false;
            return Reflect.ownKeys(value).every((key) => {
              const descriptor = Object.getOwnPropertyDescriptor(value, key);
              return !!descriptor && 'value' in descriptor;
            });
          },
          supportedHeaders = (value) => {
            if (value === undefined || value instanceof Headers) return true;
            if (!plainRecord(value)) return false;
            return Reflect.ownKeys(value).every((key) => {
              const descriptor = Object.getOwnPropertyDescriptor(value, key);
              return typeof key === 'string' && typeof descriptor?.value === 'string';
            });
          };
        window.__semonFlushHomePostInvocationBindings = () =>
          Promise.all([...pendingBindings]).then(() => true);
        window.fetch = function (input, init) {
          let prepared;
          try {
            if (
              arguments.length <= 2 &&
              (typeof input === 'string' || input instanceof URL) &&
              (init === undefined || plainRecord(init))
            ) {
              const url = new URL(input instanceof URL ? input.href : input, location.href),
                method = init?.method ?? 'GET';
              if (
                url.origin === location.origin &&
                url.pathname === '/api/viewer/home' &&
                typeof method === 'string' &&
                method.toUpperCase() === 'POST' &&
                typeof init?.body === 'string' &&
                supportedHeaders(init?.headers)
              ) {
                const headers = new Headers(init?.headers);
                if (
                  !headers.has('x-semon-capture-id') &&
                  sequence < 99999999 &&
                  pendingBindings.size < 128
                ) {
                  const descriptors =
                    init === undefined ? {} : Object.getOwnPropertyDescriptors(init);
                  delete descriptors.headers;
                  const taggedInit = Object.create(
                    init === undefined ? Object.prototype : Object.getPrototypeOf(init),
                  );
                  Object.defineProperties(taggedInit, descriptors);
                  const id = `${nonce}-${++sequence}`;
                  headers.set('x-semon-capture-id', id);
                  Object.defineProperty(taggedInit, 'headers', {
                    value: headers,
                    configurable: true,
                    enumerable: true,
                    writable: true,
                  });
                  prepared = {
                    taggedInit,
                    record: {
                      id,
                      routeMarker:
                        typeof window.__semonCaptureRouteMarker === 'string'
                          ? window.__semonCaptureRouteMarker
                          : null,
                    },
                  };
                }
              }
            }
          } catch {}
          if (!prepared) return Reflect.apply(nativeFetch, this, arguments);
          const binding = window.__semonCaptureHomePostInvocation;
          if (typeof binding !== 'function') return Reflect.apply(nativeFetch, this, arguments);
          try {
            const recordPromise = Promise.resolve(binding(prepared.record)).catch(() => {});
            let pending;
            pending = recordPromise.then(() => pendingBindings.delete(pending));
            pendingBindings.add(pending);
          } catch {
            return Reflect.apply(nativeFetch, this, arguments);
          }
          return Reflect.apply(nativeFetch, this, [input, prepared.taggedInit]);
        };
      } catch {}
    }, page.captureHomePostTagEnabled);
  } else if (opts.captureRequests) {
    page.on('request', (request) => page.requests.push(request.url()));
  }
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
        .catch((diagnosticError) => ({ unavailable: diagnosticError.message })),
      pending = [...(page.inFlightApiRequests?.values() ?? [])];
    throw new Error(
      `initial viewer load did not settle; DOM=${JSON.stringify(dom)}; ` +
        `pending selected API=${JSON.stringify(pending)}; ` +
        `page errors=${JSON.stringify(page.capturePageErrors ?? page.errors)}; ` +
        `API diagnostics=${JSON.stringify(page.captureApiEvents?.slice(-30) ?? [])}; ` +
        `cause=${error.message}`,
      { cause: error },
    );
  }
  if (opts.captureReadiness) {
    // Keep pending initial reads in the same readiness transaction until they
    // finish; changing its key early would lose late failures or valid recovery.
    await waitForCaptureReady(page, 'initial viewer load');
    page.captureRouteKey = await page.evaluate(() => {
      const marker = JSON.stringify(history.state ?? { v: 'unknown' });
      if (typeof window.__semonCaptureHomePostInvocation === 'function')
        window.__semonCaptureRouteMarker = marker;
      return marker;
    });
    page.captureLastReadyRouteKey = page.captureRouteKey;
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
    source = session ? (D.SOURCES?.[session.machine] ?? session.machine) : undefined,
    key = D.KEYS?.[sid] ?? sid,
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
  const url = new URL(destination, page.url());
  if (route.v === 'session' && route.turn) url.hash = routeSegment(route.turn);
  const current = JSON.stringify(route);
  if (page.captureReadinessEnabled) {
    const previous = page.captureRouteKey;
    if (page.captureHomePostTagEnabled)
      page.captureHomePostRouteTransition = {
        from: previous,
        to: current,
        fromAccepted: page.captureLastReadyRouteKey === previous,
      };
    let previousRoute;
    try {
      previousRoute = JSON.parse(previous);
    } catch {}
    const traceQuery = new URLSearchParams({
      machine: source ?? '',
      scope: 'retained_history',
      sid: key ?? '',
      turn: route.turn ?? '',
    });
    page.captureRouteTransition =
      page.captureRole === 'candidate' &&
      previousRoute?.v === 'trace' &&
      route.v === 'trace' &&
      source
        ? {
            from: previous,
            to: current,
            expectedQueryDigest: selectedApiQueryDigest(
              new URL('/api/session-trace?' + traceQuery, page.url()).href,
              new URL(page.url()).origin,
            ),
          }
        : null;
    page.captureRouteKey = current;
  }
  try {
    await page.evaluate(
      ({ r, url, marker }) => {
        if (typeof window.__semonCaptureHomePostInvocation === 'function')
          window.__semonCaptureRouteMarker = marker;
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
        marker: current,
      },
    );
  } finally {
    page.captureRouteTransition = null;
  }
  // Historical comparators still open Home on the independent reference.
  // On the candidate that same root destination now opens Sessions.
  const title =
    route.v === 'home' && page.captureRole !== 'reference' ? 'Sessions' : titleOf(route, D);
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
  if (page.captureReadinessEnabled) await waitForCaptureReady(page, `route ${route.v}`);
  else await page.waitForTimeout(60);
}

// Test expectations in the reference fixture's shapes, assembled exclusively
// from source-qualified native publications and generation-fenced body reads.
// Exhaustive fixture census is test work; production readers remain selected.
export async function data(options = {}) {
  // A fixture census spans several source-qualified publications. If refresh
  // moves a preview generation, discard the entire census and reacquire it;
  // never combine its old relationships with bytes from the new observation.
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
  const landing = await fetch(base + '/?t=' + token);
  const cookie = landing.headers
    .getSetCookie()
    .map((value) => value.split(';')[0])
    .join('; ');
  const get = async (path) => {
    const deadline = Date.now() + 5000;
    for (;;) {
      const response = await fetch(base + path, { headers: cookie ? { cookie } : {} });
      if (response.ok) {
        const result = await response.json();
        if (!path.startsWith('/api/sessions?') || result.completeness.state === 'complete')
          return result;
      } else if (response.status !== 503 || !path.startsWith('/api/sessions?')) {
        const body = await response.json().catch(() => null);
        throw Error('Native fixture ' + path + ': ' + response.status, {
          cause: {
            nativeObservationChanged:
              response.status === 409 &&
              path.startsWith('/api/session-home?') &&
              body?.error?.code === 'home_details_unavailable' &&
              body.error.retryable === true,
          },
        });
      }
      if (Date.now() >= deadline)
        throw Error('Native fixture Catalog publication deadline: ' + path);
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
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
        scope: 'retained_history',
        limit: '100',
      });
      if (cursor) query.set('cursor', cursor);
      const page = await get('/api/sessions?' + query);
      if (page.machine !== source.source_key) throw Error('Native fixture Catalog source changed');
      for (const item of page.items) {
        nativeKeys.add(item.key);
        records.push({ source, item });
      }
      cursor = page.next_cursor;
    } while (cursor);
  }
  const overview = await get('/api/viewer/home');
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
    let after = 0,
      generation = null,
      page;
    do {
      const query = new URLSearchParams({
        machine: source.source_key,
        scope: 'retained_history',
        sid: item.key,
        limit: '100',
        after: String(after),
      });
      if (generation) query.set('generation', generation);
      page = await get('/api/session-transcript?' + query);
      generation ??= page.projection.generation;
      if (
        page.identity.source_key !== source.source_key ||
        page.identity.catalog_key !== item.key ||
        page.projection.generation !== generation ||
        page.range.first !== after ||
        page.range.end !== after + page.entries.length ||
        (page.range.end <= after && page.projection.total > after)
      )
        throw Error('Native fixture transcript identity, generation or range changed: ' + sid);
      const context = page.relationship_context;
      for (const [key, header] of Object.entries(context.sessions))
        headers.set(JSON.stringify([source.source_key, key]), header);
      for (const [key, version] of Object.entries(context.previews))
        previews.set(JSON.stringify([source.source_key, key]), version);
      for (const turn of context.turns)
        turns.set(JSON.stringify([source.source_key, turn.id]), {
          ...copyPresent(turn),
          sid: qualify(source.source_key, turn.sid),
        });
      for (const edge of context.handoffs)
        handoffs.set(JSON.stringify([source.source_key, edge.id]), {
          ...copyPresent(edge),
          from: qualify(source.source_key, edge.from),
          to: qualify(source.source_key, edge.to),
        });
      entries.push(
        ...page.entries.map((entry) =>
          entry.ret
            ? { ...entry, ret: { ...entry.ret, to: qualify(source.source_key, entry.ret.to) } }
            : entry,
        ),
      );
      after = page.range.end;
    } while (after < page.projection.total);
    const details = page.session.details;
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
    previews.set(JSON.stringify([source.source_key, item.key]), details.content_generation);
    // The reference census uses the supported default 24-hour observation
    // window. Native retained evidence itself remains complete.
    const cutoff = overview.now - 24 * 60 * 60 * 1000;
    SESS[sid].busy = details.busy
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
        scope: 'retained_history',
        sid: item.key,
        generation,
        events: JSON.stringify(group.flatMap((value) => (value.event ? [value.event] : []))),
        turns: JSON.stringify(group.flatMap((value) => (value.turn ? [value.turn] : []))),
      });
      const detail = await get('/api/session-home?' + query);
      if (
        detail.identity.source_key !== source.source_key ||
        detail.identity.catalog_key !== item.key ||
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
