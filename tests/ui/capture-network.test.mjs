import { test } from 'node:test';
import assert from 'node:assert/strict';
import { failedSelectedApiEvents } from './lib.mjs';

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
