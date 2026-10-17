import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import {
  captureRequestOwnership,
  failedSelectedApiEvents,
  homePostTagDiagnostics,
} from './lib.mjs';
import {
  SESSION_TRACE_EVIDENCE_LIMITS,
  summarizeSessionTraceResponse,
} from './session-trace-evidence.mjs';

const request = {
  id: 1,
  method: 'GET',
  path: '/api/model',
  routeKey: 'selected',
  queryDigest: 'exact-query',
  retryScopeDigest: 'identity-and-options',
  at: 10,
};
const response = (overrides = {}) => ({
  ...request,
  event: 'response',
  status: 304,
  ok: false,
  ...overrides,
});
const aborted = (overrides = {}) => ({
  ...request,
  event: 'failed',
  error: 'net::ERR_ABORTED',
  ...overrides,
});
function failures(events, role = 'reference', routeKey = 'selected') {
  return failedSelectedApiEvents({
    captureApiEvents: events,
    captureRole: role,
    captureRouteKey: routeKey,
  });
}

const homeTagId = '0123456789abcdef-1',
  homePreviousRoute = JSON.stringify({ v: 'trace', id: 'atlas-ingest', turn: 'a-ask' });
function homeTagPage(overrides = {}) {
  const started = {
      id: 7,
      event: 'started',
      method: 'POST',
      path: '/api/viewer/home',
      routeKey: 'current',
      homePostTagId: homeTagId,
      homePostTagHeaderPresent: true,
    },
    failed = {
      ...started,
      event: 'failed',
      error: 'net::ERR_ABORTED',
    },
    finished = { ...started, event: 'finished', error: failed.error };
  return {
    captureHomePostTagEnabled: true,
    captureRole: 'candidate',
    captureRouteKey: 'current',
    captureHomePostTagInvocations: [{ id: homeTagId, routeMarker: homePreviousRoute }],
    captureHomePostTagInvocationsTruncated: 0,
    captureHomePostTagRequests: [
      {
        requestId: 7,
        tagId: homeTagId,
        tagHeaderPresent: true,
        routeKey: 'current',
      },
    ],
    captureHomePostTagRequestsTruncated: 0,
    captureApiEvents: [started, failed, finished],
    captureApiEventsTruncated: 0,
    ...overrides,
  };
}

test('Home POST diagnostic joins the exact request tag while preserving the fatal current failure', () => {
  const page = homeTagPage(),
    diagnostics = homePostTagDiagnostics(page),
    failure = page.captureApiEvents.find((event) => event.event === 'failed');
  assert.equal(diagnostics.requests[0].association, 'unique-exact-tag');
  assert.equal(diagnostics.requests[0].invocationRouteMarker, homePreviousRoute);
  assert.ok(diagnostics.requests[0].rawEvents.includes(failure));
  assert.deepEqual(failures(page.captureApiEvents, 'candidate', 'current'), [failure]);
});

test('Home POST diagnostic stays unknown for duplicate, malformed, or truncated associations', () => {
  const cases = [
    homeTagPage({
      captureHomePostTagRequests: [
        ...homeTagPage().captureHomePostTagRequests,
        { requestId: 8, tagId: homeTagId, tagHeaderPresent: true, routeKey: 'current' },
      ],
    }),
    homeTagPage({
      captureHomePostTagInvocations: [
        ...homeTagPage().captureHomePostTagInvocations,
        { id: homeTagId, routeMarker: homePreviousRoute },
      ],
    }),
    homeTagPage({ captureHomePostTagInvocations: [{ id: homeTagId, routeMarker: '' }] }),
    homeTagPage({ captureHomePostTagInvocations: [{ id: homeTagId, routeMarker: null }] }),
    homeTagPage({
      captureHomePostTagRequests: [
        { requestId: 7, tagId: null, tagHeaderPresent: false, routeKey: 'current' },
      ],
    }),
    homeTagPage({
      captureHomePostTagRequests: [
        { requestId: 7, tagId: null, tagHeaderPresent: true, routeKey: 'current' },
      ],
    }),
    homeTagPage({
      captureHomePostTagRequests: [
        {
          requestId: 7,
          tagId: 'fedcba9876543210-1',
          tagHeaderPresent: true,
          routeKey: 'current',
        },
      ],
    }),
    homeTagPage({
      captureApiEvents: homeTagPage().captureApiEvents.map((event) =>
        event.event === 'started' ? { ...event, homePostTagId: 'fedcba9876543210-1' } : event,
      ),
    }),
    homeTagPage({
      captureHomePostTagInvocations: [{ id: 'invalid-id', routeMarker: homePreviousRoute }],
      captureHomePostTagRequests: [
        { requestId: 7, tagId: 'invalid-id', tagHeaderPresent: true, routeKey: 'current' },
      ],
      captureApiEvents: homeTagPage().captureApiEvents.map((event) =>
        event.event === 'started' ? { ...event, homePostTagId: 'invalid-id' } : event,
      ),
    }),
    homeTagPage({
      captureHomePostTagRequests: [
        { requestId: 7, tagId: homeTagId, tagHeaderPresent: false, routeKey: 'current' },
      ],
    }),
    homeTagPage({
      captureHomePostTagInvocations: [{ id: homeTagId, routeMarker: 'x'.repeat(513) }],
    }),
    homeTagPage({
      captureHomePostTagInvocationsTruncated: 1,
    }),
    homeTagPage({
      captureHomePostTagRequestsTruncated: 1,
    }),
    homeTagPage({ captureApiEventsTruncated: 1 }),
  ];
  for (const page of cases)
    assert.equal(homePostTagDiagnostics(page).requests[0].association, 'unknown');
});

