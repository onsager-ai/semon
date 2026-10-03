import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
// Exercise the checked-in production controller, including its runtime wire validation.
const source = fs.readFileSync(new URL("../../../crates/semon-sessions/src/viewer.generated.js", import.meta.url), "utf8");
const start = source.indexOf("  // src/lib/model.ts"), end = source.indexOf("\n  // ", start + 3);
assert.ok(start >= 0 && end > start, "production model controller is present");
const context = vm.createContext({ structuredClone });
vm.runInContext(`${source.slice(start, end)}; globalThis.apply = (base, delta) => { const store = new ModelStore(); store.adopt(base); return store.apply(delta); };`, context);
const session = fields => ({ name: "Session", harness: "claude", state: "idle", machine: "m", start: 1, last: 1, ...fields });
const handoff = id => ({ id, kind: "relay", from: "a", at: 1, status: "done" });
const turn = id => ({ id, sid: "a", sent: [] });
const base = { now: 1, machine: { id: "m", name: "Machine", up: true }, version: "v1", sessions: { a: session({ signals: { question: 1 } }), b: session({ name: "removed" }) }, handoffs: [handoff("h1"), handoff("h2")], turns: [turn("t1"), turn("t2")], account: { name: "old" }, tx: { a: "old" } };
const delta = { delta: 1, from: "v1", version: "v2", set: { now: 2, machines: [{ id: "m", name: "Machine", up: false }] }, remove: ["account"], collections: {
  sessions: { set: { a: session({ signals: { question: 0, permission: 2 } }) }, remove: ["b"] },
  tx: { set: { a: "new" }, remove: [] },
  handoffs: { set: { h3: handoff("h3") }, remove: ["h1"], order: ["h3", "h2"] },
  turns: { set: { t1: { ...turn("t1"), end: { why: "answer" } } }, remove: [], order: ["t2", "t1"] }
} };
const before = JSON.stringify(base);
const out = JSON.parse(JSON.stringify(context.apply(base, delta)));
assert.deepEqual(out.sessions, { a: session({ signals: { question: 0, permission: 2 } }) });
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
