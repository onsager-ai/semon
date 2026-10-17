// CI-only diagnostic clone of the normal pixel capture path. It replays the
// exact phone-dark main-scope frame and region prefix through Harbor, then
// records one of two predeclared paint protocols. It is not an acceptance run.
import fs from 'node:fs';
import path from 'node:path';
import { PNG } from 'pngjs';
import { compare } from '../pixel-diff.mjs';
import {
  ENV,
  launch,
  served,
  goto,
  data,
  titleOf,
  waitForCaptureReady,
  homePostTagDiagnostics,
} from '../lib.mjs';
import { sample, BASE } from '../fixture.mjs';
import { compareDomEvidence, recordDomDiffSummary } from '../dom-evidence.mjs';
import {
  SESSION_TRACE_EVIDENCE_LIMITS,
  summarizeSessionTraceResponse,
} from '../session-trace-evidence.mjs';
import {
  expectedPixelCoverage,
  PARITY_MODEL_SOURCE,
  PIXEL_REGIONS,
  REFERENCE,
  REFERENCE_MODEL_SOURCE,
  sha256,
  THRESHOLD,
  MAX_RATIO,
} from '../preservation-contract.mjs';

const scope = process.env.SEMON_PIXEL_SCOPE ?? 'main';
const coverage = expectedPixelCoverage(scope);
const expectedFrames = [...new Set(coverage.frames.map((key) => key.split('|')[1]))];
const paintArm = process.env.SEMON_HARBOR_PAINT_ARM ?? '';
const paintDiagnostic = paintArm === 'programmatic-end' || paintArm === 'wheel-traversal';
const HARBOR_FRAME = 'session-harbor';
const EXPECTED_HARBOR_PREFIX = [
  'analytics',
  'drawer',
  'home',
  'machine-laptop',
  'machines',
  'session-a-docs',
  'session-a-index',
  'session-a-parse',
  'session-a-review',
  'session-a-review-grep',
  'session-a-scan',
  'session-a-store',
  'session-advisor',
  'session-atlas',
  'session-atlas-ingest',
  'session-deps',
  'session-h-codex',
  'session-h-failed',
  'session-h-review',
  'session-h-review-codex',
  HARBOR_FRAME,
];
const harborFrameIndex = expectedFrames.indexOf(HARBOR_FRAME);
if (
  paintDiagnostic &&
  (scope !== 'main' ||
    harborFrameIndex < 0 ||
    expectedFrames[harborFrameIndex - 1] !== 'session-h-review-codex' ||
    JSON.stringify(expectedFrames.slice(0, harborFrameIndex + 1)) !==
      JSON.stringify(EXPECTED_HARBOR_PREFIX))
)
  throw new Error('Harbor paint diagnostic requires the full original main-scope prefix');
if (process.env.SEMON_HARBOR_PAINT_ARM && !paintDiagnostic)
  throw new Error('Unknown Harbor paint diagnostic arm');
if (!paintDiagnostic) throw new Error('Harbor paint diagnostic arm is required');
const captureFrames = expectedFrames.slice(0, harborFrameIndex + 1);
const selectedBase = (process.env.SEMON_PRESERVATION_BASE ?? '').replace(/\/$/, ''),
  selectedToken = process.env.SEMON_PRESERVATION_TOKEN ?? '',
  parityBase = (process.env.SEMON_REF_BASE ?? '').replace(/\/$/, ''),
  parityToken = process.env.SEMON_REF_TOKEN ?? '',
  selfParity = Boolean(parityBase);
if (Boolean(selectedBase) === selfParity)
  throw new Error('Set exactly one of the selected complete reference or served parity pair');
if ((selectedBase && !selectedToken) || (parityBase && !parityToken))
  throw new Error('The reference server token is missing');

const OUT = path.join(ENV.out, 'pixels');
fs.mkdirSync(OUT, { recursive: true });
const report = {
  candidate: process.env.GITHUB_SHA ?? null,
  candidateFingerprint: null,
  reference: selfParity ? 'served-parity' : REFERENCE.source,
  ...(selfParity ? {} : { referenceHub: REFERENCE.hub }),
  referenceModelSource: selfParity ? PARITY_MODEL_SOURCE : REFERENCE_MODEL_SOURCE,
  referenceModelDigest: null,
  scope,
  threshold: THRESHOLD,
  maxRatio: MAX_RATIO,
  coverageSource: 'tests/ui/visual-coverage.json',
  results: [],
  regions: [],
  errors: [],
  apiDiagnostics: [],
  homePostTagDiagnostics: [],
  sessionTraceEvidence: [],
  routeMappings: [],
  domDiffSummary: { casesCompared: 0, casesWithChanges: 0, bySelector: {} },
  ...(paintDiagnostic
    ? {
        diagnosticOnly: {
          purpose: 'CI-only capture-protocol diagnosis; partial main scope, never pixel acceptance',
          arm: paintArm,
          scheme: 'phone-dark',
          targetFrame: HARBOR_FRAME,
          capturedFrames: captureFrames,
          capturedFrameKeys: captureFrames.map((frame) => `phone-dark|${frame}`),
          prefixSource: 'tests/ui/visual-coverage.json main-scope frame order',
          sourceRun: 39074185832,
          sourceRunHead: '0bea3be5131e2839779c47a0cec3693d3606742f',
          candidateArtifactId: 12116097303,
          candidateArtifactDigest:
            'sha256:bc99ec4644ad9dbe31b38c72e1c96e82c12fd7e052a932824ab950e658ececab',
          referenceArtifactId: 12116471070,
          referenceArtifactDigest:
            'sha256:1f5506feec49213c0e7d1eef9d27e0ddd640d9af5d3e66417184fbebd94ea10d',
          expectedCandidateBinarySha256:
            'f8bac26d1c7736b3f72e19e6cbf9c0a818dc03c5e96ad7ddc09790d76848ca6a',
          referenceBinarySha256: process.env.SEMON_HARBOR_REFERENCE_BINARY_SHA ?? null,
          referenceSource: '263a02da278716ab2309b8e34a463419e5c781e5',
          paintCaptures: [],
        },
      }
    : {}),
};
let browser;
const homePostPages = [];
const sessionTraceBudget = { responses: 0, responseBytes: 0, summaryBytes: 0, stopped: false },
  MAX_SESSION_TRACE_RESPONSES = 36,
  MAX_SESSION_TRACE_RESPONSE_BYTES = 64 * 1024 * 1024,
  MAX_SESSION_TRACE_SUMMARY_BYTES = 16 * 1024 * 1024;

function fileKey(key) {
  return key.replace(/[^a-zA-Z0-9_-]+/g, '_');
}
function save(key, suffix, bytes) {
  fs.writeFileSync(path.join(OUT, fileKey(key) + '-' + suffix + '.png'), bytes);
}
function saveJson(key, suffix, value) {
  const bytes = Buffer.from(JSON.stringify(value, null, 2) + '\n');
  fs.writeFileSync(path.join(OUT, fileKey(key) + '-' + suffix + '.json'), bytes);
  return sha256(bytes);
}