test('the reference bodyless model 304 retains raw events without a failed read', () => {
  const events = [response(), aborted()];
  assert.deepEqual(failures(events), []);
  assert.equal(events.length, 2);
  assert.equal(events[1].error, 'net::ERR_ABORTED');
});

test('the reference 304 exemption requires the exact already-recorded request', () => {
  for (const change of [
    { id: 2 },
    { method: 'POST' },
    { path: '/api/tx' },
    { routeKey: 'previous' },
    { queryDigest: 'different-query' },
    { at: 11 },
  ]) {
    const error = aborted();
    assert.ok(failures([response(change), error]).includes(error), JSON.stringify(change));
  }
  assert.equal(failures([aborted()]).length, 1);
  assert.equal(failures([aborted(), response()]).length, 1);
});

test('an aborted 200 response remains a failed body read', () => {
  assert.equal(failures([response({ status: 200, ok: true }), aborted()]).length, 1);
});

test('a candidate aborted selected read is not excused by a 304 response', () => {
  const path = '/api/session-entry';
  assert.equal(failures([response({ path }), aborted({ path })], 'candidate').length, 1);
});

test('selected expansion failures remain fatal while optional observations remain diagnostic', () => {
  assert.equal(
    failures([response({ path: '/api/session-entry', status: 503 })], 'candidate').length,
    1,
  );
  const events = [response({ path: '/api/session-observe', status: 503 })];
  assert.deepEqual(failures(events, 'candidate'), []);
  assert.equal(events[0].status, 503);
});

test('409 recovery requires a later successful request for the same identity and options', () => {
  const conflict = response({ path: '/api/session-home', status: 409 });
  const recovered = response({
    id: 2,
    path: '/api/session-home',
    status: 200,
    ok: true,
    queryDigest: 'new-generation',
  });
  assert.deepEqual(failures([conflict, recovered], 'candidate'), []);
  for (const change of [
    { id: 1 },
    { retryScopeDigest: 'different-identity-or-options' },
    { routeKey: 'previous' },
    { status: 500, ok: false },
  ])
    assert.ok(failures([conflict, { ...recovered, ...change }], 'candidate').includes(conflict));
});

test('initial selected failures are retained until initial readiness completes', () => {
  const error = aborted({ path: '/api/session-entry', routeKey: 'initial' });
  assert.deepEqual(failures([error], 'candidate', 'initial'), [error]);
  assert.deepEqual(failures([error], 'candidate', 'later-selection'), []);
});

const previousTrace = {
  ...request,
  path: '/api/session-trace',
  routeKey: 'quill',
  queryDigest: 'exact-quill-query-with-machine-and-options',
  status: 200,
  ok: true,
};
function traceTransition(
  events = [
    { ...previousTrace, event: 'response' },
    { ...previousTrace, event: 'finished' },
  ],
) {
  return {
    captureRole: 'candidate',
    captureRouteKey: 'sentinel',
    captureRouteTransition: {
      from: 'quill',
      to: 'sentinel',
      expectedQueryDigest: 'exact-sentinel-query-with-machine-and-options',
    },
    captureApiEvents: events,
  };
}
const latePreviousTrace = {
  ...previousTrace,
  id: 2,
  routeKey: 'sentinel',
};

test('an exact completed previous Trace poll retains its owner during navigation handoff', () => {
  const page = traceTransition();
  const owner = captureRequestOwnership(page, latePreviousTrace);
  assert.equal(owner.routeKey, 'quill');
  assert.equal(owner.captureRouteKeyAtStart, 'sentinel');
  assert.deepEqual(owner.ownerAttribution, {
    kind: 'completed-prior-trace-during-navigation',
    responseId: 1,
    expectedQueryDigest: 'exact-sentinel-query-with-machine-and-options',
  });
  const error = { ...latePreviousTrace, ...owner, event: 'failed', error: 'net::ERR_ABORTED' };
  page.captureApiEvents.push(error);
  assert.deepEqual(failedSelectedApiEvents(page), []);
  assert.ok(page.captureApiEvents.includes(error));
  assert.equal(error.error, 'net::ERR_ABORTED');
});

