// Pixel comparison of every screen of the served viewer against the mockup, at 390×844 light and dark and 1280×860.
//
// Which mockup a screen is compared with is set per screen in reference-map.json, by name (a name covers itself and every
// "<name>-…" screen):
//   "sample"    the previous mockup (reference/semon-sample.html): enforced
//   "overhaul"  the overhaul mockup (reference/overhaul.html): enforced. A screen's port PR flips it here.
//   "pending"   a screen whose port PR hasn't landed but whose look has already moved on (the overhaul's tokens are global, so
//               the previous mockup no longer describes it): compared with the overhaul mockup and reported, not enforced
//   null        a screen the viewer doesn't draw yet (loading, empty, error, not found): not compared
//
// Two comparisons per screen, both against the screen's mapped mockup rendered in the same browser:
//
//   port    The mockup's own code on the served data: the mockup file with its data block replaced by what /api/model and
//           /api/tx serve, and with the clock formatters and Analytics timestamp adapter changed for served epoch milliseconds.
//           This isolates the frontend port:
//           the served page must match it within the anti-aliasing tolerance. Enforced.
//   sample  The previous sample mockup exactly as committed, with its own data. The differences are the fixture's gaps (gaps.json:
//           one machine, no moves, …) and what the overhaul fixture added (a screen of a session the sample lacks isn't compared),
//           so this one is reported with its diff images, not enforced.
//
// Fonts: both sides use the vendored woff2 files (the reference's Google Fonts request is answered with them).
// Clock: both pages stand at the fixture's now, in UTC.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { PNG } from "pngjs";
import pixelmatch from "pixelmatch";
import { ENV, launch, context, served, goto, data } from "./lib.mjs";
import { sample, BASE } from "./fixture.mjs";
import { overhaulPortReference, stableOverhaul } from "./overhaul-port.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const J = JSON.stringify;
// Set once, before any run: pixelmatch's per-pixel colour threshold with anti-aliased pixels ignored, and at most this
// share of the other pixels may differ. Never raise these to make a run pass.
const THRESHOLD = 0.1;
const MAX_RATIO = 0.0001;
// SEMON_PIXEL_SCHEMES=all adds 1280×860 dark.
const SCHEMES = [["phone", false], ["phone", true], ["desktop", false], ...(process.env.SEMON_PIXEL_SCHEMES === "all" ? [["desktop", true]] : [])];
// SEMON_PIXEL_SCREENS=a,b compares only the screens whose names start with one of these (all by default).
const ONLY = (process.env.SEMON_PIXEL_SCREENS ?? "").split(",").map((x) => x.trim()).filter(Boolean);
const FONTS = path.join(here, "../../crates/semon-sessions/src/fonts");
// The brand mark, which the reference origin serves at /mark.svg as the served viewer does: from the crate's own file.
const MARK_SVG = fs.readFileSync(path.join(here, "../../crates/semon-sessions/src/mark.svg"));
const OUT = path.join(ENV.out, "pixels");