function captureSessionTraceResponses(page, scheme) {
  page.sessionTraceEvidence = [];
  page.sessionTraceTasks = [];
  page.sessionTraceErrors = [];
  const origin = new URL(page.url()).origin;
  page.on('response', (response) => {
    let url;
    try {
      url = new URL(response.url());
    } catch {
      return;
    }
    if (
      sessionTraceBudget.stopped ||
      url.origin !== origin ||
      url.pathname !== '/api/session-trace' ||
      response.status() < 200 ||
      response.status() >= 300
    )
      return;
    sessionTraceBudget.responses++;
    if (sessionTraceBudget.responses > MAX_SESSION_TRACE_RESPONSES) {
      if (!sessionTraceBudget.stopped)
        page.sessionTraceErrors.push(
          `successful response count exceeds ${MAX_SESSION_TRACE_RESPONSES}`,
        );
      sessionTraceBudget.stopped = true;
      return;
    }
    const tracked = page.inFlightApiRequests?.get(response.request()),
      routeKey = tracked?.routeKey ?? page.captureRouteKey ?? null,
      request = Object.fromEntries(
        ['sid', 'turn', 'scope'].map((key) => [key, url.searchParams.get(key)]),
      ),
      status = response.status();
    const task = (async () => {
      const declaredBytes = Number(response.headers()['content-length']);
      if (
        Number.isFinite(declaredBytes) &&
        declaredBytes > SESSION_TRACE_EVIDENCE_LIMITS.responseBytes
      )
        throw new Error(
          `declared response bytes ${declaredBytes} exceed ${SESSION_TRACE_EVIDENCE_LIMITS.responseBytes}`,
        );
      const body = await response.body();
      if (body.length > SESSION_TRACE_EVIDENCE_LIMITS.responseBytes)
        throw new Error(
          `response bytes ${body.length} exceed ${SESSION_TRACE_EVIDENCE_LIMITS.responseBytes}`,
        );
      if (sessionTraceBudget.responseBytes + body.length > MAX_SESSION_TRACE_RESPONSE_BYTES)
        throw new Error(`aggregate response bytes exceed ${MAX_SESSION_TRACE_RESPONSE_BYTES}`);
      sessionTraceBudget.responseBytes += body.length;
      const summary = summarizeSessionTraceResponse(body),
        evidence = {
          scheme,
          routeKey,
          requestId: tracked?.id ?? null,
          queryDigest: tracked?.queryDigest ?? null,
          path: url.pathname,
          request,
          status,
          ...summary,
        },
        summaryBytes = Buffer.byteLength(JSON.stringify(evidence));
      if (sessionTraceBudget.summaryBytes + summaryBytes > MAX_SESSION_TRACE_SUMMARY_BYTES)
        throw new Error(`aggregate evidence bytes exceed ${MAX_SESSION_TRACE_SUMMARY_BYTES}`);
      sessionTraceBudget.summaryBytes += summaryBytes;
      page.sessionTraceEvidence.push(evidence);
    })().catch((error) => {
      if (/^aggregate (response|evidence) bytes exceed/.test(error.message))
        sessionTraceBudget.stopped = true;
      page.sessionTraceErrors.push(
        `route=${routeKey ?? 'unknown'} status=${status}: ${error.message}`,
      );
    });
    page.sessionTraceTasks.push(task);
  });
}

async function flushSessionTraceEvidence(page, scheme) {
  await Promise.all(page.sessionTraceTasks);
  report.sessionTraceEvidence.push(...page.sessionTraceEvidence);
  report.errors.push(
    ...page.sessionTraceErrors.map(
      (error) => `${scheme} candidate session-trace capture: ${error}`,
    ),
  );
}

async function snapshot(page) {
  return page.evaluate(() => {
    const selectors = [
      '#sidebar',
      '#topbar',
      '#lanes',
      '#lanes .tree-row',
      '#lanes .tree-row > .srow',
      '#lanes .tree-row > .srow .dot',
      '#lanes .tree-row > .srow .nm',
      '#lanes .tree-row > .srow .ag',
      '#lanes .tree-row > .srow .kid-flag',
      '#lanes .tree-row > .srow .hicon',
      '#page',
      '.nrow',
      '.think',
      '.tsum',
      '.session-dock',
      'dialog[open]',
      '.agents-panel',
      '.agents-summary',
      '.agent-row',
      '.agent-track',
      '.agent-segment',
      '.agent-edge',
      '.flow .hop',
      '.hop .sent',
      '.hop .meta',
      '.hop .node',
      '.hop .sent > *',
      '.hop .meta > *',
    ];
    return {
      historyState: history.state,
      pathname: location.pathname,
      selectors: Object.fromEntries(
        selectors.map((selector) => {
          const recentStyleSelector = selector.startsWith('#lanes .tree-row');
          return [
            selector,
            [...document.querySelectorAll(selector)]
              .slice(0, recentStyleSelector ? 30 : 60)
              .map((node) => {
                const box = node.getBoundingClientRect(),
                  css = getComputedStyle(node),
                  traceStyleSelector =
                    selector === '.hop .node' ||
                    selector === '.hop .sent > *' ||
                    selector === '.hop .meta > *',
                  styleDetail = (style) => ({
                    fontWeight: style.fontWeight,
                    color: style.color,
                    textDecoration: style.textDecoration,
                    fill: style.fill,
                    stroke: style.stroke,
                    strokeWidth: style.strokeWidth,
                  }),
                  pseudoDetail = (pseudo) => {
                    const style = getComputedStyle(node, pseudo);
                    return {
                      content: style.content,
                      display: style.display,
                      color: style.color,
                      background: style.backgroundColor,
                      border: style.border,
                      boxShadow: style.boxShadow,
                      fontWeight: style.fontWeight,
                      textDecoration: style.textDecoration,
                      fill: style.fill,
                      stroke: style.stroke,
                      strokeWidth: style.strokeWidth,
                    };
                  },
                  vectorStyles = traceStyleSelector
                    ? [...node.querySelectorAll('svg, path, rect, circle, line, polyline, polygon')]
                        .slice(0, 40)
                        .map((item) => {
                          const itemBox = item.getBoundingClientRect(),
                            itemStyle = getComputedStyle(item);
                          return {
                            tag: item.tagName,
                            class: item.getAttribute('class'),
                            path: item.getAttribute('d'),
                            fillAttribute: item.getAttribute('fill'),
                            strokeAttribute: item.getAttribute('stroke'),
                            strokeWidthAttribute: item.getAttribute('stroke-width'),
                            box: [itemBox.x, itemBox.y, itemBox.width, itemBox.height],
                            ...styleDetail(itemStyle),
                            opacity: itemStyle.opacity,
                          };
                        })
                    : undefined;
                return {
                  text: node.textContent.slice(0, 2000),
                  class: node.getAttribute('class'),
                  path: node.getAttribute('d'),
                  box: [box.x, box.y, box.width, box.height],
                  ...(recentStyleSelector
                    ? {
                        recentStyle: {
                          dataId:
                            node.getAttribute('data-id') ??
                            node.closest('.treeitem')?.getAttribute('data-id'),
                          role: node.getAttribute('role'),
                          ariaLabel:
                            node.getAttribute('aria-label') ??
                            node.closest('.treeitem')?.getAttribute('aria-label'),
                          ariaCurrent: node.getAttribute('aria-current'),
                          dataTip: node.getAttribute('data-tip')?.slice(0, 160),
                          dataHarness: node.getAttribute('data-harness'),
                          hovered: node.matches(':hover'),
                          focused: node.matches(':focus'),
                          focusVisible: node.matches(':focus-visible'),
                          focusWithin: node.matches(':focus-within'),
                          activeElement: document.activeElement === node,
                          fontWeight: css.fontWeight,
                          textDecoration: css.textDecoration,
                          fill: css.fill,
                          stroke: css.stroke,
                          strokeWidth: css.strokeWidth,
                        },
                      }
                    : {}),
                  ...(traceStyleSelector
                    ? {
                        tag: node.tagName,
                        traceStyle: {
                          ...styleDetail(css),
                          before: pseudoDetail('::before'),
                          after: pseudoDetail('::after'),
                        },
                        ...(vectorStyles?.length ? { vectorStyles } : {}),
                      }
                    : {}),
                  font: css.font,
                  color: css.color,
                  background: css.backgroundColor,
                  padding: css.padding,
                  gap: css.gap,
                  display: css.display,
                  opacity: css.opacity,
                  border: css.border,
                  icons: [...node.querySelectorAll('img')]
                    .slice(0, 40)
                    .map((image) => image.getAttribute('src')),
                  paths: [...node.querySelectorAll('path')]
                    .slice(0, 80)
                    .map((item) => item.getAttribute('d')),
                };
              }),
          ];
        }),
      ),
    };
  });
}