test('previous Trace ownership requires the complete exact prior request evidence', () => {
  for (const change of [
    { id: 2 },
    { method: 'POST' },
    { path: '/api/session-home' },
    { routeKey: 'unrelated' },
    { queryDigest: 'other-machine-turn-or-options' },
    { status: 409, ok: false },
    { status: 500, ok: false },
  ]) {
    const page = traceTransition([
      { ...previousTrace, event: 'response', ...change },
      { ...previousTrace, event: 'finished', ...change },
    ]);
    assert.deepEqual(captureRequestOwnership(page, latePreviousTrace), { routeKey: 'sentinel' });
  }
  for (const events of [
    [],
    [{ ...previousTrace, event: 'response' }],
    [
      { ...previousTrace, event: 'response' },
      { ...previousTrace, event: 'finished', error: 'net::ERR_ABORTED' },
    ],
  ])
    assert.deepEqual(captureRequestOwnership(traceTransition(events), latePreviousTrace), {
      routeKey: 'sentinel',
    });
});

test('current, unmatched, non-Trace and post-handoff aborts remain fatal', () => {
  for (const change of [
    { queryDigest: 'exact-sentinel-query-with-machine-and-options' },
    { queryDigest: 'unknown-query' },
    { method: 'POST' },
    { path: '/api/session-entry' },
  ]) {
    const page = traceTransition(),
      tracked = { ...latePreviousTrace, ...change },
      error = {
        ...tracked,
        ...captureRequestOwnership(page, tracked),
        event: 'failed',
        error: 'net::ERR_ABORTED',
      };
    page.captureApiEvents.push(error);
    assert.deepEqual(failedSelectedApiEvents(page), [error]);
  }
  const page = traceTransition();
  page.captureRouteTransition.expectedQueryDigest = latePreviousTrace.queryDigest;
  assert.deepEqual(captureRequestOwnership(page, latePreviousTrace), { routeKey: 'sentinel' });
  page.captureRouteTransition = null;
  assert.deepEqual(captureRequestOwnership(page, latePreviousTrace), { routeKey: 'sentinel' });
  page.captureRouteTransition = traceTransition().captureRouteTransition;
  page.captureRole = 'reference';
  assert.deepEqual(captureRequestOwnership(page, latePreviousTrace), { routeKey: 'sentinel' });
});

test('session trace evidence hashes exact bytes and retains only bounded graph identities', () => {
  const body = Buffer.from(
    JSON.stringify({
      api: 1,
      source_key: 'laptop',
      catalog_key: 'atlas-ingest',
      root: 't8',
      generation: 'a'.repeat(64),
      incomplete: true,
      previews_incomplete: false,
      sessions: {
        'atlas-ingest': { name: 'atlas ingest', harness: 'claude', kind: 'Root', cost: '$secret' },
      },
      participants: { harbor: 'harbor' },
      turns: [
        { id: 't8', sid: 'atlas-ingest', start: 'a-ask', sent: ['a-relay'], transcript: 'secret' },
      ],
      handoffs: [
        { id: 'a-ask', kind: 'ask', from: 'you', to: 'atlas-ingest', brief: 'secret' },
        { id: 'a-relay', kind: 'relay', from: 'atlas-ingest', to: 'harbor', result: 'secret' },
      ],
      toolBody: 'must not be retained',
    }),
  );

  const evidence = summarizeSessionTraceResponse(body);
  assert.deepEqual(evidence, {
    responseBytes: body.length,
    responseDigest: createHash('sha256').update(body).digest('hex'),
    api: 1,
    sourceKey: 'laptop',
    catalogKey: 'atlas-ingest',
    root: 't8',
    generation: 'a'.repeat(64),
    incomplete: true,
    previewsIncomplete: false,
    turns: [{ id: 't8', sid: 'atlas-ingest', start: 'a-ask', sent: ['a-relay'] }],
    handoffs: [
      { id: 'a-ask', kind: 'ask', from: 'you', to: 'atlas-ingest' },
      { id: 'a-relay', kind: 'relay', from: 'atlas-ingest', to: 'harbor' },
    ],
    sessions: [{ id: 'atlas-ingest', name: 'atlas ingest', harness: 'claude', kind: 'Root' }],
    participants: [{ id: 'harbor', name: 'harbor' }],
  });
  assert.doesNotMatch(JSON.stringify(evidence), /secret|toolBody/);
});

test('session trace evidence fails diagnostically on response and record bounds', () => {
  assert.throws(
    () =>
      summarizeSessionTraceResponse(Buffer.alloc(SESSION_TRACE_EVIDENCE_LIMITS.responseBytes + 1)),
    /response bytes .* exceeds limit/,
  );
  const oversizedTurns = Buffer.from(
    JSON.stringify({
      api: 1,
      incomplete: true,
      previews_incomplete: false,
      sessions: { x: { name: 'x', harness: 'claude' } },
      participants: {},
      turns: Array(SESSION_TRACE_EVIDENCE_LIMITS.turns + 1).fill({
        id: 'x',
        sid: 'x',
        start: null,
        sent: [],
      }),
    }),
  );
  assert.throws(
    () => summarizeSessionTraceResponse(oversizedTurns),
    /turns count .* exceeds limit/,
  );
});
