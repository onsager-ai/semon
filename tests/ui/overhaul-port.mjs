// The overhaul mockup (reference/overhaul.html) run on the served data: the "port reference" of the pixel comparison.
//
// The mockup keeps its sample data in one block and reads it in the mockup's own shapes: times are minutes since the day's
// midnight (fractions allowed), token totals are camel-cased, results are marked `wait` or `done`. The served model has epoch
// milliseconds, snake-cased tokens and a `cost` object per session. So the port is built in two steps:
//
//   1. The served data is converted into the mockup's shapes (times become minutes since the fixture's midnight, to the
//      microsecond) and put in the data block. Nothing in the mockup's code changes for these.
//   2. The few places where the mockup computes a figure the server owns (the clock formatters, thought durations, cost) are
//      swapped for the served viewer's rule. Each swap names the line or function it replaces and fails if that has moved, so a
//      change to the mockup is noticed rather than silently compared against.
//
// A port PR that flips a screen to the overhaul mockup adds the swaps that screen needs, here, and says so in its body.
import { BASE } from "./fixture.mjs";

const J = JSON.stringify;
const DATA_START = "  const T = (h, m) => h * 60 + m; const ST = (h, m, s = 0) => (T(h, m) * 60 + s) * 1000; const NOW = T(12, 40);";
const DATA_END = "  // ====================================================================================\n  const $ = ";

// Replace the one line that starts with `prefix`.
function swapLine(html, prefix, next) {
  const lines = html.split("\n"), at = lines.flatMap((l, i) => (l.startsWith(prefix) ? [i] : []));
  if (at.length !== 1) throw new Error("overhaul mockup line moved (" + at.length + " matches): " + prefix.slice(0, 60));
  lines[at[0]] = next;
  return lines.join("\n");
}
// Replace `  function name(...) { ... }`, whose closing brace is the first "\n  }\n" after it.
function swapFunction(html, name, next) {
  const head = "  function " + name + "(", i = html.indexOf(head);
  if (i < 0 || html.indexOf(head, i + 1) >= 0) throw new Error("overhaul mockup function moved: " + name);
  const end = html.indexOf("\n  }\n", i);
  if (end < 0) throw new Error("overhaul mockup function has no closing brace: " + name);
  return html.slice(0, i) + next + html.slice(end + "\n  }".length);
}

// The served data in the mockup's shapes.
function convert(D) {
  const minute = (t) => (t == null ? t : Math.round(((t - BASE) / 60000) * 1e6) / 1e6);
  const iso = (t) => new Date(t).toISOString();
  const hhmm = (t) => iso(t).slice(11, 16);
  const SESS = Object.fromEntries(Object.entries(D.SESS).map(([id, source]) => {
    const s = { ...source, id, modelId: source.model ?? Object.keys(source.tokens_by_model ?? {})[0] };
    s.start = minute(source.start); s.last = minute(source.last);
    s.busy = (source.busy ?? []).map(([a, b]) => [minute(a), minute(b)]);
    s.tokensByModel = Object.fromEntries(Object.entries(source.tokens_by_model ?? {}).map(([model, usage]) => [model, { input: usage.input, output: usage.output, cacheWrite: usage.cache_write, cacheRead: usage.cache_read }]));
    if (source.rate_limits) {
      const windows = source.rate_limits.windows ?? [], byMinutes = (minutes) => windows.find((w) => w.minutes === minutes);
      const shape = (window) => (window ? { used_percent: window.used_percent, resets_at: iso(window.resets_at) } : undefined);
      s.rate_limits = { recorded_at: iso(source.rate_limits.recorded_at), five_hour: shape(byMinutes(300)), weekly: shape(byMinutes(10080)) };
    }
    return [id, s];
  }));
  const H = D.H.map((h) => ({ ...h, at: minute(h.at), done: minute(h.done) }));
  const thought = (e) => String(e.text ?? e.summary ?? "").trim();
  const TX = {};
  for (const [sid, es] of Object.entries(D.TX)) {
    TX[sid] = es.map((e, i) => {
      if (e.ret) return { k: "end", text: "Returned to " + D.SESS[e.ret.to].name + (e.ret.failed ? " · failed" : "") + " · " + hhmm(e.ret.at) };
      // The last, empty thought of a working session is one still being written; the mockup marks that `pending`.
      if ((e.k === "think" || e.k === "reasoning") && !thought(e) && i === es.length - 1 && D.SESS[sid]?.state === "work") return { ...e, pending: true };
      return e;
    });
  }
  return { SESS, H, TX, NOW: minute(D.NOW) };
}