function minute(t) {
  return Math.round(((t - BASE) / 60000) * 1000) / 1000;
}
async function pinnedReferenceModel(base, token) {
  const origin = new URL(base).origin,
    landing = await fetch(base + '/?t=' + encodeURIComponent(token));
  if (!landing.ok || new URL(landing.url).origin !== origin)
    throw new Error(`pinned reference model landing failed: HTTP ${landing.status}`);
  const cookies =
      typeof landing.headers.getSetCookie === 'function'
        ? landing.headers.getSetCookie()
        : [landing.headers.get('set-cookie')].filter(Boolean),
    cookie = cookies.map((value) => value.split(';')[0]).join('; ');
  if (!cookie)
    throw new Error('pinned reference model landing did not set an authorization cookie');
  const response = await fetch(base + '/api/model?delta=1&content=1', {
    headers: { cookie, accept: 'application/json' },
  });
  if (!response.ok || new URL(response.url).origin !== origin)
    throw new Error(`pinned reference model request failed: HTTP ${response.status}`);
  const bytes = Buffer.from(await response.arrayBuffer()),
    model = JSON.parse(bytes.toString('utf8'));
  if (!Array.isArray(model.handoffs) || !Array.isArray(model.turns))
    throw new Error('pinned reference model is missing handoffs or turns');
  return { model, digest: sha256(bytes) };
}

function referenceHandoffs(D, source, referenceModel) {
  const ids = new Map();
  for (const h of source.H) {
    const match = D.H.find(
      (candidate) =>
        !ids.has(candidate.id) &&
        candidate.kind === h.kind &&
        candidate.from === h.from &&
        candidate.to === h.to &&
        minute(candidate.at) === h.at,
    );
    if (!match) continue;
    if (!referenceModel) {
      ids.set(match.id, { fixtureId: h.id, modelId: h.id });
      continue;
    }
    const semantic = (candidate) =>
        candidate.kind === h.kind &&
        candidate.from === h.from &&
        candidate.to === h.to &&
        minute(candidate.at) === h.at,
      exact = referenceModel.handoffs.find((candidate) => candidate.id === h.id),
      matches = exact && semantic(exact) ? [exact] : referenceModel.handoffs.filter(semantic);
    if (matches.length !== 1) {
      report.errors.push(
        `Reference handoff mapping for ${match.id} -> ${h.id} has ${matches.length} exact model matches`,
      );
      continue;
    }
    ids.set(match.id, { fixtureId: h.id, modelId: matches[0].id });
  }
  return ids;
}

function originalTurnId(turn, referenceModel, handoffIds) {
  if (!referenceModel) return { id: turn.id };
  const start = turn.start ? handoffIds.get(turn.start)?.modelId : null,
    userText = typeof turn.text === 'string' ? turn.text : turn.u?.text,
    matches = referenceModel.turns.filter((candidate) => {
      if (candidate.sid !== turn.sid) return false;
      if (start) return candidate.start === start;
      if (!turn.u || !candidate.u) return false;
      return (
        (userText == null || candidate.text === userText) &&
        (turn.at == null || candidate.at === turn.at)
      );
    });
  if (matches.length !== 1)
    throw new Error(
      `pinned reference turn mapping for ${turn.sid}:${turn.id} has ${matches.length} exact ` +
        `matches (start=${start ?? 'none'}, user=${Boolean(turn.u)}, at=${turn.at ?? 'none'})`,
    );
  return matches[0];
}

function inputEvidence(turn) {
  if (!turn?.u) return null;
  const text = typeof turn.text === 'string' ? turn.text : (turn.u?.text ?? '');
  return { text: String(text).slice(0, 160), chars: String(text).length, at: turn.at ?? null };
}

function handoffEvidence(handoff, id) {
  return handoff
    ? {
        id: handoff.id,
        kind: handoff.kind,
        from: handoff.from,
        to: handoff.to,
        at: handoff.at,
      }
    : id
      ? { id }
      : null;
}

function screens(D, source, handoffIds, referenceModel) {
  const list = [
    { name: 'home', route: { v: 'home' } },
    { name: 'analytics', route: { v: 'analytics' } },
    { name: 'sessions', route: { v: 'sessions' } },
    { name: 'machines', route: { v: 'machines' } },
  ];
  for (const machine of Object.keys(D.MACHINE))
    list.push({ name: 'machine-' + machine, route: { v: 'machine', id: machine } });
  for (const sid of Object.keys(D.SESS))
    list.push({ name: 'session-' + sid, route: { v: 'session', id: sid } });
  for (const turn of D.turns) {
    if (!turn.sent?.length) continue;
    const at = (D.TX[turn.sid] ?? []).findIndex((entry) => entry.turn === turn.id);
    if (at < 0) {
      list.push({
        name: 'trace-' + turn.sid + '-' + String(turn.id).replace(/[^\w-]/g, '_'),
        route: { v: 'trace', sid: turn.sid, turn: turn.id },
        referenceRoute: null,
      });
      continue;
    }
    const fixtureTurn = turn.start
      ? handoffIds.get(turn.start)?.fixtureId
      : source.TX[turn.sid]?.[at]?.k === 'u'
        ? turn.sid + ':' + at
        : null;
    if (!fixtureTurn) {
      report.errors.push(
        `Historical frame key missing for ${turn.sid}:${turn.id} at transcript entry ${at}`,
      );
    }
    let referenceTurn = null;
    try {
      referenceTurn = originalTurnId(turn, referenceModel, handoffIds);
    } catch (error) {
      report.errors.push(error.message);
    }
    const stableTurn = fixtureTurn ?? turn.id;
    list.push({
      name: 'trace-' + turn.sid + '-' + String(stableTurn).replace(/[^\w-]/g, '_'),
      route: { v: 'trace', sid: turn.sid, turn: turn.id },
      referenceRoute: referenceTurn ? { v: 'trace', sid: turn.sid, turn: referenceTurn.id } : null,
      referenceTurnBinding: {
        candidateTurn: turn.id,
        candidateSession: turn.sid,
        candidateAt: turn.at ?? null,
        candidateStart: handoffEvidence(
          D.H.find((handoff) => handoff.id === turn.start),
          turn.start,
        ),
        candidateInput: inputEvidence(turn),
        referenceTurn: referenceTurn?.id ?? null,
        referenceAt: referenceTurn?.at ?? null,
        referenceStart: handoffEvidence(
          referenceModel?.handoffs.find((handoff) => handoff.id === referenceTurn?.start),
          referenceTurn?.start,
        ),
        referenceInput: inputEvidence(referenceTurn),
        historicalFrameKey: stableTurn,
      },
    });
  }
  return list;
}

