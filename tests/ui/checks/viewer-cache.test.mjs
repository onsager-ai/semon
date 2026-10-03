import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

// Execute the checked-in production cache and transcript owners. Module isolation
// follows the next generated module marker rather than assuming bundle ordering.
const bundle = fs.readFileSync(
  new URL('../../../crates/semon-sessions/src/viewer.generated.js', import.meta.url),
  'utf8',
);
function moduleSource(name) {
  const start = bundle.indexOf('  // ' + name),
    end = bundle.indexOf('\n  // ', start + 1);
  assert.ok(start >= 0 && end > start, 'production ' + name + ' controller is present');
  return bundle.slice(start, end);
}
const context = vm.createContext({});
vm.runInContext(moduleSource('src/lib/routes.ts'), context);
vm.runInContext(moduleSource('src/lib/live.ts'), context);
vm.runInContext(moduleSource('src/state/viewUpdates.ts'), context);
vm.runInContext(moduleSource('src/state/transcript.ts'), context);
vm.runInContext(
  `
  const store = new TranscriptStore({ request() { throw new Error('cache must not fetch'); }, turns() { return []; }, turn() {}, entry(e) { return e; }, cleared() {} });
  globalThis.run = (originSnapshot, originNow, stale) => {
    store.cache.clear(); store.staleBriefs.clear();
    if (stale) store.staleBriefs.add('child');
    store.keep('child', [{ k: 'u', text: 'brief' }], { total: 1, to: 1, from: 0, tok: '1.1', origin: originSnapshot }, originNow);
    return store.cache.has('child');
  };
`,
  context,
);
assert.equal(context.run(false, false, false), true, 'ordinary complete transcript is cached');
assert.equal(
  context.run(false, true, false),
  false,
  'origin arrival invalidates deferred cache write',
);
assert.equal(
  context.run(true, true, true),
  false,
  'abandoned late reload never caches its duplicate brief',
);
assert.equal(context.run(true, true, false), true, 'fresh reload can be cached again');
console.log('viewer cache race regressions passed');