const MAP = JSON.parse(fs.readFileSync(path.join(here, "reference-map.json"), "utf8"));
for (const [name, file] of Object.entries(MAP.references)) if (!fs.existsSync(path.join(here, file))) throw new Error("reference-map.json: " + name + ": " + file + " is missing");
for (const [screen, ref] of Object.entries(MAP.screens)) if (ref !== null && ref !== "pending" && !(ref in MAP.references)) throw new Error("reference-map.json: " + screen + " names an unknown reference: " + ref);
// The reference a screen is compared with: the longest map name that is the screen's name or its prefix before a "-". `null` means
// unmapped (no comparison); undefined means the map has no entry, which is an error, so a new screen can't go unmapped by accident.
function referenceOf(name) {
  const keys = Object.keys(MAP.screens).filter((k) => name === k || name.startsWith(k + "-")).sort((a, b) => b.length - a.length);
  return keys.length ? MAP.screens[keys[0]] : undefined;
}
const MOCKUP = fs.readFileSync(path.join(here, MAP.references.sample), "utf8");
const OVERHAUL = fs.readFileSync(path.join(here, MAP.references.overhaul), "utf8");
function stableMockup(html) {
  if (html.includes('if (route.v !== "session" || loadingView || panelEl) return;')) return stableOverhaul(html);
  const liveDemo = 'if (route.v !== "session") return;';
  if (html.split(liveDemo).length !== 2) throw new Error("mockup demo tick guard moved");
  return html.replace(liveDemo, 'if (window.__SEMON_PIXEL_COMPARE || route.v !== "session") return;');
}
const DATA_START = "  const T = (h, m) => h * 60 + m; const ST = (h, m, s = 0) => (T(h, m) * 60 + s) * 1000; const NOW = T(12, 40);";
const DATA_END = "  // ====================================================================================\n  const $ = ";
const CLOCKS = [
  ['  const clock = (m) => String(Math.floor(m / 60)).padStart(2, "0") + ":" + String(m % 60).padStart(2, "0");',
    '  const clock = (t) => { const d = new Date(t), n = new Date(NOW); const hm = String(d.getHours()).padStart(2, "0") + ":" + String(d.getMinutes()).padStart(2, "0"); return d.toDateString() === n.toDateString() ? hm : d.toLocaleDateString(undefined, { weekday: "short" }) + " " + hm; };'],
  ['  const ago = (m) => { const d = NOW - m; return d < 1 ? "now" : d < 60 ? d + "m" : Math.floor(d / 60) + "h"; };',
    '  const ago = (t) => { const d = Math.floor((NOW - t) / 60000); return d < 1 ? "now" : d < 60 ? d + "m" : d < 2880 ? Math.floor(d / 60) + "h" : Math.floor(d / 1440) + "d"; };'],
  ['  const dur = (a, b) => { const d = (b ?? NOW) - a; return d >= 60 ? Math.floor(d / 60) + "h " + (d % 60) + "m" : d + "m"; };',
    '  const dur = (a, b) => { const d = Math.max(0, Math.floor(((b ?? NOW) - a) / 60000)); return d >= 1440 ? Math.floor(d / 1440) + "d " + Math.floor((d % 1440) / 60) + "h" : d >= 60 ? Math.floor(d / 60) + "h " + (d % 60) + "m" : d + "m"; };'],
];
const ANALYTICS_LINES = [
  ['  const analyticsAt = ([daysAgo, minute]) => ANALYTICS_DAY0 - daysAgo * DAY_MS + minute * 60000;',
    '  const analyticsAt = ([daysAgo, minute]) => Number(minute) > 1e11 ? Number(minute) : ANALYTICS_DAY0 - daysAgo * DAY_MS + minute * 60000;'],
];
// The server's transcript endpoint owns the complete tool-call and error counts; the static mockup normally counts its
// sample TX arrays directly. In port mode use the same per-session marks as the served viewer.
const CALL_LINES = [
  ['    const es = TX[s.id] ?? [], calls = es.filter((e) => e.k === "tool").length, errors = es.filter((e) => e.k === "tool" && e.ok === false).length, nT = (TURNS[s.id] ?? []).filter(hasTurn).length;',
    '    const es = TX[s.id] ?? [], calls = TXM[s.id]?.calls ?? es.filter((e) => e.k === "tool").length, errors = TXM[s.id]?.errors ?? es.filter((e) => e.k === "tool" && e.ok === false).length, nT = (TURNS[s.id] ?? []).filter(hasTurn).length;'],
  ['    const block = el("div", "child-return"), calls = (TX[s.id] ?? []).filter((e) => e.k === "tool").length, finished =',
    '    const block = el("div", "child-return"), calls = TXM[s.id]?.calls ?? (TX[s.id] ?? []).filter((e) => e.k === "tool").length, finished ='],
  ['    const calls = (TX[s.id] ?? []).filter((e) => e.k === "tool").length, origin = originHandoff(s.id);',
    '    const calls = TXM[s.id]?.calls ?? (TX[s.id] ?? []).filter((e) => e.k === "tool").length, origin = originHandoff(s.id);'],
  ['    if (child) { const calls = (TX[child.id] ?? []).filter((e) => e.k === "tool").length, meta = el("div", "child-meta");',
    '    if (child) { const calls = TXM[child.id]?.calls ?? (TX[child.id] ?? []).filter((e) => e.k === "tool").length, meta = el("div", "child-meta");'],
];
// Port-mode entries already carry the server's measured duration; source-log timing fields stay in the sample only.
const THOUGHT_LINES = [
  ['  function thoughtSeconds(entries, i, sid) {\n    const e = entries[i];\n    if (SESS[sid]?.harness === "codex") return Number.isFinite(e.completed_at_ms) && Number.isFinite(e.started_at_ms) && e.completed_at_ms >= e.started_at_ms ? Math.round((e.completed_at_ms - e.started_at_ms) / 1000) : null;\n    const before = entryTimeMs(entries[i - 1]), at = entryTimeMs(e);\n    return Number.isFinite(before) && Number.isFinite(at) && at >= before ? Math.round((at - before) / 1000) : null;\n  }',
    '  function thoughtSeconds(entries, i, sid) { const secs = entries[i]?.secs; return Number.isFinite(secs) && secs >= 0 ? secs : null; }'],
  ['  const isPendingThought = (e) => !!(e.pending || e.status === "thinking");\n  const isMaskedThought = (e) => isThought(e) && !isPendingThought(e) && !thoughtText(e);',
    '  const isPendingThought = (e, entries, i, sid) => !!(e.pending || e.status === "thinking") || Array.isArray(entries) && e.k === "think" && !thoughtText(e) && i === entries.length - 1 && SESS[sid]?.state === "work";\n  const isMaskedThought = (e, entries, i, sid) => isThought(e) && !isPendingThought(e, entries, i, sid) && !thoughtText(e);'],
  ['      const row = { ...e, k: "think", displaySecs: thoughtSeconds(entries, i, sid) };',
    '      const pending = isPendingThought(e, entries, i, sid), row = { ...e, k: "think", ...(pending ? { pending: true } : {}), displaySecs: thoughtSeconds(entries, i, sid) };'],
];
// Parents show the API-equivalent cost of their own session and descendant runs, as the served viewer does.
const COST_LINES = [
  [`  const TOKEN_KINDS = [["input", "Input"], ["output", "Output"], ["cacheWrite", "Cache write"], ["cacheRead", "Cache read"]];
  const asMoney = (usd) => "$" + usd.toFixed(2), shortMoney = (usd) => "$" + usd.toFixed(1);
  const usageTotal = (s) => Object.values(s.tokensByModel ?? {}).reduce((sum, m) => sum + TOKEN_KINDS.reduce((n, [k]) => n + (Number(m[k]) || 0), 0), 0);
  function costForSessions(sessions) {
    const kinds = Object.fromEntries(TOKEN_KINDS.map(([key]) => [key, { tokens: 0, usd: 0 }])), models = new Map(), unknown = new Set(); let usd = 0;
    for (const s of sessions) for (const [modelId, usage] of Object.entries(s.tokensByModel ?? {})) {
      const price = API_PRICE[modelId], current = models.get(modelId) ?? { modelId, kinds: Object.fromEntries(TOKEN_KINDS.map(([key]) => [key, { tokens: 0, usd: 0 }])), usd: 0, priced: !!price };
      if (!price) { unknown.add(modelId); current.priced = false; }
      for (const [key] of TOKEN_KINDS) {
        const tokens = Number(usage[key]) || 0; current.kinds[key].tokens += tokens; kinds[key].tokens += tokens;
        if (price) { const amount = tokens * price[key] / 1e6; current.kinds[key].usd += amount; kinds[key].usd += amount; current.usd += amount; usd += amount; }
      }
      models.set(modelId, current);
    }
    return { usd: unknown.size ? null : usd, knownUsd: usd, kinds, models: [...models.values()], unknown: [...unknown] };
  }
  const costForSession = (sid, includeRuns = false) => costForSessions(SESS[sid] ? [SESS[sid], ...(includeRuns ? descendantsOf(sid, sessionChildren()) : [])] : []);
  const costText = (cost) => cost.unknown.length ? "—" : asMoney(cost.usd);
  `, `  const TOTAL_TOKEN_KINDS = ["input", "output", "cacheWrite", "cacheRead"];
  const TOKEN_KINDS = [["input", "Input"], ["output", "Output"], ["cache_read", "Cache read"], ["cache_write_5m", "Cache write · 5m"], ["cache_write_1h", "Cache write · 1h"], ["web_search", "Web search"]];
  const asMoney = (usd) => "$" + usd.toFixed(2), shortMoney = (usd) => "$" + usd.toFixed(1);
  const usageTotal = (s) => Object.values(s.tokensByModel ?? {}).reduce((sum, m) => sum + TOTAL_TOKEN_KINDS.reduce((n, k) => n + (Number(m[k]) || 0), 0), 0);
  function costForSessions(sessions) {
    const unknown = new Set(), models = new Map(), by_day = {}; let knownUsd = 0, allPriced = true;
    for (const s of sessions) {
      const cost = s.cost ?? {};
      if (cost.usd == null) allPriced = false; else knownUsd += Number(cost.usd) || 0;
      for (const modelId of cost.unpriced_models ?? []) unknown.add(modelId);
      for (const [day, amount] of Object.entries(cost.by_day ?? {})) by_day[day] = (by_day[day] ?? 0) + (Number(amount) || 0);
      for (const [modelId, model] of Object.entries(cost.by_model ?? {})) {
        const current = models.get(modelId) ?? { modelId, kinds: Object.fromEntries(TOKEN_KINDS.map(([key]) => [key, { tokens: 0, usd: 0 }])), usd: 0, priced: true };
        if (model.usd == null) current.priced = false; else current.usd += Number(model.usd) || 0;
        for (const [key] of TOKEN_KINDS) { current.kinds[key].tokens += Number(model.tokens?.[key]) || 0; current.kinds[key].usd += Number(model.usd_by_kind?.[key]) || 0; }
        models.set(modelId, current);
      }
    }
    return { usd: allPriced && !unknown.size ? knownUsd : null, knownUsd, models: [...models.values()], unknown: [...unknown].sort(), by_day };
  }
  const costForSession = (sid, includeRuns = false) => costForSessions(SESS[sid] ? [SESS[sid], ...(includeRuns ? descendantsOf(sid, sessionChildren()) : [])] : []);
  const costText = (cost) => cost.unknown.length || cost.usd == null ? "—" : asMoney(cost.usd);
  function costSpan(from, to) {
    const day = Math.floor(to / DAY_MS) * DAY_MS;
    return [day - (analyticsRange - 1) * DAY_MS, day + DAY_MS];
  }
  const wholeDay = (day, a, b) => { const start = Date.parse(day + "T00:00:00.000Z"); return Number.isFinite(start) && start >= a && start + DAY_MS <= b; };
  function sessionCostInRange(s, from, to) {
    const [spanFrom, spanTo] = costSpan(from, to), cost = s.cost ?? {}, days = Object.entries(cost.by_day ?? {}).filter(([day]) => wholeDay(day, spanFrom, spanTo));
    const unknown = days.length ? cost.unpriced_models ?? [] : [];
    return { usd: unknown.length ? null : days.reduce((sum, [, amount]) => sum + (Number(amount) || 0), 0), unknown, hasData: days.length > 0 };
  }
  function analyticsCost(rows, from, to) {
    const unknown = new Set(); let usd = 0;
    for (const row of rows) { const cost = sessionCostInRange(row.s, from, to); if (!cost.hasData) continue; usd += Number(cost.usd) || 0; cost.unknown.forEach((model) => unknown.add(model)); }
    return { usd: unknown.size ? null : usd, unknown: [...unknown] };
  }
  `],
  ['    const cost = costForSession(s.id), costItem = el("span", "meta-item meta-cost"); costItem.append(icon(I.coin), el("span", "meta-value", cost.unknown.length ? "—" : shortMoney(cost.usd))); costItem.title = COST_TIP + (cost.unknown.length ? " no price for " + cost.unknown.join(", ") : ""); costItem.setAttribute("aria-label", "API-equivalent cost " + costText(cost) + ". " + COST_TIP + (cost.unknown.length ? " no price for " + cost.unknown.join(", ") : ""));',
    '    const directRunsForCost = childSessions(s.id), descendantRunsForCost = descendantsOf(s.id, sessionChildren()), cost = directRunsForCost.length ? costForSessions([s, ...descendantRunsForCost]) : costForSession(s.id), costItem = el("span", "meta-item meta-cost"); costItem.append(icon(I.coin), el("span", "meta-value", (directRunsForCost.length ? "incl. runs " : "") + (cost.unknown.length ? "—" : shortMoney(cost.usd)))); costItem.title = "API-equivalent cost. " + COST_TIP + (cost.unknown.length ? " no price for " + cost.unknown.join(", ") : ""); costItem.setAttribute("aria-label", "API-equivalent cost " + costText(cost) + (directRunsForCost.length ? ", including runs" : "") + ". " + COST_TIP + (cost.unknown.length ? " no price for " + cost.unknown.join(", ") : ""));'],
  ['    const costRows = rows.filter((r) => inRange(r.costAt, from, to)).map((r) => ({ row: r, cost: costForSession(r.id) }));',
    '    const costRange = analyticsCost(rows, from, to);'],
  ['    const costUnknown = [...new Set(costRows.flatMap((x) => x.cost.unknown))];',
    '    const costUnknown = costRange.unknown;'],
  ['      longestCurrent: current[0] ?? null, waitBy, costRows, costUnknown,\n      apiCost: costUnknown.length ? null : costRows.reduce((sum, x) => sum + x.cost.knownUsd, 0),',
    '      longestCurrent: current[0] ?? null, waitBy, costUnknown,\n      apiCost: costRange.usd,'],
  ['    const bins = Array.from({ length: count }, () => ({ claude: 0, codex: 0 })), unpriced = new Set();\n    for (const row of rows) {\n      if (!inRange(row.costAt, from, to)) continue; const cost = costForSession(row.id); if (cost.usd == null) { cost.unknown.forEach((m) => unpriced.add(m)); continue; }\n      const at = Math.min(count - 1, Math.floor((row.costAt - from) / (to - from) * count)); bins[at][row.s.harness] += cost.usd;\n    }',
    '    const bins = Array.from({ length: count }, () => ({ claude: 0, codex: 0 })), unpriced = new Set(), [spanFrom, spanTo] = costSpan(from, to);\n    for (const row of rows) for (const [day, amount] of Object.entries(row.s.cost?.by_day ?? {})) {\n      if (!wholeDay(day, spanFrom, spanTo)) continue;\n      for (const model of row.s.cost?.unpriced_models ?? []) unpriced.add(model);\n      const index = Math.round((Date.parse(day + "T00:00:00.000Z") - spanFrom) / DAY_MS); bins[index][row.s.harness] += Number(amount) || 0;\n    }'],
  ['    panel.append(title, el("div", "panel-sub", "API-equivalent cost " + unit + " · stacked by harness"));',
    '    panel.append(title, el("div", "panel-sub", "API-equivalent cost per UTC day · today so far · stacked by harness"));'],
  ['    addMetric("API-equivalent cost", now.apiCost == null ? "—" : asMoney(now.apiCost), costNote, null, true);',
    '    addMetric(analyticsRange === 1 ? "Cost today (UTC)" : "Cost, last " + analyticsRange + " UTC days", now.apiCost == null ? "—" : asMoney(now.apiCost), costNote, null, true);'],
  ['      const ms = busyMsIn(row, from, to); if (!ms && !inRange(row.startedAt, from, to)) continue;\n      const key = keyFor(row.s), g = groups.get(key) ?? { key, ms: 0, cost: 0, unknown: new Set(), sessions: new Set() }; g.ms += ms; g.sessions.add(row.id);',
    '      const ms = busyMsIn(row, from, to), c = sessionCostInRange(row.s, from, to); if (!ms && !inRange(row.startedAt, from, to) && !c.hasData) continue;\n      const key = keyFor(row.s), g = groups.get(key) ?? { key, ms: 0, cost: 0, unknown: new Set(), sessions: new Set() }; g.ms += ms; g.sessions.add(row.id);'],
  ['      if (inRange(row.costAt, from, to)) { const c = costForSession(row.id); g.cost += c.knownUsd; c.unknown.forEach((x) => g.unknown.add(x)); }',
    '      if (c.hasData) { g.cost += Number(c.usd) || 0; c.unknown.forEach((x) => g.unknown.add(x)); }'],
  ['    for (const item of items) { const s = item.s, b = el("button", "analytics-session"); b.type = "button"; b.append(el("span", "session-name", s.name), harnessName(s.harness, true), el("span", "session-value", value(item))); if (item.cost?.unknown.length) b.append(el("span", "no-price", "no price for " + item.cost.unknown.join(", "))); b.addEventListener("click", () => goSession(s.id)); list.append(b); }',
    '    for (const item of items) { const s = item.s, b = el("button", "analytics-session"); b.type = "button"; b.append(el("span", "session-name", s.name), harnessName(s.harness, true), el("span", "session-value", value(item))); if (item.cost?.unknown.length) b.append(el("span", "no-price", "no price for " + item.cost.unknown.join(", "))); b.addEventListener("click", () => goSession(s.id)); list.append(b); }'],
  ['    const costTop = all.filter((r) => inRange(r.costAt, from, to)).map((r) => { const cost = costForSession(r.id); return { ...r, cost, value: cost.usd }; }).sort((a, b) => (b.value ?? -1) - (a.value ?? -1)).slice(0, 5);',
    '    const costTop = all.map((r) => { const rangeCost = sessionCostInRange(r.s, from, to), cost = { ...rangeCost, unknown: rangeCost.unknown }; return { ...r, cost, value: cost.usd }; }).filter((r) => r.cost.hasData).sort((a, b) => (b.value ?? -1) - (a.value ?? -1)).slice(0, 5);'],
];
// Port mode keeps the served result status and read behavior so the image comparison covers the chrome faithfully.
const RESULT_LINES = [
  ['  const inbox = () => H.filter((h) => h.kind === "toyou" && h.status === "wait").sort((a, b) => b.at - a.at);',
    '  const inbox = () => H.filter((h) => h.kind === "toyou" && (h.portStatus === "wait" || (h.ask === "result" && !SEEN_RESULTS.has(h.id)))).sort((a, b) => b.at - a.at);'],
  ['  const statWord = (h) => ({ work: "working", wait: "waiting on you", err: "failed", done: h.kind === "toyou" ? "answered" : h.result ? "returned" : "delivered" })[h.status];',
    '  const statWord = (h) => h.kind === "toyou" && h.ask === "result" ? SEEN_RESULTS.has(h.id) ? "read" : "new" : ({ work: "working", wait: "waiting on you", err: "failed", done: h.kind === "toyou" ? "answered" : h.result ? "returned" : "delivered" })[h.status];'],
  ['  function renderSession(page, sid) {\n    const s = SESS[sid], origin = originHandoff(sid), head = el("div", "ph sr");',
    '  function renderSession(page, sid) {\n    markSeenResults(H.filter((h) => h.kind === "toyou" && h.ask === "result" && h.from === sid));\n    const s = SESS[sid], origin = originHandoff(sid), head = el("div", "ph sr");'],
  ['        if (h.kind === "ask") { if (!show.messages) continue; const m = el("div", "msg user"); m.append(markdown(h.brief)); tx.append(m); if (cur?.t.start === h) tx.append(el("div", "msg-tm", clock(h.at))); continue; }\n        // A relay or brief that starts a turn is that turn\'s message.',
    '        if (h.kind === "ask") { if (!show.messages) continue; const m = el("div", "msg user"); m.append(markdown(h.brief)); tx.append(m); if (cur?.t.start === h) tx.append(el("div", "msg-tm", clock(h.at))); continue; }\n        if (h.kind === "toyou" && h.ask === "result") { const marker = el("div", "result-marker " + (SEEN_RESULTS.has(h.id) ? "read" : "new")); marker.append(icon(I.result), el("span", "word", statWord(h)), el("span", "tm", clock(h.at))); tx.append(marker); continue; }\n        // A relay or brief that starts a turn is that turn\'s message.'],
];
// A result stays `new` in the served model, but opening its session/trace makes the turn end read as done.
const TURN_RESULT_LINES = [
  ['    if (ty) return { st: ty.status === "wait" ? "wait" : "done", text: (TOYOU[ty.ask] ?? "Sent you a message") + " · " + statWord(ty) + " · " + clock(ty.at) };',
    '    if (ty) return { st: ty.status === "wait" && !(ty.ask === "result" && SEEN_RESULTS.has(ty.id)) ? "wait" : "done", text: (TOYOU[ty.ask] ?? "Sent you a message") + " · " + statWord(ty) + " · " + clock(ty.at) };'],
];
// The served trace marks result handoffs as read before it draws the compact row. Match that state and row in port mode.
const TRACE_RESULT_LINES = [
  ['    if (!root) { page.append(el("p", "empty", "This turn isn\'t in the logs on this machine.")); return; }\n    const flow = el("div", "flow"), seen = new Set([root.id]), sess = new Set([root.sid]); let n = 0;',
    '    if (!root) { page.append(el("p", "empty", "This turn isn\'t in the logs on this machine.")); return; }\n    const readTrace = (turn, visited = new Set()) => { if (!turn || visited.has(turn.id)) return; visited.add(turn.id); markSeenResults(turn.sent); for (const h of turn.sent) if (h.kind === "spawn" || h.kind === "relay") readTrace(STARTS.get(h.id), visited); };\n    readTrace(root);\n    const flow = el("div", "flow"), seen = new Set([root.id]), sess = new Set([root.sid]); let n = 0;'],
  ['        const c = h.kind === "spawn" || h.kind === "relay" ? STARTS.get(h.id) : null, tgt = h.kind === "toyou" ? h.from : h.to, [ic, parts] = sentence(h, null);',
    '        const result = h.kind === "toyou" && h.ask === "result", c = h.kind === "spawn" || h.kind === "relay" ? STARTS.get(h.id) : null, tgt = h.kind === "toyou" ? h.from : h.to, [ic, parts] = result ? [I.result, [el("span", "verb", statWord(h))]] : sentence(h, null);'],
  ['        const [x, b] = hop("child k-" + h.kind + " s-" + h.status + (c || h.kind === "toyou" || h.kind === "move" ? "" : " stub"), ic, hcls(tgt), parts, h.at); x.dataset.h = h.id; if (c) x.dataset.turn = c.id;\n        clampBrief(b, h.brief);',
    '        const [x, b] = hop("child k-" + h.kind + " s-" + h.status + (c || h.kind === "toyou" || h.kind === "move" ? "" : " stub"), ic, hcls(tgt), parts, h.at); x.dataset.h = h.id; if (c) x.dataset.turn = c.id;\n        if (result) { n++; continue; }\n        clampBrief(b, h.brief);'],
];
const RESULT_CSS = [
  '.result-marker { display: flex; align-items: center; gap: 7px; min-height: 24px; color: var(--muted); font-size: 12.5px; }',
  '.result-marker > svg { width: 15px; height: 15px; flex: none; color: var(--faint); }',
  '.result-marker .tm { margin-left: auto; font-family: var(--mono); font-size: 11.5px; color: var(--faint); }',
  '.result-marker.new { color: var(--accent); }',
].join('\n');
// The sample uses illustrative hostnames; in port mode the served model owns the machine names and the viewer
// shortens those names in sidebar and line-2 labels.
const HOST_LINES = [
  ['  const HOST = { laptop: "marvin-mbp.local", studio: "studio.onsager.dev", buildbox: "buildbox.onsager.ai", cloud: "claude.ai-cloud.onsager.dev" };',
    (D) => "  const HOST = " + J(D.MACHINE) + ";"],
];
// The served viewer's other deliberate differences on the Machines screens (several machines from the server): an
// offline machine's last-seen time where the sample had a move, and the embedding server's management link.
const MACHINE_LINES = [
  ['        : ["Not responding" + (mh ? " since " + clock(mh.at) : ""), mv ? mv + (mv === 1 ? " session" : " sessions") + " moved off" : null].filter(Boolean).join(" · ")));\n      list.append(r);\n    }\n    page.append(list);',
    '        : ["Not responding" + (mh ? " since " + clock(mh.at) : MACHINE_LAST[m] != null ? " since " + clock(MACHINE_LAST[m]) : ""), mv ? mv + (mv === 1 ? " session" : " sessions") + " moved off" : null].filter(Boolean).join(" · ")));\n      list.append(r);\n    }\n    if (ADMIN) { const a = el("button", "more", ADMIN.label); a.type = "button"; a.addEventListener("click", () => location.assign(ADMIN.href)); list.append(a); }\n    page.append(list);'],
  ['l2.append(el("span", "rest", up ? w + " working · " + here.length + (here.length === 1 ? " session" : " sessions") : "Semon moved its sessions to other machines")); };',
    'l2.append(el("span", "rest", up ? w + " working · " + here.length + (here.length === 1 ? " session" : " sessions") : movedOff(m).length ? "Semon moved its sessions to other machines" : [MACHINE_LAST[m] != null ? "Last seen " + clock(MACHINE_LAST[m]) : null, here.length + (here.length === 1 ? " session" : " sessions")].filter(Boolean).join(" · "))); };'],
];