async function navigateReference(page, route, referenceData) {
  const title = titleOf(route, referenceData);
  if (!title)
    throw new Error(`pinned reference route has no qualified title: ${JSON.stringify(route)}`);
  if (page.captureReadinessEnabled) page.captureRouteKey = JSON.stringify(route);
  await page.evaluate((value) => {
    history.pushState(value, '');
    dispatchEvent(new PopStateEvent('popstate', { state: value }));
  }, route);
  try {
    await page.waitForFunction(
      ({ value, title }) => {
        const panel = document.querySelector('#page');
        return (
          history.state?.v === value.v &&
          document.querySelector('#topbar .t')?.textContent === title &&
          panel?.childElementCount > 0 &&
          !panel.hasAttribute('aria-busy') &&
          !panel.querySelector('[aria-busy="true"]') &&
          (value.v !== 'session' ||
            (panel.querySelector('.ph h1')?.textContent === title &&
              !!panel.querySelector('section[aria-label="Transcript"], .transcript'))) &&
          (value.v !== 'trace' ||
            (!!panel.querySelector('.trace-summary') &&
              [...panel.querySelectorAll('.hop.k-root[data-turn]')].some(
                (node) => node.dataset.turn === value.turn,
              )))
        );
      },
      { value: route, title },
    );
  } catch (error) {
    const state = await page.evaluate(() => {
      const panel = document.querySelector('#page');
      return {
        historyState: history.state,
        pathname: location.pathname,
        title: document.querySelector('#topbar .t')?.textContent ?? null,
        busy:
          !panel || panel.hasAttribute('aria-busy') || !!panel.querySelector('[aria-busy="true"]'),
        traceSummary: !!panel?.querySelector('.trace-summary'),
        pageText: panel?.innerText.slice(0, 600) ?? null,
      };
    });
    state.apiDiagnostics = page.captureApiEvents?.slice(-20) ?? [];
    throw new Error(
      `pinned reference route ${JSON.stringify(route)} did not settle; ` +
        `state=${JSON.stringify(state)}; pageErrors=${JSON.stringify(page.errors ?? [])}; ` +
        `wait=${error.message}`,
    );
  }
  if (route.v === 'analytics')
    await page.waitForSelector('.analytics-metrics[data-analytics-ready]');
  await page.evaluate(() => document.fonts.ready);
  await waitForCaptureReady(page, `pinned reference route ${route.v}`);
}

async function navigate(page, route, referenceData) {
  if (selfParity) await goto(page, route, referenceData);
  else await navigateReference(page, route, referenceData);
}

async function prepareFrame(page, phone) {
  await page.evaluate(async () => {
    await document.fonts.ready;
    const state = history.state,
      main = document.querySelector('#main');
    if (state?.v === 'session' && !state.turn) {
      if (matchMedia('(max-width: 760px)').matches)
        window.scrollTo(0, document.documentElement.scrollHeight);
      else if (main) main.scrollTop = main.scrollHeight;
    } else {
      window.scrollTo(0, 0);
      if (main) main.scrollTop = 0;
    }
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  });
  await waitForCaptureReady(page, `frame scroll ${page.url()}`);
  const before =
    paintDiagnostic && page.captureRouteKey === JSON.stringify({ v: 'session', id: 'harbor' })
      ? await minimalPaintState(page, 'programmatic-end-before-original-fullpage')
      : null;
  const bytes = await page.screenshot({ fullPage: phone, animations: 'disabled', caret: 'hide' });
  return before ? { bytes, before } : bytes;
}

async function minimalPaintState(page, label) {
  return page.evaluate((captureLabel) => {
    const main = document.querySelector('#main');
    const state = history.state && typeof history.state === 'object' ? history.state : {};
    return {
      label: captureLabel,
      pathname: location.pathname,
      route: {
        v: typeof state.v === 'string' ? state.v.slice(0, 32) : null,
        id: typeof state.id === 'string' ? state.id.slice(0, 80) : null,
        sid: typeof state.sid === 'string' ? state.sid.slice(0, 80) : null,
        turn: typeof state.turn === 'string' ? state.turn.slice(0, 80) : null,
      },
      viewport: { width: innerWidth, height: innerHeight, dpr: devicePixelRatio },
      scroll: { windowX: scrollX, windowY: scrollY, mainTop: main?.scrollTop ?? null },
    };
  }, label);
}

async function boundedPaintState(page, label) {
  return page.evaluate((captureLabel) => {
    const round = (value) => Math.round(value * 100) / 100;
    const root = document.documentElement;
    const main = document.querySelector('#main');
    const panel = document.querySelector('#page');
    const state = history.state && typeof history.state === 'object' ? history.state : {};
    const visual = window.visualViewport;
    const allTurns = [...document.querySelectorAll('.turn')];
    const faces = [...document.fonts].slice(0, 24);
    return {
      label: captureLabel,
      pathname: location.pathname,
      route: {
        v: typeof state.v === 'string' ? state.v.slice(0, 32) : null,
        id: typeof state.id === 'string' ? state.id.slice(0, 80) : null,
        sid: typeof state.sid === 'string' ? state.sid.slice(0, 80) : null,
        turn: typeof state.turn === 'string' ? state.turn.slice(0, 80) : null,
      },
      viewport: {
        innerWidth,
        innerHeight,
        devicePixelRatio,
        visual: visual
          ? {
              width: round(visual.width),
              height: round(visual.height),
              offsetLeft: round(visual.offsetLeft),
              offsetTop: round(visual.offsetTop),
              pageLeft: round(visual.pageLeft),
              pageTop: round(visual.pageTop),
              scale: visual.scale,
            }
          : null,
      },
      scroll: {
        windowX: round(scrollX),
        windowY: round(scrollY),
        documentTop: round(root.scrollTop),
        mainTop: main ? round(main.scrollTop) : null,
      },
      document: {
        clientWidth: root.clientWidth,
        clientHeight: root.clientHeight,
        scrollWidth: root.scrollWidth,
        scrollHeight: root.scrollHeight,
        bodyScrollWidth: document.body?.scrollWidth ?? null,
        bodyScrollHeight: document.body?.scrollHeight ?? null,
        pageChildCount: panel?.childElementCount ?? null,
        mainClientHeight: main?.clientHeight ?? null,
        mainScrollHeight: main?.scrollHeight ?? null,
      },
      fonts: {
        status: document.fonts.status,
        faceCount: document.fonts.size,
        faces: faces.map((face) => ({
          family: face.family.slice(0, 80),
          status: face.status,
          weight: face.weight,
          style: face.style,
        })),
        truncatedFaces: Math.max(0, document.fonts.size - faces.length),
      },
      contentVisibility: {
        count: allTurns.length,
        sampled: allTurns.slice(0, 64).map((node, index) => {
          const rect = node.getBoundingClientRect();
          const style = getComputedStyle(node);
          return {
            index,
            box: [rect.x, rect.y, rect.width, rect.height].map(round),
            inViewport: rect.bottom > 0 && rect.top < innerHeight,
            contentVisibility: style.contentVisibility,
            contain: style.contain,
            containIntrinsicBlockSize: style.containIntrinsicBlockSize,
            display: style.display,
            visibility: style.visibility,
            opacity: style.opacity,
          };
        }),
        truncated: Math.max(0, allTurns.length - 64),
      },
    };
  }, label);
}

