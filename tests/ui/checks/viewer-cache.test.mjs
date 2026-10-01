import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";

// Execute the production cache routine with a deferred navigation snapshot. The
// origin can arrive before that callback runs, even after LIVE.late was cleared.
const source = fs.readFileSync(new URL("../../../crates/semon-sessions/src/viewer.js", import.meta.url), "utf8");
const start = source.indexOf("  function cacheTx("), end = source.indexOf("\n  // A kept transcript", start);
const context = vm.createContext({});
vm.runInContext(`
  const TXCACHE = new Map(), STALE_BRIEFS = new Set(), TXCACHE_MAX = 5, TXCACHE_BYTES = 1024;
  let hasOrigin = false;
  const originHandoff = () => hasOrigin;
  const weigh = () => 10;
  ${source.slice(start, end)}
  globalThis.run = (originSnapshot, originNow, stale) => {
    TXCACHE.clear(); STALE_BRIEFS.clear(); hasOrigin = originNow;
    if (stale) STALE_BRIEFS.add('child');
    cacheTx('child', [{ k: 'u', text: 'brief' }], { total: 1, to: 1, tok: '1.1', origin: originSnapshot });
    return TXCACHE.has('child');
  };
`, context);
assert.equal(context.run(false, false, false), true, "ordinary complete transcript is cached");
assert.equal(context.run(false, true, false), false, "origin arrival invalidates deferred cache write");
assert.equal(context.run(true, true, true), false, "abandoned late reload never caches its duplicate brief");
assert.equal(context.run(true, true, false), true, "fresh reload can be cached again");
console.log("viewer cache race regressions passed");