// The mockup file with the served data in its data block. The overhaul mockup's is built in overhaul-port.mjs.
function portReference(D, ref = "sample") {
  if (ref === "overhaul") return overhaulPortReference(D, OVERHAUL);
  const start = MOCKUP.indexOf(DATA_START), end = MOCKUP.indexOf(DATA_END);
  if (start < 0 || end < start) throw new Error("mockup data block markers moved");
  const hhmm = (t) => new Date(t).toISOString().slice(11, 16);
  const asIso = (t) => new Date(t).toISOString();
  const TX = {};
  const H = D.H.map((h) => ({ ...h, portStatus: h.status }));
  for (const [sid, es] of Object.entries(D.TX)) TX[sid] = es.map((e) => (e.ret ? { k: "end", text: "Returned to " + D.SESS[e.ret.to].name + (e.ret.failed ? " · failed" : "") + " · " + hhmm(e.ret.at) } : e));
  const SESS = Object.fromEntries(Object.entries(D.SESS).map(([id, source]) => {
    const s = { ...source, id, modelId: Object.keys(source.tokens_by_model ?? {})[0] ?? source.model };
    s.tokensByModel = Object.fromEntries(Object.entries(source.tokens_by_model ?? {}).map(([model, usage]) => [model, { input: usage.input, output: usage.output, cacheWrite: usage.cache_write, cacheRead: usage.cache_read }]));
    if (source.rate_limits) {
      const windows = source.rate_limits.windows ?? [], byMinutes = (minutes) => windows.find((w) => w.minutes === minutes);
      const shape = (window) => window ? { used_percent: window.used_percent, resets_at: asIso(window.resets_at) } : undefined;
      s.rate_limits = { recorded_at: asIso(source.rate_limits.recorded_at), five_hour: shape(byMinutes(300)), weekly: shape(byMinutes(10080)) };
    }
    return [id, s];
  }));
  const block = ["  const NOW = " + D.NOW + ";", "  const MACHINE = " + J(D.MACHINE) + ";", "  const MACHINE_UP = " + J(D.MACHINE_UP) + ";",
    "  const MACHINE_LAST = " + J(D.MACHINE_LAST ?? {}) + ";", "  const ADMIN = " + J(D.ADMIN ?? null) + ";",
    '  const HARNESS = { claude: "Claude Code", codex: "Codex" };', "  const SESS = " + J(SESS) + ";",
    "  const API_PRICE = {};", "  const H = " + J(H) + ";", "  const SEEN_RESULTS = new Set();", "  const markSeenResults = (handoffs) => { for (const h of handoffs) if (h.kind === \"toyou\" && h.ask === \"result\") SEEN_RESULTS.add(h.id); };", "  const THREADS = {};", "  const TX = " + J(TX) + ";", "  const TXM = " + J(D.TXM ?? {}) + ";", ""].join("\n");
  let html = MOCKUP.slice(0, start) + block.replace(/<\/script/gi, "<\\/script") + MOCKUP.slice(end);
  const styleEnd = html.lastIndexOf("</style>");
  if (styleEnd < 0) throw new Error("mockup style block moved");
  html = html.slice(0, styleEnd) + RESULT_CSS + "\n" + html.slice(styleEnd);
  for (const [a, b] of [...CLOCKS, ...MACHINE_LINES, ...ANALYTICS_LINES, ...CALL_LINES, ...THOUGHT_LINES, ...COST_LINES, ...RESULT_LINES, ...TURN_RESULT_LINES, ...TRACE_RESULT_LINES]) { if (html.split(a).length !== 2) throw new Error("mockup line moved: " + a.slice(0, 40)); html = html.replace(a, b); }
  for (const [a, replacement] of HOST_LINES) { if (html.split(a).length !== 2) throw new Error("mockup line moved: " + a.slice(0, 40)); html = html.replace(a, replacement(D)); }
  const histories = /  const ANALYTICS_HISTORY = \{[\s\S]*?\n  \};\n  const ANALYTICS_WAIT_SAMPLES = \[[\s\S]*?\n  \];/;
  if (!histories.test(html)) throw new Error("mockup analytics history block moved");
  html = html.replace(histories, "  const ANALYTICS_HISTORY = {};\n  const ANALYTICS_WAIT_SAMPLES = [];");
  return html;
}