function appendPaintCapture(entry) {
  if (paintDiagnostic) report.diagnosticOnly.paintCaptures.push(entry);
}

async function saveHarborViewport(page, scheme, side, label) {
  const key = `${scheme}|${HARBOR_FRAME}|paint-${side}-${label}`;
  const before = await minimalPaintState(page, `${label}:before-viewport-screenshot`);
  const bytes = await page.screenshot({ fullPage: false, animations: 'disabled', caret: 'hide' });
  save(key, 'viewport', bytes);
  const after = await boundedPaintState(page, `${label}:after-viewport-screenshot`);
  appendPaintCapture({
    side,
    label,
    file: `${fileKey(key)}-viewport.png`,
    bytes: bytes.byteLength,
    sha256: sha256(bytes),
    before,
    after,
  });
}

async function harborScrollPosition(page) {
  return page.evaluate(() => {
    const root = document.documentElement;
    const main = document.querySelector('#main');
    const windowMax = Math.max(0, root.scrollHeight - innerHeight);
    const mainMax = main ? Math.max(0, main.scrollHeight - main.clientHeight) : 0;
    const activeScroller = windowMax > 1 ? 'window' : mainMax > 1 ? 'main' : 'none';
    return {
      windowY: scrollY,
      mainTop: main?.scrollTop ?? 0,
      windowMax,
      mainMax,
      activeScroller,
      activePosition:
        activeScroller === 'window' ? scrollY : activeScroller === 'main' ? main.scrollTop : 0,
      activeMax: activeScroller === 'window' ? windowMax : activeScroller === 'main' ? mainMax : 0,
      viewportHeight: innerHeight,
    };
  });
}

function harborAtEnd(position) {
  return position.activeMax <= 1 || position.activePosition >= position.activeMax - 1;
}

async function harborWheelPoint(page) {
  return page.evaluate(() => {
    const bar = document.querySelector('#topbar');
    if (!bar) throw new Error('missing-topbar');
    const rect = bar.getBoundingClientRect();
    const x = Math.min(rect.right - 1, innerWidth - 1);
    const y = Math.max(rect.top + 1, 1);
    const target = document.elementFromPoint(x, y);
    if (!bar.contains(target) || target.closest('button, a, input, [role="button"]'))
      throw new Error('no-inert-topbar-wheel-point');
    return { x, y };
  });
}

async function captureHarborWheelViewports(page, scheme, side) {
  const maxSteps = 16;
  let position = await harborScrollPosition(page);
  let reachedEnd = harborAtEnd(position);
  await saveHarborViewport(page, scheme, side, 'wheel-000-initial');
  for (let step = 1; !reachedEnd && step <= maxSteps; step++) {
    const before = position;
    const deltaY = Math.max(
      1,
      Math.min(Math.round(before.viewportHeight * 0.75), before.activeMax - before.activePosition),
    );
    await page.evaluate(() => {
      window.__harborPrefixWheel = null;
      document.addEventListener(
        'wheel',
        (event) => {
          window.__harborPrefixWheel = {
            trusted: event.isTrusted,
            deltaY: event.deltaY,
            deltaMode: event.deltaMode,
          };
        },
        { once: true, capture: true, passive: true },
      );
    });
    const point = await harborWheelPoint(page);
    await page.mouse.move(point.x, point.y);
    await page.mouse.wheel(0, deltaY);
    let failure = null;
    try {
      await page.waitForFunction(() => window.__harborPrefixWheel?.trusted === true, null, {
        timeout: 8000,
      });
      await page.waitForFunction(
        (previous) => {
          const main = document.querySelector('#main');
          const root = document.documentElement;
          const windowMax = Math.max(0, root.scrollHeight - innerHeight);
          const mainMax = main ? Math.max(0, main.scrollHeight - main.clientHeight) : 0;
          return previous.activeScroller === 'window'
            ? scrollY > previous.windowY + 1 || scrollY >= windowMax - 1
            : previous.activeScroller === 'main'
              ? main.scrollTop > previous.mainTop + 1 || main.scrollTop >= mainMax - 1
              : true;
        },
        before,
        { timeout: 8000 },
      );
      await page.evaluate(
        () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
      );
      await waitForCaptureReady(page, `Harbor trusted wheel step ${step}`);
    } catch (error) {
      failure = error;
    }
    position = await harborScrollPosition(page);
    const event = await page.evaluate(() => window.__harborPrefixWheel);
    await saveHarborViewport(page, scheme, side, `wheel-${String(step).padStart(3, '0')}`);
    appendPaintCapture({
      side,
      label: `wheel-${String(step).padStart(3, '0')}-movement`,
      point,
      requestedDeltaY: deltaY,
      event,
      before,
      after: position,
      ...(failure ? { errorName: failure.name ?? 'Error' } : {}),
    });
    if (
      failure ||
      !event?.trusted ||
      (position.activePosition <= before.activePosition + 1 && !harborAtEnd(position))
    ) {
      report.errors.push(
        `${side} Harbor wheel traversal step ${step}: ${failure?.message ?? 'no trusted progress'}`,
      );
      break;
    }
    reachedEnd = harborAtEnd(position);
  }
  appendPaintCapture({
    side,
    label: 'wheel-traversal-result',
    reachedEnd,
    finalPosition: position,
  });
  if (!reachedEnd)
    report.errors.push(
      `${side} Harbor wheel traversal exceeded ${maxSteps} steps without reaching end`,
    );
}

async function captureHarborProgrammaticViewports(page, scheme, side) {
  await saveHarborViewport(page, scheme, side, 'programmatic-end-after-fullpage');
}

