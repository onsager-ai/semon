import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
const source = fs.readFileSync(new URL("../../../crates/semon-sessions/src/viewer.js", import.meta.url), "utf8");
const start = source.indexOf("  function applyModelDelta("), end = source.indexOf("\n  function update(m)", start);
const context = vm.createContext({});
vm.runInContext(`let wireModel; ${source.slice(start, end)}; globalThis.apply = (base, delta) => { wireModel = base; return applyModelDelta(delta); };`, context);
const base = { version: "v1", sessions: { a: { signals: { question: 1 } }, b: { name: "removed" } }, handoffs: [{ id: "h1" }, { id: "h2" }], turns: [{ id: "t1" }, { id: "t2" }], account: { name: "old" }, tx: { a: "old" } };
const delta = { delta: 1, from: "v1", version: "v2", set: { now: 2, machines: [{ id: "m", up: false }] }, remove: ["account"], collections: {
  sessions: { set: { a: { signals: { question: 0, permission: 2 } } }, remove: ["b"] },
  tx: { set: { a: "new" }, remove: [] },
  handoffs: { set: { h3: { id: "h3" } }, remove: ["h1"], order: ["h3", "h2"] },
  turns: { set: { t1: { id: "t1", end: { why: "answer" } } }, remove: [], order: ["t2", "t1"] }
} };
const before = JSON.stringify(base);
const out = JSON.parse(JSON.stringify(context.apply(base, delta)));
assert.deepEqual(out.sessions, { a: { signals: { question: 0, permission: 2 } } });
assert.deepEqual(out.handoffs.map((row) => row.id), ["h3", "h2"]);
assert.deepEqual(out.turns.map((row) => row.id), ["t2", "t1"]);
assert.equal(out.turns[1].end.why, "answer");
assert.equal(out.tx.a, "new");
assert.equal(out.account, undefined);
assert.equal(out.version, "v2");
assert.equal(JSON.stringify(base), before, "delta leaves its baseline intact");
assert.throws(() => context.apply(base, { ...delta, from: "expired" }), /base expired/);
assert.throws(() => context.apply(base, { ...delta, collections: { turns: { set: {}, remove: [], order: ["missing"] } } }), /Incomplete/);
console.log("model delta protocol regressions passed");
