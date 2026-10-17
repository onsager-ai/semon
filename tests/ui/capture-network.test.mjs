import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { failedSelectedApiEvents } from './lib.mjs';
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