async function captureFrame(candidate, reference, key, phone) {
  const images = [],
    artifactDigests = {},
    evidenceBySide = {},
    prefix = fileKey(key),
    artifactPaths = {
      candidateImage: `pixels/${prefix}-candidate.png`,
      candidateEvidence: `pixels/${prefix}-candidate.json`,
      referenceImage: `pixels/${prefix}-reference.png`,
      referenceEvidence: `pixels/${prefix}-reference.json`,
      diffImage: `pixels/${prefix}-diff.png`,
      comparisonEvidence: `pixels/${prefix}-comparison.json`,
    };
  for (const [name, page] of [
    ['candidate', candidate],
    ['reference', reference],
  ]) {
    try {
      const prepared = await prepareFrame(page, phone);
      const bytes = Buffer.isBuffer(prepared) ? prepared : prepared.bytes;
      save(key, name, bytes);
      evidenceBySide[name] = await snapshot(page);
      artifactDigests[name] = {
        image: sha256(bytes),
        evidence: saveJson(key, name, evidenceBySide[name]),
      };
      if (paintDiagnostic && key === `phone-dark|${HARBOR_FRAME}`)
        appendPaintCapture({
          side: name,
          label: 'original-fullpage',
          file: artifactPaths[`${name}Image`],
          fullPage: phone,
          bytes: bytes.byteLength,
          sha256: sha256(bytes),
          before: Buffer.isBuffer(prepared) ? null : prepared.before,
          after: evidenceBySide[name],
        });
      images.push(PNG.sync.read(bytes));
    } catch (error) {
      report.errors.push(`${key} ${name} screenshot: ${error.message}`);
    }
  }
  if (images.length !== 2) return;
  recordDomDiffSummary(
    report.domDiffSummary,
    compareDomEvidence(evidenceBySide.candidate, evidenceBySide.reference, key),
  );
  const result = compare(...images, THRESHOLD);
  const diffBytes = PNG.sync.write(result.diff);
  save(key, 'diff', diffBytes);
  artifactDigests.diff = {
    image: sha256(diffBytes),
    evidence: saveJson(key, 'comparison', {
      key,
      pixels: result.pixels,
      ratio: result.ratio,
      difference: result.difference,
      size: result.size,
      threshold: THRESHOLD,
      maxRatio: MAX_RATIO,
    }),
  };
  report.results.push({
    key,
    pixels: result.pixels,
    ratio: result.ratio,
    difference: result.difference,
    size: result.size,
    pass: !result.size && result.ratio <= MAX_RATIO,
    artifactDigests,
    artifactPaths,
  });
}

async function regionEvidence(page, region) {
  return page.evaluate(({ selector, open }) => {
    const roots = [...document.querySelectorAll(selector)];
    const nodes = roots.flatMap((root) => [root, ...root.querySelectorAll('*')]).slice(0, 180);
    return {
      historyState: history.state,
      pathname: location.pathname,
      viewport: [window.innerWidth, window.innerHeight],
      scroll: { window: window.scrollY, main: document.querySelector('#main')?.scrollTop ?? null },
      selector,
      count: roots.length,
      openSelector: open ?? null,
      openCount: open ? document.querySelectorAll(open).length : null,
      nodes: nodes.map((node) => {
        const box = node.getBoundingClientRect(),
          css = getComputedStyle(node);
        return {
          tag: node.tagName,
          id: node.id,
          class: node.getAttribute('class'),
          label: node.getAttribute('aria-label'),
          text: [...node.childNodes]
            .filter((child) => child.nodeType === Node.TEXT_NODE)
            .map((child) => child.textContent.slice(0, 300))
            .join(''),
          box: [box.x, box.y, box.width, box.height],
          style: Object.fromEntries(
            ['display', 'font', 'color', 'backgroundColor', 'gap', 'padding', 'overflow'].map(
              (key) => [key, css[key]],
            ),
          ),
        };
      }),
    };
  }, region);
}

async function regionShot(page, region, key, side) {
  if (region.top || region.name === 'transcript') await resetTopScroll(page);
  if (region.open) {
    const opener = page.locator(region.open).first();
    if (!(await opener.count())) throw new Error(`open control missing: ${region.open}`);
    await opener.click();
    await waitForCaptureReady(page, `${key} opened region`);
  }
  const evidence = await regionEvidence(page, region);
  const evidenceDigest = saveJson(key, side + '-geometry', evidence);
  if (!evidence.count) throw new Error(`region selector missing: ${region.selector}`);
  const target = page.locator(region.selector).first();
  await target.waitFor({ state: 'visible', timeout: 3000 });
  const clip = region.name === 'transcript' ? await target.boundingBox() : null;
  if (region.name === 'transcript' && !clip)
    throw new Error('transcript region has no capture box');
  const bytes = clip
    ? await page.screenshot({ clip, animations: 'disabled', caret: 'hide' })
    : await target.screenshot({ animations: 'disabled', caret: 'hide' });
  if (region.open) await closeRegionPanel(page, region, key);
  save(key, side, bytes);
  return {
    image: PNG.sync.read(bytes),
    digests: { image: sha256(bytes), evidence: evidenceDigest },
    evidence,
  };
}

async function closeRegionPanel(page, region, key) {
  // The original app's document Escape listener removes the menu node before
  // its native close owner can release viewerEl/history. Use its real Close
  // button for capture cleanup; keyboard dismissal has its functional suite.
  await page
    .locator(region.selector)
    .first()
    .getByRole('button', { name: 'Close', exact: true })
    .click();
  await page.waitForFunction(
    () => !document.querySelector('dialog[open]') && !history.state?.sheet,
  );
  await waitForCaptureReady(page, `${key} closed region`);
}

async function resetTopScroll(page) {
  // End the application's opening tail transaction through real reader input,
  // as the complete-reading protocol does. A programmatic scroll alone can be
  // repinned by its pending ResizeObserver (the Quill capture moved 23px).
  const point = await page.evaluate(() => {
    window.__regionTopWheelObserved = false;
    document.addEventListener(
      'wheel',
      (event) => {
        window.__regionTopWheelObserved = event.isTrusted;
      },
      { once: true, capture: true, passive: true },
    );
    const bar = document.querySelector('#topbar'),
      rect = bar.getBoundingClientRect(),
      x = Math.min(rect.right - 1, innerWidth - 1),
      y = Math.max(rect.top + 1, 1),
      target = document.elementFromPoint(x, y);
    if (!bar.contains(target) || target.closest('button, a, input, [role="button"]'))
      throw Error('Region reading input requires inert topbar padding');
    return { x, y };
  });
  await page.mouse.move(point.x, point.y);
  await page.mouse.wheel(0, -100000);
  await page.waitForFunction(() => window.__regionTopWheelObserved, null, { timeout: 8000 });
  await page.evaluate(() => {
    window.scrollTo(0, 0);
    const main = document.querySelector('#main');
    if (main) main.scrollTop = 0;
  });
  await page.evaluate(
    () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
  );
  await waitForCaptureReady(page, 'region top scroll');
  await page.waitForFunction(
    () => scrollY <= 1 && document.querySelector('#main').scrollTop <= 1,
    null,
    { timeout: 8000 },
  );
}