// The fonts, as the served viewer declares them, for the reference's Google Fonts request.
const FACES = fs.readFileSync(path.join(here, "../../crates/semon-sessions/src/viewer.css"), "utf8").match(/@font-face \{[^}]*\}/g).join("\n");
async function referencePage(browser, size, dark, html) {
  const ctx = await context(browser, { size, dark });
  const page = await ctx.newPage();
  page.errors = [];
  page.on("pageerror", (e) => page.errors.push(e.message.split("\n")[0]));
  await page.clock.setFixedTime(ENV.now);
  await page.addInitScript(() => { window.__SEMON_PIXEL_COMPARE = true; });
  await page.route(/.*/, async (r) => {
    const url = new URL(r.request().url());
    if (url.href === "http://reference.test/") return r.fulfill({ contentType: "text/html; charset=utf-8", body: stableMockup(html) });
    const harness = url.host === "reference.test" ? /^\/harness\/([\w.-]+\.svg)$/.exec(url.pathname) : null; // the harness marks the viewer serves at /harness/*.svg
    if (harness && fs.existsSync(path.join(here, "../../assets/harnesses", harness[1]))) return r.fulfill({ contentType: "image/svg+xml", body: fs.readFileSync(path.join(here, "../../assets/harnesses", harness[1])) });
    if (url.href === "http://reference.test/mark.svg") return r.fulfill({ contentType: "image/svg+xml", body: MARK_SVG });
    if (url.host === "fonts.googleapis.com") return r.fulfill({ contentType: "text/css", body: FACES.replaceAll('url("/fonts/', 'url("http://reference.test/fonts/') });
    const font = /^\/fonts\/(instrument-sans|jetbrains-mono|source-serif-4)-(latin-ext|latin)\.woff2$/.exec(url.pathname);
    if (url.host === "reference.test" && font) return r.fulfill({ contentType: "font/woff2", body: fs.readFileSync(path.join(FONTS, font[1], font[2] + ".woff2")) });
    return r.abort();
  });
  await page.goto("http://reference.test/", { waitUntil: "load" });
  return page;
}