// What the mockup computes from tokens and a price table, the server owns: the exact cost, by model and by kind. The mockup
// has four kinds; the served five-minute and one-hour cache writes are shown together as one.
const COST_FUNCTION = `  function costForSessions(sessions) {
    const kinds = Object.fromEntries(TOKEN_KINDS.map(([key]) => [key, { tokens: 0, usd: 0 }])), models = new Map(), unknown = new Set(); let usd = 0, allPriced = true;
    const KEY = { input: "input", output: "output", cache_read: "cacheRead", cache_write_5m: "cacheWrite", cache_write_1h: "cacheWrite", web_search: "webSearch" };
    for (const s of sessions) {
      const cost = s.cost ?? {};
      if (cost.usd == null) allPriced = false; else usd += Number(cost.usd) || 0;
      for (const modelId of cost.unpriced_models ?? []) unknown.add(modelId);
      for (const [modelId, model] of Object.entries(cost.by_model ?? {})) {
        const current = models.get(modelId) ?? { modelId, kinds: Object.fromEntries(TOKEN_KINDS.map(([key]) => [key, { tokens: 0, usd: 0 }])), usd: 0, priced: true };
        if (model.usd == null) current.priced = false; else current.usd += Number(model.usd) || 0;
        for (const [served, key] of Object.entries(KEY)) {
          const tokens = Number(model.tokens?.[served]) || 0, amount = Number(model.usd_by_kind?.[served]) || 0;
          current.kinds[key].tokens += tokens; current.kinds[key].usd += amount; kinds[key].tokens += tokens; kinds[key].usd += amount;
        }
        models.set(modelId, current);
      }
    }
    return { usd: allPriced && !unknown.size ? usd : null, knownUsd: usd, kinds, models: [...models.values()], unknown: [...unknown].sort() };
  }`;

// The served viewer's clock formatters, on minutes since the fixture's midnight: a time on another day carries its weekday, an
// age counts hours to two days and then days.
const CLOCKS = [
  ["  const clock = (m) =>", '  const clock = (m) => { const d = new Date(' + BASE + ' + m * 60000), n = new Date(' + BASE + ' + NOW * 60000); const hm = String(d.getHours()).padStart(2, "0") + ":" + String(d.getMinutes()).padStart(2, "0"); return d.toDateString() === n.toDateString() ? hm : d.toLocaleDateString(undefined, { weekday: "short" }) + " " + hm; };'],
  ["  const ago = (m) =>", '  const ago = (m) => { const d = Math.floor(NOW - m); return d < 1 ? "now" : d < 60 ? d + "m" : d < 2880 ? Math.floor(d / 60) + "h" : Math.floor(d / 1440) + "d"; };'],
  ["  const dur = (a, b) =>", '  const dur = (a, b) => { const d = Math.max(0, Math.floor((b ?? NOW) - a)); return d >= 1440 ? Math.floor(d / 1440) + "d " + Math.floor((d % 1440) / 60) + "h" : d >= 60 ? Math.floor(d / 60) + "h " + (d % 60) + "m" : d + "m"; };'],
];

