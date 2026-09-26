// The fixture's gaps: every place where the model served from the fixture (fixture.mjs) differs from the sample mockup's
// data, field by field and entry by entry. Each difference must be listed, with its reason, in gaps.json; a new one, or one
// that disappeared, fails the check.
//
//   node gaps.mjs --base http://127.0.0.1:PORT --token TOKEN     against a running `semon sessions --serve`
//   node gaps.mjs --model model.json --tx tx.json                against saved JSON
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { sample, BASE } from "./fixture.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const J = JSON.stringify;
const min = (t) => (t == null ? t : Math.round(((t - BASE) / 60000) * 1000) / 1000);

// A served transcript in full: every page, back from the last.
export async function served(base, token) {
  const get = async (p) => { const r = await fetch(base + p + (p.includes("?") ? "&" : "?") + "t=" + token); if (!r.ok) throw new Error(p + ": " + r.status); return r.json(); };
  const model = await get("/api/model"), tx = {};
  for (const sid of Object.keys(model.sessions)) {
    let page = await get("/api/tx?sid=" + encodeURIComponent(sid)), entries = page.entries;
    while (page.from > 0) { page = await get("/api/tx?sid=" + encodeURIComponent(sid) + "&before=" + page.from); entries = page.entries.concat(entries); }
    tx[sid] = entries;
  }
  return { model, tx };
}

// The longest common subsequence of two lists of strings, as [kept, removed from a, added in b].
function align(a, b) {
  const L = Array.from({ length: a.length + 1 }, () => Array(b.length + 1).fill(0));
  for (let i = a.length - 1; i >= 0; i--) for (let j = b.length - 1; j >= 0; j--) L[i][j] = a[i] === b[j] ? L[i + 1][j + 1] + 1 : Math.max(L[i + 1][j], L[i][j + 1]);
  const gone = [], added = [];
  let i = 0, j = 0;
  while (i < a.length && j < b.length) { if (a[i] === b[j]) { i++; j++; } else if (L[i + 1][j] >= L[i][j + 1]) gone.push(a[i++]); else added.push(b[j++]); }
  gone.push(...a.slice(i)); added.push(...b.slice(j));
  return [gone, added];
}

export function gaps({ model, tx }) {
  const S = sample(), out = [];
  const say = (x) => out.push(x);
  // Machines.
  const sampleMachines = Object.values(S.MACHINE), servedMachines = [model.machine.name];
  for (const m of sampleMachines) if (!servedMachines.includes(m)) say("machine " + m + ": not served");
  // Sessions.
  for (const [id, s] of Object.entries(S.SESS)) {
    const v = model.sessions[id];
    if (!v) { say("session " + id + ": not served"); continue; }
    const cmp = (field, a, b) => { if (J(a) !== J(b)) say("session " + id + "." + field + ": sample " + J(a) + ", served " + J(b)); };
    cmp("name", s.name, v.name); cmp("kind", s.kind, v.kind); cmp("lane", !!s.lane, !!v.lane); cmp("role", !!s.role, !!v.role);
    cmp("harness", s.harness, v.harness); cmp("machine", S.MACHINE[s.machine], v.machine === model.machine.id ? model.machine.name : v.machine);
    cmp("movedFrom", s.movedFrom, v.movedFrom); cmp("model", s.model, v.model); cmp("state", s.state, v.state); cmp("tokens", s.tokens, v.tokens);
    cmp("repo", s.repo, v.repo); cmp("branch", s.branch, v.branch); cmp("start", s.start, min(v.start)); cmp("last", s.last, min(v.last));
    cmp("busy", s.busy, v.busy.map(([a, b]) => [min(a), min(b)])); cmp("activity", s.activity, v.activity?.slice(0, 3));
  }
  for (const id of Object.keys(model.sessions)) if (!S.SESS[id]) say("session " + id + ": served, not in the sample");
  // Handoffs, matched by kind, ends and time.
  const ids = new Map(), used = new Set();
  for (const h of S.H) {
    const v = model.handoffs.find((x) => !used.has(x.id) && x.kind === h.kind && x.from === h.from && x.to === h.to && min(x.at) === h.at);
    if (!v) { say("handoff " + h.id + " (" + h.kind + (h.ask ? " " + h.ask : "") + " " + h.from + "→" + h.to + " at " + h.at + "): not served"); continue; }
    used.add(v.id); ids.set(v.id, h.id);
    const cmp = (field, a, b) => { if (J(a) !== J(b)) say("handoff " + h.id + "." + field + ": sample " + J(a) + ", served " + J(b)); };
    cmp("ask", h.ask, v.ask); cmp("status", h.status, v.status); cmp("done", h.done, min(v.done)); cmp("brief", h.brief, v.brief);
    cmp("result", h.result, v.result); cmp("answer", h.answer, v.answer);
  }
  for (const v of model.handoffs) if (!used.has(v.id)) say("handoff " + v.kind + (v.ask ? " " + v.ask : "") + " " + v.from + "→" + v.to + " at " + min(v.at) + ": served, not in the sample");
  // Transcripts, entry by entry, with the served handoff ids read as the sample's.
  const norm = (e) => {
    const x = { k: e.k };
    for (const f of ["text", "label", "name", "arg", "ok", "secs", "in", "out", "diff", "live", "unfinished"]) if (e[f] !== undefined) x[f] = e[f];
    if (e.k === "h") x.id = ids.get(e.id) ?? e.id;
    if (e.ret) x.text = "Returned to " + (model.sessions[e.ret.to]?.name ?? e.ret.to) + (e.ret.failed ? " · failed" : "") + " · " + new Date(e.ret.at).toISOString().slice(11, 16);
    return J(x);
  };
  for (const sid of Object.keys(S.TX)) {
    const [gone, added] = align(S.TX[sid].map(norm), (tx[sid] ?? []).map(norm));
    for (const g of gone) say("tx " + sid + ": sample entry not served: " + g);
    for (const a of added) say("tx " + sid + ": served entry not in the sample: " + a);
  }
  return out;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const arg = (k) => { const i = process.argv.indexOf("--" + k); return i > 0 ? process.argv[i + 1] : null; };
  const data = arg("base") ? await served(arg("base"), arg("token")) : { model: JSON.parse(fs.readFileSync(arg("model"), "utf8")), tx: Object.fromEntries(Object.entries(JSON.parse(fs.readFileSync(arg("tx"), "utf8"))).map(([k, p]) => [k, p.entries ?? p])) };
  const found = gaps(data);
  if (arg("write")) { fs.writeFileSync(arg("write"), J(found, null, 1) + "\n"); }
  const listed = JSON.parse(fs.readFileSync(path.join(here, "gaps.json"), "utf8"));
  const known = new Set(listed.map((g) => g.gap));
  const fresh = found.filter((g) => !known.has(g)), fixed = [...known].filter((g) => !found.includes(g));
  console.log("fixture gaps: " + found.length + " found, " + listed.length + " listed");
  for (const g of fresh) console.log("  NEW (not in gaps.json): " + g);
  for (const g of fixed) console.log("  GONE (listed, no longer found): " + g);
  if (fresh.length || fixed.length) process.exit(1);
}