const FACE_LOADS = ['400 14px "Instrument Sans"', '500 14px "Instrument Sans"', '600 14px "Instrument Sans"', '400 12px "JetBrains Mono"', '500 12px "JetBrains Mono"', '400 14px "Source Serif 4"', '600 14px "Source Serif 4"'];
async function ready(page) {
  await page.evaluate((faces) => Promise.all(faces.map((f) => document.fonts.load(f))).then(() => document.fonts.ready), FACE_LOADS);
  await page.evaluate(() => {
    const state = history.state, sessionAtEnd = state?.v === "session" && !state.turn, main = document.querySelector("#main");
    if (!sessionAtEnd) { window.scrollTo(0, 0); if (main) main.scrollTop = 0; }
    else if (matchMedia("(max-width: 760px)").matches) window.scrollTo(0, document.documentElement.scrollHeight);
    else if (main) main.scrollTop = main.scrollHeight;
  });
  await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
  await page.waitForTimeout(120);
}
async function shot(page, size) {
  await ready(page);
  return PNG.sync.read(await page.screenshot({ fullPage: size === "phone", animations: "disabled", caret: "hide" }));
}

// Differing pixels over the larger of the two images; the area one image lacks counts as differing.
function compare(a, b) {
  const w = Math.max(a.width, b.width), h = Math.max(a.height, b.height);
  const pad = (img) => { if (img.width === w && img.height === h) return img; const p = new PNG({ width: w, height: h }); p.data.fill(255); PNG.bitblt(img, p, 0, 0, img.width, img.height, 0, 0); return p; };
  const A = pad(a), B = pad(b), diff = new PNG({ width: w, height: h });
  const n = pixelmatch(A.data, B.data, diff.data, w, h, { threshold: THRESHOLD, includeAA: false });
  return { pixels: n, ratio: n / (w * h), size: a.width === b.width && a.height === b.height ? null : [a.width + "×" + a.height, b.width + "×" + b.height], diff, A, B };
}

