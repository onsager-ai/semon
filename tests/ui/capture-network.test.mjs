import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import {
  captureRequestOwnership,
  failedSelectedApiEvents,
  resolveHomePostPriorAbortOwners,
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
  homePreviousRoute = JSON.stringify({ v: 'trace', id: 'atlas-ingest', turn: 'a-ask' }),
  homeCurrentRoute = JSON.stringify({ v: 'trace', id: 'h-review', turn: 'h-answer' });
function homePostPage(overrides = {}) {
  const started = {
      id: 17,
      event: 'started',
      method: 'POST',
      path: '/api/viewer/home',
      routeKey: homeCurrentRoute,
      homePostTagId: homeTagId,
      homePostTagHeaderPresent: true,
    },
    failed = { ...started, event: 'failed', error: 'net::ERR_ABORTED' },
    finished = { ...started, event: 'finished', error: failed.error };
  return {
    captureRole: 'candidate',
    captureRouteKey: homeCurrentRoute,
    captureLastReadyRouteKey: homePreviousRoute,
    captureHomePostRouteTransition: {
      from: homePreviousRoute,
      to: homeCurrentRoute,
      fromAccepted: true,
    },
    captureHomePostTagEnabled: true,
    captureHomePostTagFlushComplete: true,
    captureHomePostTagInvocations: [{ id: homeTagId, routeMarker: homePreviousRoute }],
    captureHomePostTagInvocationsTruncated: 0,
    captureHomePostTagRequests: [
      {
        requestId: 17,
        tagId: homeTagId,
        tagHeaderPresent: true,
        routeKey: homeCurrentRoute,
      },
    ],
    captureHomePostTagRequestsTruncated: 0,
    captureApiEvents: [started, failed, finished],
    captureApiEventsTruncated: 0,
    ...overrides,
  };
}

function homeFailureState(page) {
  const owners = resolveHomePostPriorAbortOwners(page);
  page.captureHomePostOwners = owners;
  return { owners, failures: failedSelectedApiEvents(page) };
}

test('only an exact prior-route Home POST abort is excluded from the current capture gate', () => {
  const page = homePostPage(),
    failure = page.captureApiEvents.find((event) => event.event === 'failed'),
    owners = resolveHomePostPriorAbortOwners(page);
  assert.equal(owners.size, 1);
  assert.deepEqual(owners.get(17), {
    ownerRouteKey: homePreviousRoute,
    tagId: homeTagId,
    invocationRouteMarker: homePreviousRoute,
    nodeRouteKey: homeCurrentRoute,
  });
  page.captureHomePostOwners = owners;
  assert.deepEqual(failedSelectedApiEvents(page), []);
  assert.ok(page.captureApiEvents.includes(failure));
  assert.equal(failure.error, 'net::ERR_ABORTED');
});

test('repeated readiness keeps the accepted transition proof; a later navigation replaces it', () => {
  const page = homePostPage();
  page.captureHomePostOwners = resolveHomePostPriorAbortOwners(page);
  assert.equal(page.captureHomePostOwners.size, 1);

  // The first readiness pass accepts the destination. A repeated pass for the
  // same navigation still uses its immutable accepted-from provenance.
  page.captureLastReadyRouteKey = homeCurrentRoute;
  page.captureHomePostOwners = resolveHomePostPriorAbortOwners(page);
  assert.equal(page.captureHomePostOwners.size, 1);
  assert.deepEqual(failedSelectedApiEvents(page), []);

  const nextRoute = JSON.stringify({ v: 'session', id: 'deps' });
  page.captureHomePostRouteTransition = {
    from: homeCurrentRoute,
    to: nextRoute,
    fromAccepted: true,
  };
  page.captureRouteKey = nextRoute;
  page.captureHomePostOwners = resolveHomePostPriorAbortOwners(page);
  assert.equal(page.captureHomePostOwners.size, 0);

  const nextFailure = {
    id: 18,
    event: 'failed',
    method: 'POST',
    path: '/api/viewer/home',
    routeKey: nextRoute,
    error: 'net::ERR_ABORTED',
  };
  page.captureApiEvents.push(nextFailure);
  assert.deepEqual(failedSelectedApiEvents(page), [nextFailure]);
});

test('current, unknown, duplicate, malformed, truncated and unaccepted Home POST ownership stays fatal', () => {
  const start = homePostPage().captureApiEvents[0],
    request = homePostPage().captureHomePostTagRequests[0],
    invocation = homePostPage().captureHomePostTagInvocations[0],
    cases = [
      homePostPage({ captureHomePostTagFlushComplete: false }),
      homePostPage({
        captureHomePostRouteTransition: {
          from: homePreviousRoute,
          to: homeCurrentRoute,
          fromAccepted: false,
        },
      }),
      homePostPage({
        captureHomePostTagInvocations: [{ ...invocation, routeMarker: homeCurrentRoute }],
      }),
      // A Node request with no delivered page binding has no exact owner.
      homePostPage({ captureHomePostTagInvocations: [] }),
      homePostPage({
        captureHomePostTagInvocations: [invocation, invocation],
      }),
      homePostPage({
        captureHomePostTagRequests: [request, { ...request, requestId: 18 }],
      }),
      homePostPage({ captureHomePostTagRequests: [request, { ...request }] }),
      homePostPage({ captureHomePostTagInvocations: [{ ...invocation, routeMarker: 'invalid' }] }),
      homePostPage({ captureHomePostTagInvocations: [{ ...invocation, routeMarker: '' }] }),
      homePostPage({
        captureHomePostTagInvocations: [
          {
            ...invocation,
            routeMarker: JSON.stringify({ v: 'trace', id: 'atlas-ingest', turn: '' }),
          },
        ],
      }),
      homePostPage({ captureHomePostTagInvocationsTruncated: 1 }),
      homePostPage({ captureHomePostTagRequestsTruncated: 1 }),
      homePostPage({ captureApiEventsTruncated: 1 }),
      homePostPage({
        captureHomePostTagRequests: [{ ...request, tagHeaderPresent: false }],
      }),
      homePostPage({
        captureHomePostTagRequests: [{ ...request, requestId: 99 }],
      }),
      homePostPage({
        captureHomePostTagInvocations: [{ ...invocation, id: 'invalid-tag' }],
        captureHomePostTagRequests: [{ ...request, tagId: 'invalid-tag' }],
        captureApiEvents: homePostPage().captureApiEvents.map((event) =>
          event.event === 'started' || event.event === 'failed' || event.event === 'finished'
            ? { ...event, homePostTagId: 'invalid-tag' }
            : event,
        ),
      }),
      homePostPage({
        captureApiEvents: [
          { ...start, homePostTagId: 'fedcba9876543210-1' },
          ...homePostPage().captureApiEvents.slice(1),
        ],
      }),
      homePostPage({
        captureApiEvents: homePostPage().captureApiEvents.map((event, index) =>
          index === 0 ? { ...event, routeKey: homePreviousRoute } : event,
        ),
      }),
      homePostPage({
        captureHomePostTagRequests: [{ ...request, requestId: Number.MAX_SAFE_INTEGER + 1 }],
        captureApiEvents: homePostPage().captureApiEvents.map((event) => ({
          ...event,
          id: Number.MAX_SAFE_INTEGER + 1,
        })),
      }),
      homePostPage({
        captureApiEvents: [start, ...homePostPage().captureApiEvents],
      }),
      homePostPage({
        captureApiEvents: [...Array(9).fill(start), ...homePostPage().captureApiEvents.slice(1)],
      }),
    ];
  for (const page of cases) {
    const failed = page.captureApiEvents.find((event) => event.event === 'failed');
    const result = homeFailureState(page);
    assert.equal(result.owners.size, 0);
    assert.deepEqual(result.failures, [failed]);
  }
});

test('prior Home ownership never waives a non-abort failure or an HTTP non-2xx response', () => {
  const nonAbort = homePostPage({
      captureApiEvents: homePostPage().captureApiEvents.map((event) =>
        event.event === 'failed' ? { ...event, error: 'net::ERR_CONNECTION_RESET' } : event,
      ),
    }),
    httpFailure = homePostPage({
      captureApiEvents: [
        ...homePostPage().captureApiEvents.slice(0, 1),
        { ...homePostPage().captureApiEvents[0], event: 'response', status: 503, ok: false },
        ...homePostPage().captureApiEvents.slice(1),
      ],
    });
  const nonAbortResult = homeFailureState(nonAbort),
    httpFailureResult = homeFailureState(httpFailure);
  assert.equal(nonAbortResult.owners.size, 0);
  assert.equal(nonAbortResult.failures.length, 1);
  assert.equal(httpFailureResult.owners.size, 0);
  const httpErrors = httpFailureResult.failures;
  assert.ok(httpErrors.some((event) => event.event === 'response' && event.status === 503));
  assert.ok(httpErrors.some((event) => event.event === 'failed'));
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