export function overhaulPortReference(D, html) {
  const start = html.indexOf(DATA_START), end = html.indexOf(DATA_END);
  if (start < 0 || end < start) throw new Error("overhaul mockup data block markers moved");
  const { SESS, H, TX, NOW } = convert(D);
  const block = ["  const NOW = " + NOW + ";", "  const MACHINE = " + J(D.MACHINE) + ";", "  const MACHINE_UP = " + J(D.MACHINE_UP) + ";",
    "  const MACHINE_LAST = " + J(D.MACHINE_LAST ?? {}) + ";", "  const ADMIN = " + J(D.ADMIN ?? null) + ";",
    '  const HARNESS = { claude: "Claude Code", codex: "Codex" };', "  const SESS = " + J(SESS) + ";",
    "  const API_PRICE = {};", "  const H = " + J(H) + ";", "  const THREADS = {};", "  const TX = " + J(TX) + ";", ""].join("\n");
  let out = html.slice(0, start) + block.replace(/<\/script/gi, "<\\/script") + html.slice(end);
  for (const [prefix, next] of CLOCKS) out = swapLine(out, prefix, next);
  out = swapLine(out, "  const HOST = ", "  const HOST = " + J(D.MACHINE) + ";");
  // The session menu lists what the log gives: a directory and a process id only when there is one.
  out = swapLine(out, "  const dirOf = ", "  const dirOf = (s) => s.cwd ?? s.dir ?? s.directory ?? null;");
  out = swapLine(out, "  const pidOf = ", "  const pidOf = (s) => s.pid ?? null;");
  out = swapLine(out, "  const sessionIdOf = ", "  const sessionIdOf = (s) => s.sessionId ?? s.id;");
  out = swapLine(out, "  const costText = ", '  const costText = (cost) => cost.unknown.length || cost.usd == null ? "—" : asMoney(cost.usd);');
  out = swapFunction(out, "costForSessions", COST_FUNCTION);
  // The served model gives a thought's seconds itself.
  out = swapFunction(out, "thoughtSeconds", "  function thoughtSeconds(entries, i, sid) { const secs = entries[i]?.secs; return Number.isFinite(secs) && secs >= 0 ? secs : null; }");
  // The mockup sets inline code in a transcript at .86em of 13px (11.2px), under the 12px floor its own principle P3 sets (and
  // the tokens check enforces); the viewer floors it at the caption size, so the reference does too.
  out = swapLine(out, ".body code, .cc-brief code, .ev-text code, .ib code { font-size: .86em;", ".body code, .cc-brief code, .ev-text code, .ib code { font-size: max(.86em, var(--fs-cap)); background: color-mix(in srgb, var(--ink) 6%, transparent); padding: 1px 4px; border-radius: 4px; overflow-wrap: anywhere; }");
  // Until the sidebar's port (PR 4), the viewer's sidebar is 296px wide and the mockup's 272px. The regions compare parts of the main
  // column, so the reference takes the viewer's width; PR 4 removes this.
  const styleEnd = out.lastIndexOf("</style>");
  if (styleEnd < 0) throw new Error("overhaul mockup style block moved");
  out = out.slice(0, styleEnd) + "@media (min-width: 761px) { .app:not(.rail) { grid-template-columns: 296px minmax(0, 1fr); } }\n" + out.slice(styleEnd);
  // A working run's card names its current call. The mockup reads it from the run's transcript (the last call, finished or not); the
  // served model reports only a call still running (gaps.json: h-review-codex.activity), so the card shows that.
  out = swapLine(out, '    else if (c.state === "work") { const last = (TX[c.id] ?? []).filter', '    else if (c.state === "work" && c.activity) { const n = el("span", "cc-now"); n.append(el("span", "spin"), el("span", null, verbNow(c.activity[0])), el("code", null, c.activity[1])); b.append(n); }');
  // Each of the mockup's histories is inside the served sessions' busy intervals already.
  const histories = /  const ANALYTICS_HISTORY = \{[\s\S]*?\n  \};\n  const ANALYTICS_WAIT_SAMPLES = \[[\s\S]*?\n  \];/;
  if (!histories.test(out)) throw new Error("overhaul mockup analytics history block moved");
  out = out.replace(histories, "  const ANALYTICS_HISTORY = {};\n  const ANALYTICS_WAIT_SAMPLES = [];");
  return out;
}

// The mockup's live demo (a working session gains a step every six seconds) must stand still for a comparison.
export function stableOverhaul(html) {
  const guard = '    if (route.v !== "session" || loadingView || panelEl) return;';
  if (html.split(guard).length !== 2) throw new Error("overhaul mockup demo tick guard moved");
  return html.replace(guard, '    if (window.__SEMON_PIXEL_COMPARE || route.v !== "session" || loadingView || panelEl) return;');
}