// Every screen, as routes on the served viewer and on each reference.
function screens(D, S, ids) {
  const list = [["home", { v: "home" }], ["analytics", { v: "analytics" }], ["sessions", { v: "sessions" }], ["machines", { v: "machines" }]];
  for (const m of Object.keys(D.MACHINE)) list.push(["machine-" + m, { v: "machine", id: m }]);
  // A session the sample mockup lacks (the fixture has grown past it) has no sample screen to compare with.
  for (const sid of Object.keys(D.SESS)) list.push(["session-" + sid, { v: "session", id: sid }, undefined, S.SESS[sid] ? undefined : null]);
  for (const t of D.turns) {
    if (!t.sent.length) continue;
    const at = D.TX[t.sid].findIndex((e) => e.turn === t.id);
    // The mockup names a turn by its start handoff, else "<session>:<entry index>".
    const port = t.start ?? t.sid + ":" + at;
    const inSample = !S.SESS[t.sid] ? null : t.start ? ids.get(t.start) : S.TX[t.sid]?.[at]?.k === "u" ? t.sid + ":" + at : null;
    list.push(["trace-" + t.sid + "-" + (inSample ?? t.id).replace(/[^\w-]/g, "_"), { v: "trace", sid: t.sid, turn: t.id }, { v: "trace", sid: t.sid, turn: port }, inSample ? { v: "trace", sid: t.sid, turn: inSample } : null]);
  }
  return list.map(([name, served, port, sample]) => ({ name, served, port: port ?? served, sample: sample === undefined ? served : sample }))
    .filter((s) => !ONLY.length || ONLY.some((prefix) => s.name.startsWith(prefix)));
}

// The sample's handoff for each served one, matched as gaps.mjs matches them.
function sampleIds(D, S) {
  const min = (t) => Math.round(((t - BASE) / 60000) * 1000) / 1000, ids = new Map();
  for (const h of S.H) { const v = D.H.find((x) => !ids.has(x.id) && x.kind === h.kind && x.from === h.from && x.to === h.to && min(x.at) === h.at); if (v) ids.set(v.id, h.id); }
  return ids;
}

async function nav(page, route, D, mockup) {
  if (!mockup) return goto(page, route, D);
  await page.evaluate((r) => { history.pushState(r, ""); dispatchEvent(new PopStateEvent("popstate", { state: r })); }, route);
  await page.waitForTimeout(80);
}