async function sharedTranscriptViewport(candidate, reference, region) {
  const pages = [candidate, reference],
    original = pages.map((page) => page.viewportSize());
  if (
    !original[0] ||
    !original[1] ||
    original[0].width !== original[1].width ||
    original[0].height !== original[1].height
  )
    throw new Error('candidate and reference transcript viewports must match before capture');
  await Promise.all(pages.map(resetTopScroll));
  const boxes = await Promise.all(
    pages.map((page) => page.locator(region.selector).first().boundingBox()),
  );
  if (boxes.some((box) => !box))
    throw new Error('transcript is missing while sizing shared viewport');
  const needed = await Promise.all(
    pages.map(async (page, index) => {
      const dock = page.locator('.session-dock').first(),
        dockHeight = (await dock.count()) ? ((await dock.boundingBox())?.height ?? 0) : 0;
      return boxes[index].y + boxes[index].height + dockHeight + 80;
    }),
  );
  const height = Math.ceil(Math.max(original[0].height, ...needed)),
    shared = original.map((viewport) => ({ width: viewport.width, height }));
  try {
    await Promise.all(pages.map((page, index) => page.setViewportSize(shared[index])));
    await Promise.all(
      pages.map((page) =>
        page.evaluate(async () => {
          await document.fonts.ready;
          await new Promise((resolve) =>
            requestAnimationFrame(() => requestAnimationFrame(resolve)),
          );
        }),
      ),
    );
    await Promise.all(pages.map((page) => waitForCaptureReady(page, 'transcript viewport resize')));
    const stabilized = await Promise.all(
      pages.map((page) => page.locator(region.selector).first().boundingBox()),
    );
    if (
      stabilized.some(
        (box, index) =>
          !box ||
          Math.abs(box.width - boxes[index].width) > 0.5 ||
          Math.abs(box.height - boxes[index].height) > 0.5,
      )
    )
      throw new Error('transcript geometry changed while matching capture viewports');
    const viewports = pages.map((page) => page.viewportSize());
    if (
      viewports.some(
        (viewport) =>
          !viewport || viewport.width !== original[0].width || viewport.height !== height,
      )
    )
      throw new Error('candidate and reference transcript viewports failed to match');
    const requiredAfter = await Promise.all(
      pages.map(async (page, index) => {
        const dock = page.locator('.session-dock').first(),
          dockHeight = (await dock.count()) ? ((await dock.boundingBox())?.height ?? 0) : 0;
        return stabilized[index].y + stabilized[index].height + dockHeight + 80;
      }),
    );
    if (requiredAfter.some((required) => required > height))
      throw new Error('shared transcript viewport does not fit both capture regions');
    const actualViewports = await Promise.all(
      pages.map((page) => page.evaluate(() => [window.innerWidth, window.innerHeight])),
    );
    if (
      actualViewports.some(
        ([width, viewportHeight]) => width !== original[0].width || viewportHeight !== height,
      )
    )
      throw new Error('browser transcript viewports failed to match');
    return original;
  } catch (error) {
    await Promise.all(pages.map((page, index) => page.setViewportSize(original[index])));
    throw error;
  }
}

async function captureRegion(candidate, reference, region, key) {
  const images = [],
    artifactDigests = {},
    evidenceBySide = {},
    prefix = fileKey(key),
    artifactPaths = {
      candidateImage: `pixels/${prefix}-candidate.png`,
      candidateEvidence: `pixels/${prefix}-candidate-geometry.json`,
      referenceImage: `pixels/${prefix}-reference.png`,
      referenceEvidence: `pixels/${prefix}-reference-geometry.json`,
      diffImage: `pixels/${prefix}-diff.png`,
      comparisonEvidence: `pixels/${prefix}-comparison.json`,
    };
  const pages = [candidate, reference],
    originalViewports =
      region.name === 'transcript'
        ? await sharedTranscriptViewport(candidate, reference, region)
        : null;
  try {
    for (const [side, page] of [
      ['candidate', candidate],
      ['reference', reference],
    ]) {
      try {
        const captured = await regionShot(page, region, key, side);
        images.push(captured.image);
        artifactDigests[side] = captured.digests;
        evidenceBySide[side] = captured.evidence;
      } catch (error) {
        report.errors.push(`${key} ${side}: ${error.message}`);
      }
    }
    if (images.length !== 2) return;
    recordDomDiffSummary(
      report.domDiffSummary,
      compareDomEvidence(evidenceBySide.candidate, evidenceBySide.reference, key),
    );
    const result = compare(...images, THRESHOLD);
    const diffBytes = PNG.sync.write(result.diff);
    save(key, 'diff', diffBytes);
    artifactDigests.diff = {
      image: sha256(diffBytes),
      evidence: saveJson(key, 'comparison', {
        key,
        pixels: result.pixels,
        ratio: result.ratio,
        difference: result.difference,
        size: result.size,
        threshold: THRESHOLD,
        maxRatio: MAX_RATIO,
      }),
    };
    report.regions.push({
      key,
      pixels: result.pixels,
      ratio: result.ratio,
      difference: result.difference,
      size: result.size,
      pass: !result.size && result.ratio <= MAX_RATIO,
      artifactDigests,
      artifactPaths,
    });
  } finally {
    if (originalViewports)
      await Promise.all(pages.map((page, index) => page.setViewportSize(originalViewports[index])));
  }
}

async function openDrawer(page) {
  await page.click('#lead-btn');
  await waitForCaptureReady(page, 'drawer opened');
}

