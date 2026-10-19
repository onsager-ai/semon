// CI-only native publication records for production presentation owners.
// Uses the existing fixture reader and served navigation, including their auth.
// This is not an application/transport stub and is never shipped in the viewer.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { createHash } from 'node:crypto';
import { ENV, data, launch, served, goto, waitForCaptureReady } from './lib.mjs';
import { legacyCapturePlan } from './storybook-legacy.mjs';
import { nativeRoutes } from './storybook-native-routes.mjs';

assert.equal(process.env.GITHUB_ACTIONS, 'true', 'Native presentation recording is CI-only');
assert.match(process.env.GITHUB_SHA ?? '', /^[a-f0-9]{40}$/);
const publicationPaths = new Set([
  '/api/viewer/home', '/api/viewer/analytics', '/api/sessions',
  '/api/session-capabilities', '/api/session-transcript', '/api/session-home', '/api/session-trace',
]);
const output = new URL('../../ui/stories/LegacyNative.records.json', import.meta.url);
if (process.argv[2] === '--merge') {
  const root = process.argv[3];
  assert.ok(root && process.argv.length === 4);
  const locations = {
    main: 'viewer-ui-visual/storybook/native-records/records.json',
    'two-machines': 'viewer-ui-transport/two-machines/storybook/native-records/records.json',
    pushed: 'viewer-ui-transport/pushed/storybook/native-records/records.json',
  };
  const merged = { version: 1, sourceRevision: process.env.GITHUB_SHA, scopes: {} };
  for (const [scope, location] of Object.entries(locations)) {
    const fragment = JSON.parse(fs.readFileSync(new URL(location, new URL('file://' + root.replace(/\/$/, '') + '/')), 'utf8'));
    assert.equal(fragment.version, 1); assert.equal(fragment.sourceRevision, process.env.GITHUB_SHA);
    assert.ok(fragment.scopes[scope]); merged.scopes[scope] = fragment.scopes[scope];
  }
  fs.writeFileSync(output, JSON.stringify(merged, null, 2) + '\n');
  process.exit(0);
}
assert.equal(process.argv.length, 2);
const scope = process.env.SEMON_STORYBOOK_NATIVE_SCOPE;
assert.ok(['main', 'two-machines', 'pushed'].includes(scope), 'Explicit native fixture scope required');
const records = JSON.parse(fs.readFileSync(output, 'utf8'));
assert.ok(records.sourceRevision === null || records.sourceRevision === process.env.GITHUB_SHA);
records.sourceRevision = process.env.GITHUB_SHA;
records.fixtureProducer = 'tests/ui/fixture.mjs';
const D = await data(), routes = nativeRoutes(D), frames = {};
const required = [...new Set(legacyCapturePlan().filter((item) =>
  item.legacyKey.startsWith('pixels:' + scope + ':') ||
  (scope === 'main' && item.legacyKey.startsWith('preservation:'))).map((item) => item.fixture.frame))];
const browser = await launch({ baselineRaster: true });
try {
  for (const frame of required) {
    assert.ok(routes[frame], 'Missing native fixture route ' + scope + '/' + frame);
    const page = await served(browser, { size: 'desktop', dark: false, captureReadiness: true, captureRole: 'candidate' });
    const responses = [], tasks = [];
    page.on('response', (response) => {
      const url = new URL(response.url());
      if (url.origin !== new URL(ENV.base).origin || !publicationPaths.has(url.pathname) || !response.ok()) return;
      // Only JSON presentation publications, never cookies, headers or tokens.
      if (!response.headers()['content-type']?.includes('application/json')) return;
      url.searchParams.delete('t');
      const slot = responses.length;
      responses.push(null);
      tasks.push((async () => { responses[slot] = {
        path: url.pathname + url.search, value: await response.json(),
      }; })());
    });
    try {
      await page.reload({ waitUntil: 'networkidle' });
      await goto(page, routes[frame], D);
      await waitForCaptureReady(page, 'native presentation record ' + scope + '/' + frame);
      await Promise.all(tasks);
      if (page.errors.length) throw Error(page.errors.join('; '));
      frames[frame] = {
        route: routes[frame].v === 'machine' ? { v: 'machine', id: D.SOURCES[routes[frame].id] } :
          routes[frame].v === 'session' ? { v: 'session', id: D.KEYS[routes[frame].id] } :
          routes[frame].v === 'trace' ? { v: 'trace', sid: D.KEYS[routes[frame].sid], turn: routes[frame].turn } : routes[frame],
        source: routes[frame].v === 'machine' ? D.SOURCES[routes[frame].id] :
          D.SOURCES[D.SESS[routes[frame].id ?? routes[frame].sid]?.machine] ?? null,
        sourceLabels: D.MACHINE,
        sourceKeys: D.SOURCES,
        responses,
      };
    } finally { await page.context().close(); }
  }
} finally { await browser.close(); }
records.scopes[scope] = frames;
fs.writeFileSync(output, JSON.stringify(records, null, 2) + '\n');
const bytes = fs.readFileSync(output);
const artifact = new URL('file://' + ENV.out.replace(/\/$/, '') + '/storybook/native-records/');
fs.mkdirSync(artifact, { recursive: true });
fs.copyFileSync(output, new URL('records.json', artifact));
fs.writeFileSync(new URL('provenance.json', artifact), JSON.stringify({
  sourceRevision: records.sourceRevision, scope, now: ENV.now,
  producer: 'tests/ui/fixture.mjs', reader: 'tests/ui/lib.mjs nativeFixtureData',
  sha256: createHash('sha256').update(bytes).digest('hex'), frames: Object.keys(frames),
}, null, 2) + '\n');