const save = (dir, name, img) => { fs.mkdirSync(dir, { recursive: true }); fs.writeFileSync(path.join(dir, name + ".png"), PNG.sync.write(img)); };
// The ratchet: a pending screen may not drift away from the overhaul. tests/ui/pixel-baseline.json holds, per screen and scheme, its
// ratio of differing pixels and the size of both pictures (served, reference). A screen fails when its ratio rises by more than half
// a tenth of a point (the run-to-run drift seen so far is 0.013), or when either size changes. A fall is reported ("fell", in the
// table) but does not fail, so improving a screen never makes the baseline a conflict for the next PR; a PR that ports a screen
// refreshes the baselines it touches in its own diff (out/pixel-baseline.<section>.json, written by every run, holds the current
// values), and its body says which and why. SEMON_PIXEL_BASELINE names the section
// ("main" by default; the two-machines step uses "two-machines").
const SECTION = process.env.SEMON_PIXEL_BASELINE ?? "main";
const BASELINE_FILE = path.join(here, "pixel-baseline.json");
const BASELINE = fs.existsSync(BASELINE_FILE) ? JSON.parse(fs.readFileSync(BASELINE_FILE, "utf8"))[SECTION] : undefined;
const RISE = 0.001;
const CURRENT = {};
function ratchet(row, p) {
  const key = row.scheme + "|" + row.screen, d = p.A.width + "×" + p.A.height, size = p.size ?? [d, d];
  CURRENT[key] = { ratio: Math.round(p.ratio * 1e5) / 1e5, size };
  const was = BASELINE?.[key];
  if (!BASELINE) return "no baseline section " + SECTION + " in pixel-baseline.json";
  if (!was) return "no baseline for " + key;
  if (p.ratio > was.ratio + RISE) return "rose from " + (was.ratio * 100).toFixed(2) + "% to " + (p.ratio * 100).toFixed(2) + "%";
  if (was.size[0] !== size[0] || was.size[1] !== size[1]) return "size changed from " + was.size.join(" vs ") + " to " + size.join(" vs ");
  if (p.ratio < was.ratio - RISE) return "~fell from " + (was.ratio * 100).toFixed(2) + "% to " + (p.ratio * 100).toFixed(2) + "%"; // reported, never a failure
  return null;
}
// Served against served: the same viewer built over the same data, compared with itself. SEMON_REF_BASE/SEMON_REF_TOKEN name the
// second server (the pushed-data step compares the received copy with the machine's own). No mockup and no ratchet is involved.
const REF_BASE = (process.env.SEMON_REF_BASE ?? "").replace(/\/$/, ""), REF_TOKEN = process.env.SEMON_REF_TOKEN ?? "";
// Regions: parts of a screen that are already ported (the top bar, an open menu), compared with the overhaul and enforced, while the
// rest of the screen is still pending. reference-map.json lists each region's selector, the screens it applies to (a name ending in
// "-" covers every screen with that prefix) and, for a panel, the button that opens it. A region is compared after the screen's
// full-page comparison, on the same two pages.
const REGIONS = MAP.regions ?? [];
const regionsOf = (name) => REGIONS.filter((r) => r.screens.some((x) => (x.endsWith("-") ? name.startsWith(x) : name === x)));
async function regionShot(page, region) {
  // A region that does not depend on how far the page is scrolled (the bar, whose border shows once it is) is taken at the top, so a
  // pending page that happens to be a few pixels taller on one side cannot change it.
  if (region.top) { await page.evaluate(() => { window.scrollTo(0, 0); const main = document.querySelector("#main"); if (main) main.scrollTop = 0; }); await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)))); await page.waitForTimeout(120); }
  if (region.open) { await page.click(region.open); await page.waitForTimeout(200); }
  const target = page.locator(region.selector).first();
  await target.waitFor({ state: "visible", timeout: 3000 });
  const png = PNG.sync.read(await target.screenshot({ animations: "disabled", caret: "hide" }));
  if (region.open) { await page.keyboard.press("Escape"); await page.waitForTimeout(150); }
  return png;
}
// Both pictures and the diff of an enforced mismatch. A pending screen differs by design until its port PR, so only the diff is kept,
// with the pictures for the screens that show the most (Home, the lists, one session, one trace).
const SHOWN = ["home", "sessions", "machines", "analytics", "session-harbor", "session-h-codex", "drawer"];
function saveMismatch(row, name, scheme, p) {
  if (row.port.pass && (row.port.enforced || !p.pixels)) return;
  const dir = path.join(OUT, row.port.enforced ? "port" : "pending", scheme);
  save(dir, name + "-diff", p.diff);
  if (row.port.enforced || SHOWN.includes(name)) { save(dir, name + "-served", p.A); save(dir, name + "-reference", p.B); }
}