try {
  const referenceBase = selfParity ? parityBase : selectedBase,
    referenceToken = selfParity ? parityToken : selectedToken;
  if (!ENV.base || !ENV.token) throw new Error('Candidate server is required');
  if (!/^[a-f0-9]{40}$/.test(report.candidate ?? ''))
    throw new Error('Candidate commit SHA is required for exact visual evidence');
  const candidateBinary = process.env.SEMON_VISUAL_CANDIDATE_BIN ?? process.env.SEMON_BIN;
  if (!candidateBinary || !fs.existsSync(candidateBinary))
    throw new Error('Candidate binary is required for exact visual evidence');
  report.candidateFingerprint = sha256(fs.readFileSync(candidateBinary));
  if (
    paintDiagnostic &&
    report.candidateFingerprint !==
      'f8bac26d1c7736b3f72e19e6cbf9c0a818dc03c5e96ad7ddc09790d76848ca6a'
  )
    throw new Error(
      'Harbor paint diagnostic candidate binary fingerprint does not match retained run',
    );
  if (ENV.base === referenceBase)
    throw new Error('Candidate and reference servers must be independent');
  browser = await launch();
  report.diagnosticOnly.browserVersion = browser.version();
  const D = await data(),
    original = sample('overhaul.json'),
    pinnedModel = selfParity ? null : await pinnedReferenceModel(referenceBase, referenceToken);
  report.referenceModelDigest = pinnedModel?.digest ?? null;
  const handoffIds = referenceHandoffs(D, original, pinnedModel?.model),
    routes = screens(D, original, handoffIds, pinnedModel?.model),
    routeByName = new Map();
  for (const route of routes) {
    if (routeByName.has(route.name))
      report.errors.push('Duplicate generated frame route: ' + route.name);
    routeByName.set(route.name, route);
  }
  const selectedNames = new Set(expectedFrames);
  report.routeMappings = routes.map((route) => ({
    frame: route.name,
    candidateRoute: route.route,
    referenceRoute: Object.hasOwn(route, 'referenceRoute') ? route.referenceRoute : route.route,
    ...(route.referenceTurnBinding ? { referenceTurnBinding: route.referenceTurnBinding } : {}),
  }));
  if (expectedFrames.includes('drawer'))
    report.routeMappings.push({
      frame: 'drawer',
      candidateRoute: { v: 'home' },
      referenceRoute: { v: 'home' },
      openControl: '#lead-btn',
    });
  for (const name of expectedFrames)
    if (name !== 'drawer' && !routeByName.has(name))
      report.errors.push('Missing generated frame route: ' + name);
  if (scope === 'main')
    for (const route of routes)
      if (!selectedNames.has(route.name))
        report.errors.push('Unmapped generated frame route: ' + route.name);
  if (scope === 'two-machines')
    for (const route of routes)
      if (
        (route.name === 'machines' || route.name.startsWith('machine-')) &&
        !selectedNames.has(route.name)
      )
        report.errors.push('Unmapped multi-machine frame route: ' + route.name);
  const referenceData = selfParity
    ? await data({ base: parityBase, token: parityToken })
    : {
        SESS: pinnedModel.model.sessions,
        MACHINE: Object.fromEntries(
          (pinnedModel.model.machines ?? [pinnedModel.model.machine]).map((machine) => [
            machine.id,
            machine.name,
          ]),
        ),
      };

  const schemes = ['phone-dark'];
  for (const scheme of schemes) {
    const [size, colorScheme] = scheme.split('-'),
      dark = colorScheme === 'dark',
      phone = size === 'phone',
      options = {
        size: phone ? 'phone' : 'desktop',
        viewport: { width: phone ? 390 : 1280, height: phone ? 844 : 860 },
        dark,
      };
    const candidate = await served(browser, {
        ...options,
        captureRequests: true,
        captureReadiness: true,
        captureRole: 'candidate',
        captureHomePostTag: true,
        onCaptureHomePostTagPage: (page) => homePostPages.push({ scheme, page }),
      }),
      reference = await served(browser, {
        ...options,
        captureRequests: true,
        captureReadiness: true,
        captureRole: selfParity ? 'candidate' : 'reference',
        base: referenceBase,
        token: referenceToken,
        ...(selfParity ? {} : { path: '/?compat=1' }),
      });
    captureSessionTraceResponses(candidate, scheme);
    const retired = candidate.requests ?? [];
    for (const frame of captureFrames) {
      if (frame === 'drawer' && !phone) continue;
      const key = `${scheme}|${frame}`;
      if (frame === 'drawer') {
        try {
          await goto(candidate, { v: 'home' }, D);
          await navigate(reference, { v: 'home' }, referenceData);
          await Promise.all([openDrawer(candidate), openDrawer(reference)]);
          await captureFrame(candidate, reference, key, phone);
        } catch (error) {
          report.errors.push(`${key} navigation: ${error.message}`);
        }
      } else {
        const route = routeByName.get(frame);
        if (!route) continue;
        try {
          const traceTaskStart = candidate.sessionTraceTasks.length;
          await goto(candidate, route.route, D);
          if (route.route.v === 'trace') {
            await Promise.all(candidate.sessionTraceTasks.slice(traceTaskStart));
            const routeKey = JSON.stringify(route.route);
            if (!candidate.sessionTraceEvidence.some((entry) => entry.routeKey === routeKey))
              report.errors.push(
                `${key} candidate successful /api/session-trace response was not captured for ${routeKey}`,
              );
          }
          const referenceRoute = selfParity
            ? route.route
            : Object.hasOwn(route, 'referenceRoute')
              ? route.referenceRoute
              : route.route;
          if (!referenceRoute) throw new Error('original reference route mapping is unavailable');
          await navigate(reference, referenceRoute, referenceData);
          if (paintDiagnostic && frame === HARBOR_FRAME && paintArm === 'wheel-traversal') {
            await captureHarborWheelViewports(candidate, scheme, 'candidate');
            await captureHarborWheelViewports(reference, scheme, 'reference');
          }
          await captureFrame(candidate, reference, key, phone);
          if (paintDiagnostic && frame === HARBOR_FRAME && paintArm === 'programmatic-end') {
            await captureHarborProgrammaticViewports(candidate, scheme, 'candidate');
            await captureHarborProgrammaticViewports(reference, scheme, 'reference');
          }
          for (const region of coverage.regions
            .map((item) => item.split('|'))
            .filter(
              ([targetScheme, targetFrame]) => targetScheme === scheme && targetFrame === frame,
            )
            .map(([, , name]) => PIXEL_REGIONS.find((item) => item.name === name))) {
            const regionKey = `${scheme}|${frame}|${region.name}`;
            await captureRegion(candidate, reference, region, regionKey);
          }
          if (paintDiagnostic && frame === HARBOR_FRAME) break;
        } catch (error) {
          report.errors.push(`${key} navigation: ${error.message}`);
        }
      }
    }
    report.errors.push(
      ...candidate.errors.map((error) => `${scheme} candidate page error: ${error}`),
      ...reference.errors.map((error) => `${scheme} reference page error: ${error}`),
      ...retired
        .filter((url) =>
          /^\/api\/(?:model|transcript|expand|tx|tool|image)(?:\/|$)/.test(new URL(url).pathname),
        )
        .map((url) => `${scheme} retired candidate request: ${new URL(url).pathname}`),
    );
    await flushSessionTraceEvidence(candidate, scheme);
    for (const [side, page] of [
      ['candidate', candidate],
      ['reference', reference],
    ]) {
      report.apiDiagnostics.push(
        ...page.captureApiEvents.map((event) => ({ scheme, side, ...event })),
      );
      if (page.captureApiEventsTruncated)
        report.errors.push(
          `${scheme} ${side} API diagnostics truncated after ${page.captureApiEventsTruncated} events`,
        );
    }
    await candidate.context().close();
    await reference.context().close();
  }
} catch (error) {
  report.errors.push(error.stack ?? error.message);
} finally {
  report.homePostTagDiagnostics = homePostPages.map(({ scheme, page }) => ({
    scheme,
    ...homePostTagDiagnostics(page),
  }));
  await browser?.close();
  report.diagnosticOnly.coverageComplete = false;
  report.diagnosticOnly.capturedFrameCount = report.results.length;
  report.diagnosticOnly.capturedRegionCount = report.regions.length;
  report.diagnosticOnly.targetFrameResult =
    report.results.find((result) => result.key === `phone-dark|${HARBOR_FRAME}`) ?? null;
  fs.writeFileSync(path.join(ENV.out, 'pixels.json'), JSON.stringify(report, null, 2) + '\n');
  fs.writeFileSync(path.join(OUT, 'report.json'), JSON.stringify(report, null, 2) + '\n');
}
console.log(
  JSON.stringify({
    diagnosticOnly: true,
    arm: paintArm,
    capturedFrames: report.diagnosticOnly.capturedFrameCount,
    capturedRegions: report.diagnosticOnly.capturedRegionCount,
    targetFrameResult: report.diagnosticOnly.targetFrameResult,
    errors: report.errors.length,
  }),
);
if (report.errors.length) process.exitCode = 1;