(async () => {
  const D = await data(), S = sample("semon-sample.html"), ids = sampleIds(D, S), list = screens(D, S, ids);
  const unmapped = list.filter((s) => referenceOf(s.name) === undefined).map((s) => s.name);
  if (unmapped.length) throw new Error("reference-map.json has no entry for: " + unmapped.join(", "));
  // The phone's navigation drawer isn't in the screen list, so its entry is checked by name.
  if (referenceOf("drawer") === undefined) throw new Error("reference-map.json has no entry for: drawer");
  const skipped = Object.entries(MAP.screens).filter(([, ref]) => ref === null).map(([name]) => name);
  const browser = await launch();
  const results = [], errors = [], regions = [];
  for (const [size, dark] of SCHEMES) {
    const scheme = size + "-" + (dark ? "dark" : "light");
    const page = await served(browser, { size, dark });
    // One port reference per mockup a screen is mapped to, opened on first use.
    const ports = new Map();
    const portFor = async (ref) => { if (!ports.has(ref)) ports.set(ref, await referencePage(browser, size, dark, portReference(D, ref))); return ports.get(ref); };
    const other = REF_BASE ? await served(browser, { size, dark, base: REF_BASE, token: REF_TOKEN }) : null;
    const orig = other ? null : await referencePage(browser, size, dark, MOCKUP);
    for (const s of list.filter((x) => other || referenceOf(x.name) !== null)) {
      const mapped = other ? "served" : referenceOf(s.name), enforced = mapped !== "pending", ref = other ? "served" : enforced ? mapped : "overhaul";
      const port = other ?? await portFor(ref);
      await nav(page, s.served, D, false); const a = await shot(page, size);
      if (other) await nav(port, s.served, D, false); else await nav(port, s.port, D, true);
      const b = await shot(port, size);
      const p = compare(a, b);
      const row = { scheme, screen: s.name, reference: ref, port: { pixels: p.pixels, ratio: p.ratio, size: p.size, enforced, pass: !enforced || (p.pixels <= MAX_RATIO * p.diff.width * p.diff.height && !p.size) } };
      if (!enforced) { const problem = ratchet(row, p); if (problem) { row.port.ratchet = problem; if (!problem.startsWith("~")) row.port.pass = false; } }
      saveMismatch(row, s.name, scheme, p);
      for (const region of regionsOf(s.name)) {
        const name = s.name + "#" + region.name, rrow = { scheme, screen: name, reference: "overhaul", port: { enforced: true, pass: false } };
        const except = region.except?.[scheme + "|" + s.name];
        if (except) {
          // A named, reasoned exception, held only while its cause holds: the served page scrolls and the mockup's does not (a sticky bar is drawn in its
          // own layer only when the page scrolls). Once that is no longer so, the exception fails until it is removed from reference-map.json.
          const scrolls = (pg) => pg.evaluate(() => { const el = matchMedia("(max-width: 760px)").matches ? document.scrollingElement : document.querySelector("#main"); return el.scrollHeight > el.clientHeight + 1; });
          const [servedScrolls, portScrolls] = [await scrolls(page), await scrolls(port)];
          if (servedScrolls && !portScrolls) rrow.port = { pixels: 0, ratio: 0, enforced: true, pass: true, skipped: except };
          else { rrow.error = "the exception's cause is gone (served scrolls: " + servedScrolls + ", the mockup's: " + portScrolls + "): remove " + scheme + "|" + s.name + " from reference-map.json"; }
          regions.push(rrow); continue;
        }
        try {
          const q = compare(await regionShot(page, region), await regionShot(port, region));
          rrow.port = { pixels: q.pixels, ratio: q.ratio, size: q.size, enforced: true, pass: q.pixels <= MAX_RATIO * q.diff.width * q.diff.height && !q.size };
          saveMismatch(rrow, name.replace("#", "-"), scheme, q);
        } catch (e) { rrow.error = String(e.message ?? e).split("\n")[0]; }
        regions.push(rrow);
      }
      if (s.sample && !other) {
        await nav(orig, s.sample, D, true); const c = await shot(orig, size);
        const q = compare(a, c);
        row.sample = { pixels: q.pixels, ratio: q.ratio, size: q.size };
        if (q.pixels) { const dir = path.join(OUT, "sample", scheme); save(dir, s.name + "-served", q.A); save(dir, s.name + "-sample", q.B); save(dir, s.name + "-diff", q.diff); }
      }
      results.push(row);
    }
    // The phone's navigation drawer, open on Home.
    if (size === "phone" && !ONLY.length && (other || referenceOf("drawer") !== null)) {
      const port = other ?? await portFor(referenceOf("drawer") === "pending" ? "overhaul" : referenceOf("drawer"));
      // Each session screenshot can mark result handoffs read. Start the drawer comparison in a fresh served context,
      // matching the reference page reload below, so both drawers show the fixture's initial unread count.
      const drawerPage = await served(browser, { size, dark });
      await drawerPage.click("#lead-btn"); await drawerPage.waitForTimeout(350);
      let side;
      if (other) { side = await served(browser, { size, dark, base: REF_BASE, token: REF_TOKEN }); await side.click("#lead-btn"); await side.waitForTimeout(350); }
      else { await port.goto("http://reference.test/", { waitUntil: "load" }); await port.waitForSelector("#lead-btn"); await port.click("#lead-btn"); await port.waitForTimeout(350); side = port; }
      const p = compare(await shot(drawerPage, "desktop"), await shot(side, "desktop"));
      const enforced = other ? true : referenceOf("drawer") !== "pending";
      const row = { scheme, screen: "drawer", reference: other ? "served" : enforced ? referenceOf("drawer") : "overhaul", port: { pixels: p.pixels, ratio: p.ratio, size: p.size, enforced, pass: !enforced || (p.pixels <= MAX_RATIO * p.diff.width * p.diff.height && !p.size) } };
      if (!enforced) { const problem = ratchet(row, p); if (problem) { row.port.ratchet = problem; if (!problem.startsWith("~")) row.port.pass = false; } }
      saveMismatch(row, "drawer", scheme, p);
      results.push(row);
      errors.push(...drawerPage.errors.map((e) => scheme + " served drawer: " + e));
      await drawerPage.context().close(); if (other) await side.context().close();
    }
    errors.push(...page.errors.map((e) => scheme + " served: " + e), ...[...ports].flatMap(([ref, p]) => p.errors.map((e) => scheme + " " + ref + " port reference: " + e)), ...(orig?.errors ?? []).map((e) => scheme + " sample: " + e), ...(other?.errors ?? []).map((e) => scheme + " second server: " + e));
    await page.context().close(); await orig?.context().close(); await other?.context().close();
    for (const p of ports.values()) await p.context().close();
  }
  await browser.close();
  const failed = [...results, ...regions].filter((r) => !r.port.pass), pending = results.filter((r) => !r.port.enforced);
  // The current baselines, for a PR that moves them: written every run, never read back.
  fs.writeFileSync(path.join(ENV.out, "pixel-baseline." + SECTION + ".json"), J({ [SECTION]: Object.fromEntries(Object.entries(CURRENT).sort(([a], [b]) => (a < b ? -1 : 1))) }, null, 1) + "\n");
  const pct = (x) => (x * 100).toFixed(3) + "%";
  const md = ["| Screen | Scheme | vs port reference | vs sample mockup |", "|---|---|---|---|",
    ...results.map((r) => "| " + r.screen + " | " + r.scheme + " | " + (r.port.enforced ? (r.port.pass ? "✓ " : "✗ ") : r.port.ratchet ? (r.port.ratchet.startsWith("~") ? "pending (" + r.port.ratchet.slice(1) + ") · " : "✗ ratchet (" + r.port.ratchet + ") · ") : "pending · ") + r.port.pixels + " px (" + pct(r.port.ratio) + ")" + (r.port.size ? " size " + r.port.size.join(" vs ") : "") + " | " + (r.sample ? r.sample.pixels + " px (" + pct(r.sample.ratio) + ")" + (r.sample.size ? " size " + r.sample.size.join(" vs ") : "") : "n/a") + " |")].join("\n");
  const regionMd = regions.length ? "\n\n| Region | Scheme | vs overhaul |\n|---|---|---|\n" + regions.map((r) => "| " + r.screen + " | " + r.scheme + " | " + (r.error ? "✗ " + r.error : r.port.skipped ? "skipped: " + r.port.skipped : (r.port.pass ? "✓ " : "✗ ") + r.port.pixels + " px (" + pct(r.port.ratio) + ")" + (r.port.size ? " size " + r.port.size.join(" vs ") : "")) + " |").join("\n") : "";
  fs.writeFileSync(path.join(ENV.out, "pixels.json"), J({ threshold: THRESHOLD, maxRatio: MAX_RATIO, results, regions, errors }, null, 1));
  const note = skipped.length ? "\nNot compared (unmapped in reference-map.json, not drawn by the viewer yet): " + skipped.join(", ") + "\n" : "";
  fs.writeFileSync(path.join(ENV.out, "pixels.md"), md + regionMd + "\n" + note);
  console.log(md + regionMd);
  console.log("regions: " + regions.length + ", failing: " + regions.filter((r) => !r.port.pass).length);
  console.log("screens: " + results.length + ", port mismatches: " + failed.length + ", page errors: " + errors.length + (pending.length ? ", pending (reported, not enforced): " + pending.length : ""));
  if (skipped.length) console.log("unmapped, not compared: " + skipped.join(", "));
  for (const e of errors) console.log("  page error: " + e);
  if (process.env.GITHUB_STEP_SUMMARY) fs.appendFileSync(process.env.GITHUB_STEP_SUMMARY, "## Pixel comparison\n\n" + md + regionMd + "\n" + note);
  process.exit(failed.length || errors.length ? 1 : 0);
})().catch((e) => { console.error(e); process.exit(2); });
