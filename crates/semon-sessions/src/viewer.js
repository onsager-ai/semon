import { parseAccount, createAccountChrome } from "../../../ui/src/account-adapter";
(() => {
  // ====================================================================================
  // Model: sessions are places; handoffs are how work moves between them (ask: you → session,
  // spawn: session → subagent / Codex run, relay: session → session, move: takeover on another
  // machine, toyou: session → you). A turn is one stretch of a session's transcript, from an
  // incoming ask, relay or brief to the next one. A trace follows a turn's outgoing handoffs to
  // the turns they started, recursively. Both are computed from what the logs record.
  // Harness, machine, project and role are facets, never a hierarchy. Real data from this machine's logs.
  // ====================================================================================
  // Data: /api/model fills these when the page loads (boot), and /api/tx fills TX a page at a time.
  let NOW = Date.now();
  const MACHINE = {};
  const MACHINE_UP = {};
  // An offline machine's last-seen time (epoch ms), and the embedding server's machine-management link, when served.
  const MACHINE_LAST = {};
  let ADMIN = null;
  let ACCOUNT = null;
  let NAV_MACHINES = null;
  // An embedding page that shows the viewer's sidebar beside its own content marks its .app data-viewer="sidebar" (docs/shell.md):
  // only the sidebar is drawn there, and every destination opens the viewer's own page.
  const SIDEBAR_ONLY = document.querySelector(".app")?.dataset.viewer === "sidebar";
  // The harnesses Semon can name. Mirrors crates/semon-sessions/src/harness.rs (a Rust test keeps them equal). Icons identify the source only; the artwork is served unmodified.
  const HARNESSES = { claude: { name: "Claude Code", short: "Claude", icon: { light: "/harness/claude-code.svg", dark: "/harness/claude-code.svg" } }, codex: { name: "Codex", short: "Codex", icon: { light: "/harness/codex-black.svg", dark: "/harness/codex.svg" } }, opencode: { name: "OpenCode", short: "OpenCode", icon: { light: "/harness/opencode-light.svg", dark: "/harness/opencode-dark.svg" } } };
  const HARNESS = Object.fromEntries(Object.entries(HARNESSES).map(([id, h]) => [id, h.name]));
  const HARNESS_SHORT = Object.fromEntries(Object.entries(HARNESSES).map(([id, h]) => [id, h.short]));
  const SESS = {};
  const H = [];
  const TX = {};
  const SEEN_KEY = "semon.seen", SEEN_LIMIT = 2000;
  const SEEN_RESULTS = (() => {
    try {
      const ids = JSON.parse(window.localStorage.getItem(SEEN_KEY) ?? "[]");
      if (!Array.isArray(ids)) return new Set();
      const clean = ids.filter((id) => typeof id === "string").slice(-SEEN_LIMIT), seen = new Set(clean);
      if (seen.size !== ids.length) try { window.localStorage.setItem(SEEN_KEY, JSON.stringify([...seen])); } catch {}
      return seen;
    } catch { return new Set(); }
  })();

  // ====================================================================================
  const $ = (s, r = document) => r.querySelector(s);
  // Instrument Sans sets the middle dot with little side bearing. Thin spaces keep separators readable without changing code.
  const spaced = (t) => String(t).replace(/ · /g, "\u2009 · \u2009").replace(/^· /, "·\u2009 ");
  const el = (tag, cls, text) => { const n = document.createElement(tag); if (cls) n.className = cls; if (text != null) n.textContent = tag === "code" || tag === "pre" ? text : spaced(text); return n; };
  const SVGNS = "http://www.w3.org/2000/svg";
  function icon(d, cls) { const s = document.createElementNS(SVGNS, "svg"); s.setAttribute("viewBox", "0 0 24 24"); s.setAttribute("aria-hidden", "true"); s.setAttribute("fill", "none"); s.setAttribute("stroke", "currentColor"); s.setAttribute("stroke-width", "1.8"); s.setAttribute("stroke-linecap", "round"); s.setAttribute("stroke-linejoin", "round"); if (cls) s.setAttribute("class", cls); const p = document.createElementNS(SVGNS, "path"); p.setAttribute("d", d); s.append(p); return s; }
  const I = {
    menu: "M4 7h16M4 12h16M4 17h16", more: "M5 12h.01M12 12h.01M19 12h.01", back: "M15 6l-6 6 6 6", chev: "M9 6l6 6-6 6", search: "M11 18a7 7 0 1 1 0-14 7 7 0 0 1 0 14zM20 20l-4-4", filter: "M4 6h16M7 12h10M10 18h4",
    home: "M4 11l8-7 8 7M6 9.5V20h12V9.5M10 20v-5h4v5", inbox: "M4 13l2.5-8h11L20 13v6H4zM4 13h5l1 2h4l1-2h5", now: "M3 12h4l2.5-6 5 12 2.5-6h4", trace: "M6 4v10a4 4 0 0 0 4 4h8M6 10h12M15 7l3 3-3 3M15 15l3 3-3 3", sessions: "M9 6h11M9 12h11M9 18h11M4.5 6h.01M4.5 12h.01M4.5 18h.01",
    run: "M4 17l5-5-5-5M12 19h8", stack: "M12 3l9 5-9 5-9-5zM3 13l9 5 9-5", read: "M6 3h8l4 4v14H6zM14 3v4h4", edit: "M4 20h4L19 9l-4-4L4 16zM13 7l4 4", find: "M11 18a7 7 0 1 1 0-14 7 7 0 0 1 0 14zM20 20l-4-4",
    machine: "M3 5h18v11H3zM8 20h8M12 16v4", repo: "M6 3v12M6 15a3 3 0 1 0 0 6 3 3 0 0 0 0-6zM18 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM18 9c0 6-12 3-12 6", duration: "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM12 7v5l3 2", delegate: "M4 3h7v5H4zM7.5 8v9H13M13 14h7v6h-7z",
    out: "M7 17L17 7M9 7h8v8", in: "M17 7L7 17M15 17H7V9", move: "M4 8h13l-3-3M20 16H7l3 3", ask: "M5 18l-1 3 3-1 11-11-2-2zM14 6l4 4", you: "M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM4 21c1-4 4-6 8-6s7 2 8 6", q: "M9 9a3 3 0 1 1 4 2.8c-.7.3-1 .9-1 1.7V14M12 18h.01", check: "M5 12l4 4 10-10", qc: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM9.6 9.4a2.5 2.5 0 1 1 3.4 2.4c-.6.3-1 .8-1 1.5v.4M12 17h.01", decide: "M12 21v-6M12 15L6 9M12 15l6-6M6 9V4M18 9V4M4 6l2-2 2 2M16 6l2-2 2 2", result: "M14 3H6v18h12V7zM14 3v4h4M9 12h6M9 16h6", done: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM8 12.5l2.7 2.7L16 9.8", x: "M6 6l12 12M18 6L6 18", expand: "M15 3h6v6M9 21H3v-6M21 3l-7 7M3 21l7-7", copy: "M9 9h11v11H9zM5 15H4V4h11v1", ext: "M14 4h6v6M20 4l-9 9M18 14v6H4V6h6",
    down: "M12 4v15M5 12l7 7 7-7", up: "M6 15l6-6 6 6", dn: "M6 9l6 6 6-6", branch: "M6 3v12M6 15a3 3 0 1 0 0 6 3 3 0 0 0 0-6zM18 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM18 9c0 6-12 3-12 6",
    wrench: "M14.5 6.5a5 5 0 0 0-6.9 6.9l-4.8 4.8a2 2 0 0 0 2.8 2.8l4.8-4.8a5 5 0 0 0 6.9-6.9l-3 3-2.8-2.8z", wide: "M4 9V4h5M15 4h5v5M20 15v5h-5M9 20H4v-5", sidebar: "M4 5h16v14H4zM9 5v14", tokens: "M5 5h14M12 5v14M9 19h6", chart: "M4 19V5M4 19h17M8 15l3-4 3 2 5-7", coin: "M12 3v18M17 7.5C17 6.1 14.8 5 12 5S7 6.1 7 7.5 9.2 10 12 10s5 1.1 5 2.5-2.2 2.5-5 2.5-5-1.1-5-2.5", relay: "M4 7h13l-3-3M20 17H7l3 3",
  };
  const clock = (t) => { const d = new Date(t), n = new Date(NOW); const hm = String(d.getHours()).padStart(2, "0") + ":" + String(d.getMinutes()).padStart(2, "0"); return d.toDateString() === n.toDateString() ? hm : d.toLocaleDateString(undefined, { weekday: "short" }) + " " + hm; };
  const ago = (t) => { const d = Math.floor((NOW - t) / 60000); return d < 1 ? "now" : d < 60 ? d + "m" : d < 2880 ? Math.floor(d / 60) + "h" : Math.floor(d / 1440) + "d"; };
  const dur = (a, b) => { const d = Math.max(0, Math.floor(((b ?? NOW) - a) / 60000)); return d >= 1440 ? Math.floor(d / 1440) + "d " + Math.floor((d % 1440) / 60) + "h" : d >= 60 ? Math.floor(d / 60) + "h " + (d % 60) + "m" : d + "m"; };
  const tok = (m) => m >= 1 ? m.toFixed(1) + "M" : Math.round(m * 1000) + "k";
  // A dot is the state's only sign where nothing beside it says the state, and then it carries a tooltip; `tip = false` where a word does.
  const dot = (st, tip = true) => { const d = el("span", "dot " + st); d.setAttribute("role", "img"); d.setAttribute("aria-label", STATE[st] ?? st); if (tip) d.dataset.tip = STATE[st] ?? st; return d; };
  const STATE = { work: "Working", wait: "Needs you", idle: "Idle", done: "Done", err: "Failed", new: "New result", read: "Read result" };
  const nameOf = (id) => id === "you" ? "You" : SESS[id].name;
  const hcls = (id) => id === "you" ? "h-you" : "h-" + SESS[id].harness;
  const where = (s) => s.repo ? s.repo + (s.branch && s.branch !== "main" && s.branch !== s.name ? " · " + s.branch : "") : "No repo";
  const hostOf = (s) => s.host ?? MACHINE[s.machine] ?? s.machine ?? "Unknown machine";
  // The name of a machine that has none is drawn whole ("Unknown machine"); a real one is cut to 14 characters.
  const shortName = (name) => { const h = String(name).split(".")[0]; return h.length > 14 ? h.slice(0, 14) + "…" : h; };
  const shortHost = (s) => s.host == null && MACHINE[s.machine] == null && s.machine == null ? hostOf(s) : shortName(hostOf(s));
  // Short display names for machines, `[[id, full name]]` in, a Map of id to name out. A long name is cut to 14 characters; two that
  // cut alike keep their tails ("build-…-east-1"), then their whole first label, then their id, until no two in the list are alike.
  const machineShorts = (names) => {
    const forms = (id, full) => { const first = String(full).split(".")[0]; return [shortName(first), first.length > 14 ? first.slice(0, 6) + "…" + first.slice(-7) : first, first, id]; };
    const opts = new Map(names.map(([id, full]) => [id, forms(id, full)])), level = new Map([...opts.keys()].map((id) => [id, 0]));
    for (let step = 0; step < 3; step++) {
      const groups = new Map(); for (const [id, o] of opts) { const l = o[level.get(id)]; if (!groups.has(l)) groups.set(l, []); groups.get(l).push(id); }
      let clash = false; for (const ids of groups.values()) if (ids.length > 1) { clash = true; for (const id of ids) level.set(id, level.get(id) + 1); }
      if (!clash) break;
    }
    return new Map([...opts].map(([id, o]) => [id, o[level.get(id)]]));
  };
  // Each machine the sessions in `scope` (sessions or their ids; every session by default) run on, by its short display name, only where
  // they span several machines. With one machine every line would say the same thing, so the Map is empty. One entry per machine id.
  const machineLabels = (scope = Object.values(SESS)) => {
    const names = new Map();
    for (const x of scope) { const s = typeof x === "string" ? SESS[x] : x; if (s?.machine != null && !names.has(s.machine)) names.set(s.machine, s.host ?? MACHINE[s.machine] ?? s.machine); }
    return names.size > 1 ? machineShorts([...names]) : new Map();
  };
  // A session's machine by its short name, or "" where the view is of one machine.
  const machineLabel = (s, scope) => (s ? machineLabels(scope).get(s.machine) ?? "" : "");
  const branchOf = (s) => s.worktree ?? s.branch ?? "No branch";
  const shortModel = (model) => String(model ?? "Unknown model").replace(/^gpt-(\d+\.\d+)-(.+)$/i, "$2 $1").replace(/^gpt-\d+-/i, "").replace(/^claude-/i, "").replace(/^(opus|sonnet|haiku)-(\d+)-(\d+)$/i, "$1 $2.$3").replace(/^(opus|sonnet|haiku)-(\d+)\.(\d+)$/i, "$1 $2.$3").replace(/^(opus|sonnet|haiku)-(\d+)$/i, "$1 $2");
  // A harness is named in plain muted text (.hname), never in a vendor colour; "short" gives "Claude" where the line is tight. The label itself
  // holds text only: its official mark (harnessIcon) is drawn beside it, outside the span, so the word still tells Claude and Codex apart.
  // The short name ("Claude") gets the long one ("Claude Code") as its tooltip; the long one repeats itself, so it has none.
  const harnessName = (harness, short = false) => { const name = el("span", "hname h-" + harness, (short ? HARNESS_SHORT : HARNESS)[harness] ?? harness); if (short && HARNESS[harness] && HARNESS[harness] !== HARNESS_SHORT[harness]) name.dataset.tip = HARNESS[harness]; return name; };
  // A harness's official mark, drawn unmodified from HARNESSES and served at /harness/*.svg. It says which harness a session belongs to (identity or
  // source) and nothing about its state: the state dot is a separate control that a mark never replaces or merges with. Consistency comes from the
  // container's size (`size`, px) and padding alone; the artwork is never recoloured or cropped. A mark with a light and a dark file draws both and CSS
  // shows one, by the theme (prefers-color-scheme or an explicit data-theme), so the mark follows a theme switch with no script.
  // `label` says whether the mark stands alone: when it does (true, or a string to name it), its container carries the accessible name and a tip, since
  // nothing beside it names the harness. With text beside it the mark is decorative (aria-hidden), and the text is the name.
  // `lead` puts a small gap after the mark, where it sits in running text. An id the registry doesn't know has no mark: null.
  const darkTheme = () => { const t = document.documentElement.getAttribute("data-theme"); return t === "dark" || (t !== "light" && !!window.matchMedia?.("(prefers-color-scheme: dark)").matches); };
  function harnessIcon(harness, { size = 16, label = false, lead = false } = {}) {
    const def = Object.hasOwn(HARNESSES, harness) ? HARNESSES[harness] : null; if (!def) return null;
    const box = el("span", "hicon" + (lead ? " hi-lead" : "")); box.dataset.harness = harness; box.style.setProperty("--hi", size + "px");
    const same = def.icon.light === def.icon.dark, dark = darkTheme();
    for (const [theme, src] of same ? [["", def.icon.light]] : [["light", def.icon.light], ["dark", def.icon.dark]]) {
      const img = document.createElement("img"); img.alt = ""; img.draggable = false; img.decoding = "async";
      if (theme) { img.className = "hi-" + theme; if ((theme === "dark") !== dark) img.loading = "lazy"; } // the variant hidden now loads when a theme change shows it; loading is set before src, which starts the fetch
      img.src = src; box.append(img);
    }
    if (label) { const name = typeof label === "string" ? label : def.name; box.setAttribute("role", "img"); box.setAttribute("aria-label", name); box.dataset.tip = name; }
    else box.setAttribute("aria-hidden", "true");
    return box;
  }
  // Puts the mark first in `node` (text follows it, so the mark is decorative) and returns the node; nothing changes for a harness with no mark.
  const withHarnessIcon = (node, harness, opts = {}) => { const mark = harnessIcon(harness, { lead: true, ...opts }); if (mark) node.prepend(mark); return node; };
  const facetLine = (s) => [s.kind ?? HARNESS[s.harness], MACHINE[s.machine], where(s)].join(" · ");
  const parentOf = (sid) => SESS[sid]?.parent ?? H.find((h) => h.kind === "spawn" && h.to === sid)?.from;
  const originHandoff = (sid) => H.find((h) => (h.kind === "spawn" || h.kind === "relay") && h.to === sid && h.from !== sid && (h.kind === "spawn" || SESS[sid]?.kind === "Relayed" || !SESS[sid]?.lane));
  const RANK = { wait: 0, work: 1, err: 2, done: 3 };
  const isResult = (h) => h.kind === "toyou" && h.ask === "result";
  function markSeenResults(handoffs) {
    let changed = false;
    for (const h of handoffs) if (isResult(h) && typeof h.id === "string" && !SEEN_RESULTS.has(h.id)) {
      SEEN_RESULTS.add(h.id); changed = true;
    }
    while (SEEN_RESULTS.size > SEEN_LIMIT) SEEN_RESULTS.delete(SEEN_RESULTS.values().next().value);
    if (changed) try { window.localStorage.setItem(SEEN_KEY, JSON.stringify([...SEEN_RESULTS])); } catch {}
  }
  const inbox = () => {
    const entries = H.filter((h) => h.kind === "toyou" && (h.status === "wait" || (isResult(h) && !SEEN_RESULTS.has(h.id))));
    const waiting = new Set(entries.filter((h) => h.status === "wait").map((h) => h.from));
    for (const s of Object.values(SESS)) if (s.state === "wait" && !waiting.has(s.id)) entries.push({ id: "wait:" + s.id, kind: "toyou", from: s.id, to: "you", ask: "decision", status: "wait", at: s.waiting_since ?? s.last, brief: s.waiting_for ?? "Waiting for your input or permission" });
    return entries.sort((a, b) => b.at - a.at);
  };
  const working = () => Object.values(SESS).filter((s) => s.state === "work");
  const clean = (t) => t.replace(/[`*]/g, "");

  // Inline marks: `code`, **bold**, *italic*, ~~strike~~, [text](url) and bare URLs. Everything goes in as text nodes;
  // only http(s) links are live, and they open in a new tab. Anything else stays literal text, <tags> included.
  const liveUrl = (u) => { try { return /^https?:$/.test(new URL(u).protocol) && /^https?:\/\//i.test(u) ? u : null; } catch { return null; } };
  const link = (text, url) => { const u = liveUrl(url); if (!u) return document.createTextNode(text); const a = el("a", null, text); a.href = u; a.target = "_blank"; a.rel = "noopener noreferrer"; return a; };
  function inline(parent, text) {
    const re = /(`[^`]+`|\*\*(?:`[^`\n]*`|[^*`\n])*?(?:\*\*|…\s*$)|~~[^~\n]+~~|\[[^\]\n]+\]\((?:[^()\s]|\([^()\s]*\))+\)|https?:\/\/[^\s<>`)\]]+|(?<![\w*])\*[^*\s](?:[^*\n]*[^*\s])?\*(?![\w*]))/g; let last = 0, m;
    while ((m = re.exec(text))) {
      if (m.index > last) parent.append(text.slice(last, m.index));
      let t = m[0];
      if (t[0] === "`") parent.append(el("code", null, t.slice(1, -1)));
      else if (t.startsWith("**")) { const s = el("strong"); inline(s, t.endsWith("**") ? t.slice(2, -2) : t.slice(2)); parent.append(s); } // unclosed only when truncation cut it
      else if (t.startsWith("~~")) { const s = el("del"); inline(s, t.slice(2, -2)); parent.append(s); }
      else if (t[0] === "[") { const k = t.indexOf("]("); parent.append(link(t.slice(1, k), t.slice(k + 2, -1))); }
      else if (t[0] === "*") { const s = el("em"); inline(s, t.slice(1, -1)); parent.append(s); }
      else { t = t.replace(/[.,;:!?'"…]+$/, ""); parent.append(link(t, t)); re.lastIndex = m.index + t.length; } // a bare URL; trailing punctuation isn't part of it
      last = m.index + t.length;
    }
    if (last < text.length) parent.append(text.slice(last));
  }
  // Block markdown, line by line: fenced code, pipe tables, headings, quotes, rules, and lists (-, * or 1. / 1), nested by
  // indent). Any other line is its own paragraph, so a line break in the log stays one. A fence or table cut short when
  // this copy was made renders what is there.
  const cells = (line) => line.trim().replace(/^\|/, "").replace(/\|$/, "").split(/(?<!\\)\|/).map((c) => c.trim().replace(/\\\|/g, "|"));
  const isSep = (line) => /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*…?\s*$/.test(line) && line.includes("-");
  function table(lines, i, box) {
    const wrap = el("div", "tbl"), tb = el("table"), th = el("thead"), hr = el("tr"), body = el("tbody"); wrap.tabIndex = 0; wrap.setAttribute("role", "region"); wrap.setAttribute("aria-label", "Table");
    cells(lines[i]).forEach((c) => hr.append(rich("th", null, c))); th.append(hr);
    for (i += 2; i < lines.length && /^\s*\|/.test(lines[i]); i++) { const r = el("tr"); cells(lines[i]).forEach((c) => r.append(rich("td", null, c))); body.append(r); }
    tb.append(th, body); wrap.append(tb); box.append(wrap); return i - 1;
  }
  function markdown(text, cls = "body") {
    const box = el("div", cls + " md"), lines = text.split("\n"); let lists = []; // open lists, outermost first: { ind, list, ordered }
    for (let i = 0; i < lines.length; i++) {
      const line = lines[i], fence = /^\s*(```|~~~)\s*([\w+#.-]*)/.exec(line);
      if (fence) { lists = []; const code = []; for (i++; i < lines.length && !lines[i].trimStart().startsWith(fence[1]); i++) code.push(lines[i]);
        const w = el("div", "codeblock"); w.tabIndex = 0; w.setAttribute("role", "region"); w.setAttribute("aria-label", (fence[2] ? fence[2] + " " : "") + "code"); w.append(el("pre", null, code.join("\n"))); box.append(w); continue; }
      if (/^\s*\|/.test(line) && isSep(lines[i + 1] ?? "")) { lists = []; i = table(lines, i, box); continue; }
      if (/^\s*([-*_])(\s*\1){2,}\s*$/.test(line)) { lists = []; box.append(el("hr")); continue; }
      const li = /^(\s*)([-*]|\d{1,3}[.)])\s+(.*)$/.exec(line);
      if (li) {
        const ind = li[1].replace(/\t/g, "    ").length, ordered = /\d/.test(li[2]);
        while (lists.length && (ind < lists.at(-1).ind || (ind === lists.at(-1).ind && lists.at(-1).ordered !== ordered))) lists.pop();
        let top = lists.at(-1);
        if (!top || ind > top.ind) { const L = el(ordered ? "ol" : "ul"); if (ordered && parseInt(li[2], 10) !== 1) L.start = parseInt(li[2], 10); (top ? top.list.lastElementChild ?? top.list : box).append(L); lists.push(top = { ind, list: L, ordered }); }
        const item = el("li"); inline(item, li[3]); top.list.append(item); continue;
      }
      if (lists.length && /^\s+\S/.test(line)) { const item = lists.at(-1).list.lastElementChild; item.append(el("br")); inline(item, line.trim()); continue; } // an item's wrapped line
      lists = [];
      const h = /^(#{1,6})\s+(.*)$/.exec(line);
      if (h) { const x = el("p", "mh mh" + Math.min(h[1].length, 3)); x.setAttribute("role", "heading"); x.setAttribute("aria-level", String(Math.min(6, h[1].length + 2))); inline(x, h[2]); box.append(x); continue; }
      if (/^\s*>/.test(line)) { const q = el("blockquote"); for (; i < lines.length && /^\s*>/.test(lines[i]); i++) { const p = el("p"); inline(p, lines[i].replace(/^\s*>\s?/, "")); q.append(p); } i--; box.append(q); continue; }
      const p = el("p"); inline(p, line); box.append(p);
    }
    return box;
  }
  // A one-line preview (Home): block markers dropped, lines run together.
  const preview = (t) => t.split("\n").map((l) => l.replace(/^\s*(#{1,6}\s+|>\s?|[-*]\s+|\d{1,3}[.)]\s+)/, "").trim()).filter((l) => l && !/^\s*\|?\s*:?-{2,}/.test(l)).join(" ");
  // What you answered, when the logs kept it: h.answer holds one string per question, in the brief's order.
  const answersOf = (h) => h.kind === "toyou" && h.ask !== "result" && h.status === "done" ? (Array.isArray(h.answers) ? h.answers.map((a) => (a.values ?? []).join(", ")) : Array.isArray(h.answer) ? h.answer : h.answer != null ? [h.answer] : []).map((a) => String(a).trim()).filter(Boolean) : null;
  function answerEl(h, cls) {
    const a = answersOf(h); if (!a) return null; const r = el("div", cls + " answer" + (a.length ? "" : " none"));
    if (!a.length) { r.append(el("b", null, "Answered"), " · reply not in these logs"); return r; }
    r.append(el("b", null, a.length === 1 ? "You answered: " : "You answered:"));
    if (a.length === 1) r.append(a[0]); else { const ol = el("ol"); a.forEach((x) => ol.append(el("li", null, x))); r.append(ol); }
    return r;
  }
  const rich = (tag, cls, text) => { const n = el(tag, cls); inline(n, text); return n; };

  // What a handoff says, from a viewpoint. Returns [icon, [parts...]].
  function sentence(h, viewer, links = false) {
    const W = (id, action) => {
      const name = nameOf(id);
      if (!links || !action || id === "you" || id === viewer || !SESS[id]) return el("span", "who", name);
      const b = el("button", "who-link", name); b.type = "button"; b.setAttribute("aria-label", action.label(name));
      b.addEventListener("click", (ev) => { ev.stopPropagation(); action.open(); });
      return b;
    };
    const kindOf = (id) => SESS[id]?.kind === "Subagent" ? "subagent" : SESS[id]?.kind ?? "";
    if (h.kind === "ask") {
      const recipient = { label: (name) => "Open " + name + " at the turn this started", open: () => goSession(h.to, STARTS.get(h.id)?.id) };
      return [I.ask, [W("you"), el("span", "verb", " asked "), W(h.to, recipient)]];
    }
    if (h.kind === "spawn") {
      const sender = { label: (name) => "Open " + name + " where it sent this", open: () => openSender(h) };
      const recipient = { label: (name) => "Open " + name + " at the turn this started", open: () => goSession(h.to, STARTS.get(h.id)?.id) };
      return viewer === h.to ? [I.in, [el("span", "verb", "Brief from "), W(h.from, sender)]] : [I.out, [W(h.from, sender), el("span", "verb", " handed off to " + kindOf(h.to) + " "), W(h.to, recipient)]];
    }
    if (h.kind === "relay") {
      const sender = { label: (name) => "Open " + name + " where it sent this", open: () => openSender(h) };
      const recipient = { label: (name) => "Open " + name + " at the turn this started", open: () => goSession(h.to, STARTS.get(h.id)?.id) };
      return viewer === h.to ? [I.in, [el("span", "verb", "Relay from "), W(h.from, sender)]] : [I.out, [W(h.from, sender), el("span", "verb", " relayed to "), W(h.to, recipient)]];
    }
    if (h.kind === "move") {
      const ends = [h.fromMachine, h.toMachine], short = machineShorts(ends.map((id) => [id, MACHINE[id] ?? id]));
      const M = (id) => {
        const label = MACHINE[id] ?? id, n = links ? el("button", "verb mach", short.get(id)) : el("span", "verb mach", short.get(id));
        if (links) { n.type = "button"; n.setAttribute("aria-label", "Open machine " + label); n.addEventListener("click", (ev) => { ev.stopPropagation(); go({ v: "machine", id }); }); }
        n.dataset.tip = "Machine: " + label; return n;
      };
      const session = { label: (name) => "Open " + name, open: () => goSession(h.to) };
      return [I.move, [el("span", "verb", "Semon moved "), W(h.to, session), el("span", "verb", " from "), M(h.fromMachine), el("span", "verb", " to "), M(h.toMachine)]];
    }
    const what = { question: " asked you", result: " sent you a result", decision: " needs your decision" }[h.ask];
    const answered = h.status === "done" && (h.ask === "question" || h.ask === "decision");
    const sender = { label: (name) => "Open " + name + " where it sent this", open: () => openSender(h) };
    return [answered ? I.done : h.ask === "question" ? I.qc : h.ask === "decision" ? I.decide : I.result, [W(h.from, sender), el("span", "verb", what)]];
  }
  const statWord = (h) => isResult(h) ? SEEN_RESULTS.has(h.id) ? "read" : "new" : ({ work: "working", wait: "waiting on you", err: "failed", done: h.kind === "toyou" ? "answered" : h.result ? "returned" : "delivered" })[h.status];

  // ---- Turns and traces, computed from the transcripts ---------------------------------------------------
  // A turn starts at each incoming entry: your message, a relay from another session, or the brief that starts a
  // subagent or Codex run. It runs to the next incoming entry. What it sends on (spawns, relays, messages to you)
  // and machine moves are content inside it. Entries before the first incoming one form a leading turn without a
  // header. A turn's id is its start handoff's id, else session:index. STARTS maps a handoff to the turn it started;
  // HOLDS maps it to the turn that sent it.
  // A gap marker ("Earlier entries not included in this copy") is where this copy skips part of the log. It ends the
  // turn before it, and what follows starts a turn of its own, so the marker is drawn between turns, never inside one.
  const isGap = (e) => e.k === "end" && /entries (not included|omitted)|^No activity/.test(e.text ?? "");
  const HID = new Map();
  // TURNS, TURN, STARTS and HOLDS keep the shapes above, filled from the server's turn index (adopt), since the page no longer
  // holds every transcript. A turn's entries are the loaded ones (spread).
  const TURNS = {}, TURN = new Map(), STARTS = new Map(), HOLDS = new Map();
  const hasTurn = (t) => !!t.end || !!(t.start || t.u) || t.entries.some((e) => e.k === "a" || e.k === "tool" || e.k === "h");
  const oneLine = (s) => clean(s).replace(/\s+/g, " ").trim();
  // How a turn ended: still working, a message to you, a failure, a return or reply, or nothing recorded.
  const TOYOU = { question: "Asked you", result: "Sent you a result", decision: "Needs your decision" };
  function turnEnd(t) {
    if (!hasTurn(t)) return null;
    // The model leaves an active or partially indexed turn without an end record. Keep the mockup's
    // transcript-derived fallback so outgoing work remains traceable while the run is in progress.
    if (!t.end) {
      if (t.last && SESS[t.sid]?.state === "wait") return { st: "wait", text: "Waiting on permission or input" };
      if (t.last && SESS[t.sid]?.state === "work") return { st: "work", text: "Still working" };
      const ty = t.out.filter((h) => h.kind === "toyou").at(-1);
      if (ty) return { st: ty.status === "wait" ? "wait" : "done", text: (TOYOU[ty.ask] ?? "Sent you a message") + " · " + statWord(ty) + " · " + clock(ty.at) };
      const entries = t.entries.filter((e) => e.k === "a" || e.k === "tool" || (e.k === "h" && t.out.includes(HID.get(e.id))));
      const last = entries.at(-1), h = last?.k === "h" ? HID.get(last.id) : null;
      if (t.start?.status === "err") return { st: "err", text: "Failed" + (t.start.done ? " · " + clock(t.start.done) : "") };
      if (last?.k === "tool" && last.ok === false && !last.live) return { st: "err", text: last.unfinished ? "Stopped on a step with no result" : "Stopped on a failed step" };
      if (h?.status === "err") return { st: "err", text: "Handoff to " + nameOf(h.to) + " failed" };
      if (t.start?.kind === "spawn" && t.start.status === "done") return { st: "done", text: "Returned to " + nameOf(t.start.from) + (t.start.done ? " · " + clock(t.start.done) : "") };
      if (entries.some((e) => e.k === "a")) return { st: "done", text: "Replied" };
      return { st: "idle", text: "No reply in these logs" };
    }
    const { st, why } = t.end, mh = t.end.h ? HID.get(t.end.h) : null;
    if (why === "input") return { st: "wait", text: "Waiting for your input" };
    if (why === "permission" || why === "waiting" || why === "waiting_permission") return { st: "wait", text: "Waiting on permission" };
    if (why === "working") return { st: "work", text: "Still working" };
    if (why === "toyou" && mh) return { st: mh.status === "wait" ? "wait" : "done", text: (TOYOU[mh.ask] ?? "Sent you a message") + " · " + statWord(mh) + " · " + clock(mh.at) };
    if (why === "failed") return { st: "err", text: "Failed" + (t.start?.done ? " · " + clock(t.start.done) : "") };
    if (why === "unfinished_step") return { st: "err", text: "Stopped on a step with no result" };
    if (why === "failed_step") return { st: "err", text: "Stopped on a failed step" };
    if (why === "handoff_failed" && mh) return { st: "err", text: "Handoff to " + nameOf(mh.to) + " failed" };
    if (why === "returned" && t.start) return { st: "done", text: "Returned to " + nameOf(t.start.from) + (t.start.done ? " · " + clock(t.start.done) : "") };
    if (why === "replied") return { st: "done", text: "Replied" };
    return { st: st ?? "idle", text: "No reply in these logs" };
  }
  // Where a turn's trace began: from a relay or brief, step back to the sender's turn that sent it, until a turn
  // that started with your message or whose sender's side isn't in the logs.
  function traceRoot(t) { const seen = new Set(); while (!seen.has(t.id)) { seen.add(t.id); const up = t.start && t.start.from !== "you" ? HOLDS.get(t.start.id) : null; if (!up) break; t = up; } return t; }
  function originLine(t) {
    const r = traceRoot(t), msg = r.start?.from === "you" ? r.start.brief : r.u?.text, o = el("span", "org");
    if (msg) o.append("From your message: ", el("span", "oq", "“" + oneLine(msg) + "”")); else o.append("Started by " + nameOf(r.start ? r.start.from : r.sid));
    return o;
  }


  // ---- Loading: the model from /api/model, transcripts a page at a time from /api/tx --------------------------
  const TXM = {}; // per session: the loaded range of its transcript { from, to, total } and its totals { calls, errors }
  // A session's tool calls and errors, from the model (`calls` and `errors` on each session; absent from an older server or
  // cache: null, shown as "—"). Nothing fetches a transcript only to count it. A transcript loaded to its end is tailed by
  // every update, so its own totals agree with its entries; a range that stops short (a deep link, a child's start turn)
  // keeps the totals from when it was fetched, so the model's win there.
  const countOf = (s, key) => { const m = TXM[s.id]; return (m && m.to >= m.total ? m[key] : undefined) ?? s[key] ?? m?.[key] ?? null; };
  const callsText = (calls) => (calls == null ? "—" : calls) + (calls === 1 ? " tool call" : " tool calls");
  let serverNow = 0, fetchedAt = 0;
  let TOK = {}; // per session: its transcript's growth mark in the model; a loaded transcript is tailed only when it moved
  const enc = encodeURIComponent;
  const safePath = (href) => typeof href === "string" && href.startsWith("/") && !href.startsWith("//") && !href.includes("\\") && !/[\u0000-\u001f\u007f-\u009f]/.test(href) && href.length <= 512;
  const accountOf = parseAccount;
  // The embedding page's menu, `window.semonEmbed.account`, validated like a server's. A rejected one is reported once on
  // the console, since the embedding page gets no other sign of it.
  let embedWarned = false;
  function embeddedAccount() {
    let source; try { source = window.semonEmbed?.account; } catch { source = undefined; }
    const account = accountOf(source);
    if (source != null && !account && !embedWarned) { embedWarned = true; console.warn("semon: window.semonEmbed.account was rejected (see the account menu rules)"); }
    return account;
  }
  // An error carries the HTTP status (0: no response), so live polling can tell a 403 from a dropped connection.
  const api = (path, signal) => fetch(path, { credentials: "same-origin", signal }).then((r) => { if (!r.ok) throw Object.assign(new Error(r.status + " " + r.statusText), { status: r.status }); return r.json(); }, (e) => { throw Object.assign(e, { status: 0 }); });
  // NOW follows the client clock from the model's `now`, so every "ago" keeps moving; a running tool's age follows NOW.
  function tick() {
    NOW = serverNow + (Date.now() - fetchedAt);
    for (const s of Object.values(SESS)) if (s.activity && s.activity[3] != null) s.activity[2] = Math.floor((NOW - s.activity[3]) / 1000);
  }
  let wireModel = null;
  function adopt(m) {
    wireModel = structuredClone(m);
    CHILDREN = null;
    serverNow = m.now; fetchedAt = Date.now(); TOK = m.tx ?? {};
    for (const k of Object.keys(MACHINE)) { delete MACHINE[k]; delete MACHINE_UP[k]; delete MACHINE_LAST[k]; }
    // Several machines come as `machines`; one comes as `machine` alone.
    for (const x of m.machines ?? [m.machine]) { MACHINE[x.id] = x.name; MACHINE_UP[x.id] = x.up; if (x.last != null) MACHINE_LAST[x.id] = x.last; }
    ADMIN = m.admin && safePath(m.admin.href) ? m.admin : null;
    // A server-provided menu wins; otherwise an embedding page may set `window.semonEmbed.account`, held to the same rules.
    ACCOUNT = accountOf(m.account) ?? embeddedAccount();
    NAV_MACHINES = m.nav && safePath(m.nav.machines) ? m.nav.machines : null;
    for (const k of Object.keys(SESS)) delete SESS[k];
    for (const [id, s] of Object.entries(m.sessions)) { s.id = id; SESS[id] = s; }
    H.length = 0; H.push(...m.handoffs);
    // A failed send reached no one: a stub named as the send addressed it stands for the other end, marked failed.
    for (const h of H) if (h.to == null) {
      // With several machines, a stand-in belongs to the sender's machine: nothing is linked across machines.
      const machine = SESS[h.from]?.machine ?? m.machine.id, id = "unsent:" + (h.target ?? "") + (m.machines ? "@" + machine : "");
      const s = SESS[id] ??= { id, name: h.target || "unknown", harness: "claude", stub: true, machine, state: "err", model: "—", tokens: [0, 0, 0], start: h.at, last: h.at, busy: [] };
      s.start = Math.min(s.start, h.at); s.last = Math.max(s.last, h.at); h.to = id;
    }
    HID.clear(); for (const h of H) HID.set(h.id, h);
    for (const k of Object.keys(TURNS)) delete TURNS[k];
    TURN.clear(); STARTS.clear(); HOLDS.clear();
    for (const x of m.turns) {
      const t = { id: x.id, sid: x.sid, start: x.start ? HID.get(x.start) ?? null : null, at: x.at, u: x.u ? { k: "u", text: x.text ?? "" } : null, entries: [], out: [], sent: [], end: x.end };
      t.sent = x.sent.map((id) => HID.get(id)).filter(Boolean); t.out = t.sent.filter((h) => h.kind !== "move"); if (x.last) t.last = true;
      (TURNS[x.sid] ??= []).push(t); TURN.set(t.id, t); if (t.start) STARTS.set(t.start.id, t); for (const h of t.sent) HOLDS.set(h.id, t);
    }
    tick();
  }
  // Each loaded entry goes to its turn: an entry that starts a turn (or a page) names it. Its key, its turn and place in
  // it, stays the same while the transcript only grows: live updates find what was open and where the reader was by it.
  function spread(sid) {
    for (const t of TURNS[sid] ?? []) t.entries = [];
    let t = null, pre = 0;
    for (const e of TX[sid] ?? []) { if (e.turn) t = TURN.get(e.turn) ?? null; if (t) { e.key = t.id + "#" + t.entries.length; t.entries.push(e); } else e.key = sid + "#" + pre++; }
  }
  // A return line arrives as data; it reads as the mockup's "Returned to … · HH:MM".
  const txEntry = (e) => e.k === "end" && e.ret ? { k: "end", text: "Returned to " + nameOf(e.ret.to) + (e.ret.failed ? " · failed" : "") + (e.ret.at != null ? " · " + clock(e.ret.at) : ""), turn: e.turn } : e;
  // where: "before" and "after" extend the loaded range; otherwise the page replaces it.
  function fetchTx(sid, q, where, signal, onPage) {
    const tok = TOK[sid], range = TXM[sid], boundary = where === "before" ? range?.from : range?.to;
    return api("/api/tx?sid=" + enc(sid) + (q ? "&" + q : ""), signal).then((p) => {
      // An error jump or a reload may replace the range while a pager request is out. Its page no longer adjoins ours.
      if (signal?.aborted || (where && (TXM[sid] !== range || (where === "before" ? TXM[sid]?.from : TXM[sid]?.to) !== boundary))) return;
      const es = p.entries.map((e) => txEntry({ ...e, sid })), m = TXM[sid];
      if (where === "before" && m) { TX[sid] = es.concat(TX[sid]); m.from = p.from; }
      else if (where === "after" && m) { TX[sid] = TX[sid].concat(es); m.to = p.to; }
      else { clearPaging(sid); TX[sid] = es; TXM[sid] = { from: p.from, to: p.to }; STALE_BRIEFS.delete(sid); }
      Object.assign(TXM[sid], { total: p.total, calls: p.calls, errors: p.errors, watchTok: tok }); if (where !== "before" && p.to >= p.total) { TXM[sid].tok = tok; TXM[sid].newer = 0; } spread(sid);
      onPage?.(); // optional notification for a pager; the promise still resolves without a value
    });
  }
  // What a route needs before it can draw: a session's page (the one holding a deep-linked turn).
  // `signal` cancels what a navigation asked for when the reader goes elsewhere first.
  function load(r, signal) {
    if (r.v === "analytics") return fetchAnalytics().then(() => { scheduleAnalytics(); }); // the range's answer, from the server
    if (r.v !== "session" || !SESS[r.id]) return null;
    const t = r.turn ? TURN.get(r.turn) : null, deep = t && t.sid === r.id && !t.entries.length;
    if (TX[r.id] && !deep) return null;
    return fetchTx(r.id, deep ? "turn=" + enc(t.id) : "", undefined, signal);
  }
  // The last few transcripts opened, kept when the reader leaves them, so opening one again draws it at once. (A transcript
  // still in TX, which only a model update prunes, draws from there just the same.) A transcript is kept only when it was
  // loaded to its end, and the cache is bounded by entries and by estimated memory: two bytes for each character of an entry's
  // text, since JavaScript strings are UTF-16. Opening one takes it out of the cache; leaving it puts it back at the newest end.
  const STALE_BRIEFS = new Set();
  const TXCACHE = new Map(), TXCACHE_MAX = 5, TXCACHE_BYTES = 2 * 1024 * 1024;
  const weigh = (entries) => { let n = 0; for (const e of entries) for (const v of Object.values(e)) n += typeof v === "string" ? v.length : v && typeof v === "object" ? JSON.stringify(v).length : 4; return n * 2; };
  function cacheTx(sid, entries, meta) {
    if (!entries || !meta || meta.to < meta.total || meta.tok == null || STALE_BRIEFS.has(sid) || meta.origin !== !!originHandoff(sid)) return; // without its mark there is no telling later whether it grew
    TXCACHE.delete(sid);
    const bytes = weigh(entries); if (bytes > TXCACHE_BYTES) return;
    TXCACHE.set(sid, { entries, meta, bytes });
    let sum = 0; for (const c of TXCACHE.values()) sum += c.bytes;
    for (const [id, c] of TXCACHE) { if (TXCACHE.size <= TXCACHE_MAX && sum <= TXCACHE_BYTES) break; TXCACHE.delete(id); sum -= c.bytes; }
  }
  // A kept transcript becomes the session's loaded one; the caller spreads its entries over the turns before drawing (that walks
  // every entry, so it is not done in the click's task, except to check a deep link). False when there is none, or when the
  // route deep-links to a turn it lacks.
  function adoptCached(r) {
    const c = TXCACHE.get(r.id); if (!c) return false;
    TXCACHE.delete(r.id); TX[r.id] = c.entries; TXM[r.id] = c.meta;
    if (!r.turn) return true;
    spread(r.id); const t = TURN.get(r.turn);
    if (t && t.sid === r.id && !t.entries.length) { dropTx(r.id); return false; }
    return true;
  }
  // A mark with fewer entries or bytes than the one loaded means the file was cut or rewritten: load it again.
  function shrank(a, b) { const [s0, b0] = String(a).split(".").map(Number), [s1, b1] = String(b).split(".").map(Number); return s1 < s0 || b1 < b0; }
  // A transcript drawn from the cache is brought up to date the way a live update does it: when the model's mark for it moved
  // since it was kept, its tail is fetched (or the whole page, if the file shrank), and its child work loads. Nothing is asked
  // for when the mark is the same. The page is drawn again, keeping the reader's place, once something arrived.
  function revalidate(r) {
    const sid = r.id, m = TXM[sid], moved = m && m.to >= m.total && m.tok != null && TOK[sid] != null && m.tok !== TOK[sid];
    const job = moved ? (shrank(m.tok, TOK[sid]) ? reload(sid) : tail(sid)) : null, work = job;
    if (work) work.then(() => { if (route === r && rendered === r) refresh(null); }, () => {});
  }
  // Paging state survives redraws: a click and an observer share one request per session and direction, and a failed
  // page stays manual until Retry succeeds. Observers belong only to the buttons currently drawn.
  const PAGING = new Map(), pagerObservers = new Map();
  let pagerFrame = null, pagerArmed = false, automaticLoads = 0, scrollRevision = 0, programmaticScrollPending = false, programmaticScrollTimer = null;
  function clearPaging(sid) {
    for (const state of Object.values(PAGING.get(sid) ?? {})) state.controller?.abort();
    PAGING.delete(sid);
    if (route.v === "session" && route.id === sid) resetPagerInput();
  }
  function dropTx(sid) { clearPaging(sid); delete TX[sid]; delete TXM[sid]; }
  function resetPagerInput() { pagerArmed = false; automaticLoads = 0; scrollRevision++; disconnectPagerObservers(); }
  function holdProgrammaticScroll() {
    programmaticScrollPending = true;
    clearTimeout(programmaticScrollTimer);
    // Scroll events arrive after scrollTop writes. Smooth jumps keep extending this guard until scrolling is quiet.
    programmaticScrollTimer = setTimeout(() => { programmaticScrollPending = false; programmaticScrollTimer = null; }, 120);
  }
  function scrollProgrammatically(fn, jump = true) {
    if (jump) resetPagerInput();
    holdProgrammaticScroll(); fn();
  }
  function readerScrollInput() {
    if (route.v !== "session" || rendered !== route || $("#page").hasAttribute("aria-busy")) return;
    clearTimeout(programmaticScrollTimer); programmaticScrollTimer = null; programmaticScrollPending = false;
    stopOpeningEndPin(); pagerArmed = true; automaticLoads = 0; scrollRevision++; queuePagerObservers();
  }
  const automaticPagingAllowed = () => pagerArmed && automaticLoads < 3 && !openingEndUntil && !findOpen && !find && show.messages && show.tools && show.thinking;
  function pagingState(sid, where) {
    if (!PAGING.has(sid)) PAGING.set(sid, { before: { busy: false, failed: false }, after: { busy: false, failed: false } });
    return PAGING.get(sid)[where];
  }
  function paintPager(b) {
    const state = pagingState(b.dataset.pagerSid, b.dataset.pagerWhere), direction = b.dataset.pagerWhere === "before" ? "earlier" : "later";
    b.disabled = state.busy;
    const label = b.querySelector(".pager-label") ?? el("span", "pager-label");
    if (!label.parentNode) b.replaceChildren(label);
    b.querySelector(".spin")?.remove();
    if (state.busy) { const spin = el("span", "spin"); spin.setAttribute("aria-hidden", "true"); b.prepend(spin); }
    label.setAttribute("aria-live", "polite");
    label.textContent = spaced(state.busy ? "Loading " + direction + "…" : state.failed ? "Couldn't load " + direction + " entries · Retry" : "Load " + direction + (b.dataset.pagerWhere === "after" && TXM[b.dataset.pagerSid]?.newer ? " · " + TXM[b.dataset.pagerSid].newer + " new" : ""));
  }
  function disconnectPagerObservers() {
    if (pagerFrame !== null) cancelAnimationFrame(pagerFrame); pagerFrame = null;
    for (const observer of pagerObservers.values()) observer.disconnect();
    pagerObservers.clear();
  }
  function queuePagerObservers() {
    if (pagerFrame !== null || SIDEBAR_ONLY) return;
    // Wait until rendering, opening at the end and restoring the reader's place have finished.
    pagerFrame = requestAnimationFrame(() => {
      pagerFrame = null; disconnectPagerObservers();
      if (route.v !== "session" || rendered !== route || !automaticPagingAllowed() || $("#page").hasAttribute("aria-busy") || typeof IntersectionObserver === "undefined") return;
      for (const b of $("#page").querySelectorAll("[data-pager-where]")) {
        const state = pagingState(b.dataset.pagerSid, b.dataset.pagerWhere);
        if (state.busy || state.failed) continue;
        const observer = new IntersectionObserver((entries) => {
          if (pagerObservers.get(b) === observer && entries.some((entry) => entry.isIntersecting)) loadPager(b, false);
        }, { root: phone.matches ? null : $("#main"), rootMargin: b.dataset.pagerWhere === "before" ? "800px 0px 0px 0px" : "0px 0px 800px 0px" });
        pagerObservers.set(b, observer); observer.observe(b);
      }
    });
  }
  async function loadPager(b, manual) {
    const sid = b.dataset.pagerSid, where = b.dataset.pagerWhere, state = pagingState(sid, where), r = route;
    if (!b.isConnected || r.v !== "session" || r.id !== sid || rendered !== r || state.busy || (!manual && (state.failed || !automaticPagingAllowed()))) return;
    if (manual) stopOpeningEndPin();
    const m = TXM[sid]; if (!m || (where === "before" ? m.from <= 0 : m.to >= m.total)) return;
    const boundary = where === "before" ? m.from : m.to;
    if (!manual) automaticLoads++;
    state.busy = true; state.controller = new AbortController(); paintPager(b);
    try {
      let applied = false;
      await fetchTx(sid, where + "=" + boundary, where, state.controller.signal, () => { applied = true; });
      if (!applied || PAGING.get(sid)?.[where] !== state || TXM[sid] !== m) return;
      state.failed = false;
      if (route !== r || rendered !== r || PAGING.get(sid)?.[where] !== state) return;
      // Capture at the last moment: the reader can keep scrolling while the request is out.
      const box = scroller(), top = phone.matches ? 0 : box.getBoundingClientRect().top, st = capture();
      const entry = [...$("#page").querySelectorAll(".turns [data-e][data-entry-key]:not(.tgroup)")].find((n) => { const rect = n.getBoundingClientRect(); return rect.height && rect.top >= top; });
      st.paging = { anchor: entry ? { key: entry.dataset.entryKey, off: entry.getBoundingClientRect().top - top } : null, height: box.scrollHeight, before: where === "before" };
      const armed = pagerArmed, used = automaticLoads;
      render(); restore(st);
      // This paging redraw may continue the same reader gesture, within its three-page limit.
      if (!manual) { pagerArmed = armed; automaticLoads = used; }
      syncJump(); saveHistoryScroll();
    } catch (error) {
      if (error?.name !== "AbortError" && PAGING.get(sid)?.[where] === state) state.failed = true;
    } finally {
      state.busy = false; state.controller = null;
      if (PAGING.get(sid)?.[where] === state) {
        for (const button of $("#page").querySelectorAll("[data-pager-where]")) if (button.dataset.pagerSid === sid && button.dataset.pagerWhere === where) paintPager(button);
        queuePagerObservers();
      }
    }
  }
  // "Load earlier" at the top of a transcript, and "Load later" at its end when a deep link loaded a middle page.
  function pager(sid, where, label) {
    const w = el("div", "list"), b = el("button", "more", label); b.type = "button"; if (where === "before") b.dataset.loadEarlier = ""; w.append(b); // data-load-earlier: a stable hook for the budget check
    b.dataset.pagerSid = sid; b.dataset.pagerWhere = where; paintPager(b);
    b.addEventListener("click", () => loadPager(b, true));
    return w;
  }
  // "View all" reads the whole call: each part the server cut ("more") is fetched in full from /api/entry, by the entry's
  // session and slot. A part cut again at 8 MB is noted (fullCut).
  function fullOf(e) {
    const part = (as) => api("/api/entry?sid=" + enc(e.sid) + "&slot=" + e.slot + "&as=" + as).then((r) => [as, r]);
    return Promise.all((e.more ?? []).map(part)).then((rs) => {
      const f = { fullCut: [] };
      for (const [k, r] of rs) { f[k] = k === "diff" ? r.diff : r.text; if (k === "diff" && r.changes) f.changes = r.changes; if (k === "out") f.cut = r.cut ?? null; if (r.truncated) f.fullCut.push(k); }
      return f;
    });
  }
  // Real URLs: every screen has one, and the server serves this page for each.
  function urlOf(r) {
    const hs = (id) => SESS[id]?.harness ?? "claude";
    return r.v === "home" ? "/" : r.v === "analytics" ? "/analytics" : r.v === "sessions" ? "/sessions" : r.v === "machines" ? "/machines"
      : r.v === "machine" ? "/machines/" + enc(r.id)
      : r.v === "session" ? "/s/" + hs(r.id) + "/" + enc(r.id) + (r.turn ? "?turn=" + enc(r.turn) : "")
      : "/trace/" + hs(r.sid) + "/" + enc(r.sid) + "/" + enc(r.turn);
  }
  function routeOf(loc) {
    const p = loc.pathname.split("/").filter(Boolean).map((x) => { try { return decodeURIComponent(x); } catch { return x; } }), turn = new URLSearchParams(loc.search).get("turn");
    if (p[0] === "timeline" || p[0] === "analytics") return { v: "analytics" };
    if (p[0] === "sessions") return { v: "sessions" };
    if (p[0] === "machines") return p[1] && MACHINE[p[1]] ? { v: "machine", id: p[1] } : { v: "machines" };
    if (p[0] === "s" && SESS[p[2]]) {
      let fragment = ""; try { fragment = decodeURIComponent(loc.hash.slice(1)); } catch { fragment = loc.hash.slice(1); }
      const fragmentTurn = TURN.get(fragment.split("#")[0]);
      const targetTurn = turn ?? (fragmentTurn?.sid === p[2] ? fragmentTurn.id : null);
      return targetTurn ? { v: "session", id: p[2], turn: targetTurn } : { v: "session", id: p[2] };
    }
    if (p[0] === "trace" && SESS[p[2]] && p[3]) return { v: "trace", sid: p[2], turn: p[3] };
    return { v: "home" };
  }
  function boot() {
    api("/api/model?delta=1").then((m) => {
      adopt(m); LIVE.version = m.version; remember(m); if (SIDEBAR_ONLY) { render(); schedule(2000); return; } route = routeOf(location);
      if (route.v === "sessions") query = (new URLSearchParams(location.search).get("q") ?? "").trim(); // direct Sessions links can prefill its search field
      if (route.v === "machines" && NAV_MACHINES) { location.assign(NAV_MACHINES); return; }
      try { history.replaceState({ ...route, scrollTop: 0 }, "", urlOf(route) + (route.v === "session" ? location.hash : route.v === "sessions" && query ? "?q=" + enc(query) : "")); } catch {}
      const done = () => {
        render();
        if (route.v === "session" && route.turn) { revealTurn(route.turn, true); if (location.hash) requestAnimationFrame(() => requestAnimationFrame(revealEntryHash)); }
        else if (route.v === "session" && location.hash) revealEntryHash();
        else if (route.v === "session") { openSessionAtEnd(); syncJump(); }
        else quietTop();
        schedule(2000); setInterval(ticker, 1000);
      };
      const initialRoute = route, p = load(initialRoute);
      if (p) p.then(() => { done();  }, done);
      else { done();  }
    }, (err) => { $(SIDEBAR_ONLY ? "#lanes" : "#page").replaceChildren(el("p", SIDEBAR_ONLY ? "ghead" : "empty", "Couldn't load the sessions: " + err.message)); });
  }

  // ---- State & navigation ---------------------------------------------------------------
  const phone = window.matchMedia("(max-width: 760px)");
  let route = { v: "home" }; // (before the layout preferences, which read it)
  let wideMode = false, railMode = false, treePrefs = {};
  try { wideMode = localStorage.getItem("semon.wide") === "1"; } catch {}
  try { railMode = !SIDEBAR_ONLY && localStorage.getItem("semon.rail") === "1"; } catch {} // the rail is the viewer's own layout: an embedding page keeps its sidebar whole
  try { const saved = JSON.parse(localStorage.getItem("semon.tree") ?? "{}"); if (saved && typeof saved === "object" && !Array.isArray(saved)) treePrefs = pruneTreePrefs(saved); } catch {}
  const app = $(".app");
  const syncLayoutPrefs = () => { if (SIDEBAR_ONLY) return; app.classList.toggle("rail", railMode && !phone.matches); $("#page").classList.toggle("wide-mode", wideMode && !phone.matches && route.v === "session"); };
  function setWideMode(on) { wideMode = on; try { localStorage.setItem("semon.wide", on ? "1" : "0"); } catch {} syncLayoutPrefs(); $(".wide-toggle")?.setAttribute("aria-pressed", String(on)); accountChrome.updateWide(on); }
  function setRailMode(on) { railMode = on; ORD.delete("side"); try { localStorage.setItem("semon.rail", on ? "1" : "0"); } catch {} syncLayoutPrefs(); expandedAll = null; renderLanes(); const b = $("#rail-toggle"); b?.setAttribute("aria-expanded", String(!on)); b?.setAttribute("aria-label", on ? "Expand sidebar" : "Collapse sidebar"); b?.setAttribute("data-tip", on ? "Expand sidebar" : "Collapse sidebar"); }
  // A parent's saved choice is whether it is `open`. Saves from before the sidebar's "All N" row also held `more`, which nothing reads now:
  // it is dropped on load, along with any entry that has no `open`, and the next save writes the pruned list.
  function pruneTreePrefs(saved) {
    const kept = {};
    for (const [id, pref] of Object.entries(saved)) if (pref && typeof pref === "object" && typeof pref.open === "boolean") kept[id] = { open: pref.open, at: Number(pref.at) || 0 };
    return kept;
  }
  function saveTreePref(id, open) {
    treePrefs[id] = { open, at: Date.now() };
    treePrefs = Object.fromEntries(Object.entries(treePrefs).sort((a, b) => (b[1]?.at ?? 0) - (a[1]?.at ?? 0)).slice(0, 500));
    try { localStorage.setItem("semon.tree", JSON.stringify(treePrefs)); } catch {}
  }
  // An embedding page's sidebar has no rail and no toggle for it (shell::session_sidebar): the toggle is then a detached button.
  const railToggle = $("#rail-toggle") ?? el("button"); railToggle.append(icon(I.sidebar)); railToggle.setAttribute("aria-expanded", String(!railMode)); railToggle.setAttribute("data-tip", railMode ? "Expand sidebar" : "Collapse sidebar"); railToggle.setAttribute("aria-label", railMode ? "Expand sidebar" : "Collapse sidebar"); railToggle.addEventListener("click", () => setRailMode(!railMode)); syncLayoutPrefs();
  let groupBy = "recent"; let query = ""; let focusSessionsSearchOnRender = null; let analyticsRange = 7, analyticsMeasure = "hours";
  let showApprovalReviews = false;
  const sessionFilters = { repo: "", machine: "", harness: "", model: "" };
  let pendingSessionOpen = null;
  // The phone's account menu adds a history entry, so the back gesture closes it.
  let accountSheet = false;
  // What to do once the account menu's history entry has been stepped back over (leaving the page from one of its items).
  let afterPop = null;
  if (!SIDEBAR_ONLY) try { history.scrollRestoration = "manual"; } catch {}
  const SHOW_ALL = { messages: true, tools: true, thinking: true };
  let show = { ...SHOW_ALL }; let find = ""; let findOpen = false;
  const currentScroll = () => phone.matches ? window.scrollY : $("#main").scrollTop;
  const restoreScroll = (top) => scrollProgrammatically(() => { if (phone.matches) window.scrollTo(0, top); else $("#main").scrollTop = top; });
  const saveHistoryScroll = () => { if (viewerEl) return; try { if (history.state?.v) history.replaceState({ ...history.state, scrollTop: currentScroll() }, ""); } catch {} };
  let scrollSaveFrame = false;
  const queueScrollSave = () => { if (scrollSaveFrame) return; scrollSaveFrame = true; requestAnimationFrame(() => { scrollSaveFrame = false; saveHistoryScroll(); }); };
  if (!SIDEBAR_ONLY) { window.addEventListener("scroll", queueScrollSave, { passive: true }); $("#main").addEventListener("scroll", queueScrollSave, { passive: true }); }
  const quietTop = () => restoreScroll(0);
  function openSessionAtEnd() { if (location.hash) return; startOpeningEndPin(); }
  // Opening a session draws what the model already holds at once, before its transcript arrives: the sidebar row, the top bar,
  // and the old page held dimmed and inert (aria-busy). If the transcript is still on its way after 150 ms, a skeleton of turn-shaped
  // placeholders stands in for the old page, so fast switches don't flash it. `navAbort` cancels the transcript request of a
  // navigation the reader has left; a response that still arrives late is dropped because its route is no longer the current one.
  const SKELETON_MS = 150;
  // The sidebar's list was drawn for this route under this model version: the render that follows draws it only if either changed.
  let navAbort = null, skeletonTimer = null, lanesFor = null;
  // The turn shapes the skeleton cycles through: a bubble (yours), text lines, a step row. Widths are classes, sk-w1..sk-w5, in percent.
  const SKELETON_TURNS = [["bubble", [1, 3, 5], "step"], [null, [1, 2, 4], "step"], ["bubble", [2, 1, 3], null], [null, [1, 1, 5], "step"]];
  function skeleton(turns = 6) {
    const box = el("div", "skeleton"); box.setAttribute("aria-hidden", "true");
    for (let i = 0; i < turns; i++) {
      const [bubble, lines, step] = SKELETON_TURNS[i % SKELETON_TURNS.length], t = el("div", "sk-turn"), text = el("div", "sk-text");
      if (bubble) t.append(el("span", "sk-line sk-bubble sk-w3"));
      for (const w of lines) text.append(el("span", "sk-line sk-w" + w));
      t.append(text); if (step) t.append(el("span", "sk-line sk-step sk-w2"));
      box.append(t);
    }
    return box;
  }
  function paintPending(r) {
    const page = $("#page"), hadFocus = $("#sidebar").contains(document.activeElement);
    renderNav(); renderLanes(); drawSessionBar();
    lanesFor = { r, version: LIVE.version };
    if (hadFocus) $("#lanes .srow[data-id='" + CSS.escape(r.id) + "']")?.focus({ preventScroll: true });
    page.setAttribute("aria-busy", "true"); page.inert = true; page.classList.add("loading");
    clearTimeout(skeletonTimer);
    skeletonTimer = setTimeout(() => {
      if (route !== r || !page.classList.contains("loading")) return;
      page.classList.remove("loading"); page.classList.remove("child-page"); page.style.paddingBottom = "";
      clearBox(page, r); page.append(skeleton()); quietTop(); syncBarLine();
    }, SKELETON_MS);
  }
  function endLoading() {
    clearTimeout(skeletonTimer); skeletonTimer = null;
    const page = $("#page"); page.removeAttribute("aria-busy"); page.inert = false; page.classList.remove("loading");
  }
  // Once the transcript is in, a reader who hasn't put focus anywhere else on the page moves to the session's title.
  function focusTitle() {
    const a = document.activeElement; if (a && a !== document.body && !$("#sidebar").contains(a)) return;
    const h = $("#page .ph h1"); if (h) { h.tabIndex = -1; h.focus({ preventScroll: true }); }
  }
  // The session's own transcript couldn't be loaded: the skeleton gives way to the reason and a way to try again.
  function failLoad(r, err) {
    if (route !== r || err?.name === "AbortError") return;
    endLoading();
    const page = $("#page"), box = el("div", "load-error"), retry = el("button", "more", "Try again");
    box.setAttribute("role", "alert"); retry.type = "button"; retry.addEventListener("click", () => go({ ...r }, true));
    box.append(el("p", "empty", "Couldn't load this session: " + (err?.message ?? "no response")), retry);
    page.classList.remove("child-page"); page.style.paddingBottom = ""; clearBox(page, r); page.append(box);
  }
  // A deep link to a turn the loaded transcript doesn't hold yet.
  const isDeep = (r) => { const t = r.turn ? TURN.get(r.turn) : null; return !!t && t.sid === r.id && !t.entries.length; };
  function go(r, fromHistory) {
    if (r.v !== "sessions" || r !== focusSessionsSearchOnRender) focusSessionsSearchOnRender = null;
    if (SIDEBAR_ONLY) { if (!fromHistory) { closeDrawer(true); location.assign(r.v === "machines" && NAV_MACHINES ? NAV_MACHINES : urlOf(r)); } return; } // an embedding page's sidebar leads to the viewer's pages
    if (route.v === "session") clearPaging(route.id);
    resetPagerInput(); stopOpeningEndPin(); navAbort?.abort(); navAbort = null;
    if (r.v === "timeline") { r = { ...r, v: "analytics" }; try { history.replaceState({ ...r, scrollTop: r.scrollTop ?? currentScroll() }, "", urlOf(r)); } catch {} }
    if (r.v === "machines" && NAV_MACHINES) { location.assign(NAV_MACHINES); return; }
    if (!fromHistory) saveHistoryScroll();
    closeAccountMenu(true, true);
    dropErrors(true); // (first: it drops a range the error stepper moved, and that is not kept)
    // Keep the session left after the cached destination has had a frame to draw; weighing it must not delay that draw.
    if (route.v === "session" && (r.v !== "session" || r.id !== route.id) && TX[route.id] && TXM[route.id]) { const sid = route.id, entries = TX[sid], meta = { ...TXM[sid], origin: !!originHandoff(sid) }; requestAnimationFrame(() => requestAnimationFrame(() => setTimeout(() => cacheTx(sid, entries, meta), 0))); }
    if (r.v !== "session" || r.id !== route.id) show = { ...SHOW_ALL };
    if (r.v !== "sessions") ORD.delete("page");
    route = r; find = ""; findOpen = false; closeDrawer(true); clearNewEntries();
    if (!fromHistory) { const state = { ...r }; delete state.scrollTop; try { history.pushState(state, "", urlOf(r)); } catch {} }
    const done = () => {
      if (route !== r) return;
      endLoading(); render(); if (r.v === "session") focusTitle();
      if (fromHistory && Number.isFinite(r.scrollTop)) {
        // A fresh offscreen turn has only its intrinsic estimate. Measure once on history navigation
        // before setting the saved offset, so the browser cannot clamp it to the estimated height.
        const turns = [...$("#page").querySelectorAll(".turn")];
        for (const turn of turns) turn.style.contentVisibility = "visible";
        const heights = turns.map((turn) => turn.getBoundingClientRect().height);
        turns.forEach((turn, i) => { turn.style.containIntrinsicBlockSize = "auto " + Math.ceil(heights[i]) + "px"; turn.style.contentVisibility = ""; });
        restoreScroll(r.scrollTop);
      }
      else if (r.v === "session" && r.turn) { revealTurn(r.turn, !fromHistory); if (location.hash) requestAnimationFrame(() => requestAnimationFrame(revealEntryHash)); }
      else if (r.v === "session" && location.hash) revealEntryHash();
      else if (r.v === "session") openSessionAtEnd();
      else quietTop();
      syncJump();
    };
    // A transcript already in memory, still in TX or kept in the cache, needs no network: the top bar and the sidebar are drawn
    // from the model at once, the page itself on the next frame (so the first two are on screen before a long transcript is
    // built), and it is brought up to date afterwards. Otherwise the route waits for its data, with the top bar and the
    // sidebar already drawn.
    if (r.v === "session" && SESS[r.id]) {
      if (STALE_BRIEFS.has(r.id)) { delete TX[r.id]; delete TXM[r.id]; TXCACHE.delete(r.id); }
      const kept = !TX[r.id];
      if (kept ? adoptCached(r) : !isDeep(r)) {
        TXCACHE.delete(r.id);
        paintPending(r);
        requestAnimationFrame(() => setTimeout(() => { if (route !== r) return; if (kept && !r.turn) spread(r.id); done(); revalidate(r); }, 0));
        return;
      }
    }
    const signal = r.v === "session" ? (navAbort = new AbortController()).signal : undefined, p = load(r, signal);
    if (p) { if (r.v === "session") paintPending(r); p.then(() => { done();  }, (err) => failLoad(r, err)); } else done();
  }
  if (!SIDEBAR_ONLY) window.addEventListener("popstate", (e) => {
    if (skipPop) { skipPop = false; if (afterPop) { const leave = afterPop; afterPop = null; leave(); return; } if (pendingSessionOpen) { const id = pendingSessionOpen; pendingSessionOpen = null; goSession(id); } return; } // close a sheet before opening its session
    if (accountSheet) { accountSheet = false; closeAccountMenu(true); return; } // back gesture closes the phone's account menu
    if (viewerEl) { const d = viewerEl; viewerEl = null; d.close(); return; } // back gesture closes the viewer, page stays
    if (e.state?.v) go(e.state, true); });
  const goSession = (id, turn) => go(turn ? { v: "session", id, turn } : { v: "session", id });
  const goTrace = (turn) => go({ v: "trace", sid: TURN.get(turn).sid, turn });
  const openSender = (h) => { if (SESS[h.from]) goSession(h.from, HOLDS.get(h.id)?.id); };
  // A deep link to a turn: scroll it just below the top bar (measured, since its height varies) and mark it for a moment.
  // It is placed again two frames later, after "Show more" buttons above it have appeared.
  function revealTurn(id, flash) {
    resetPagerInput();
    const b = [...document.querySelectorAll(".turn")].find((x) => x.dataset.turn === id); if (!b) { quietTop(); return; }
    const place = () => { if (!b.isConnected) return; const gap = $("#topbar").offsetHeight + 8;
      scrollProgrammatically(() => { if (phone.matches) window.scrollTo(0, Math.max(0, window.scrollY + b.getBoundingClientRect().top - gap)); else { const m = $("#main"); m.scrollTop += b.getBoundingClientRect().top - m.getBoundingClientRect().top - gap; } });
      syncJump(); saveHistoryScroll(); };
    place(); requestAnimationFrame(() => requestAnimationFrame(place));
    if (flash) { b.classList.add("flash"); setTimeout(() => b.classList.remove("flash"), 1500); }
  }
  function revealEntryHash() {
    resetPagerInput();
    if (!location.hash) return;
    let key = ""; try { key = decodeURIComponent(location.hash.slice(1)); } catch { key = location.hash.slice(1); }
    const target = document.getElementById(key) ?? [...document.querySelectorAll("[data-e]")].find((n) => n.dataset.e === key) ?? [...document.querySelectorAll(".turn[data-turn]")].find((n) => n.dataset.turn === key);
    if (!target) return;
    const place = () => { if (!target.isConnected) return; const gap = $("#topbar").offsetHeight + 8;
      scrollProgrammatically(() => { if (phone.matches) window.scrollTo(0, Math.max(0, window.scrollY + target.getBoundingClientRect().top - gap)); else { const m = $("#main"); m.scrollTop += target.getBoundingClientRect().top - m.getBoundingClientRect().top - gap; } });
      syncJump(); saveHistoryScroll(); };
    place(); requestAnimationFrame(() => requestAnimationFrame(place));
  }

  // ---- Sidebar ----------------------------------------------------------------------------------
  // One typed owner for trigger/menu DOM, open state, focus and dismissal listeners. The viewer owns
  // history, pending model transactions and CSSOM placement; none of those globals enter the library.
  const accountChrome = createAccountChrome({
    place(widget, trigger) {
      const at = trigger.getBoundingClientRect();
      widget.style.setProperty("--account-left", at.left + "px");
      widget.style.setProperty("--account-width", at.width + "px");
      widget.style.setProperty("--account-bottom", Math.max(0, innerHeight - at.top + 6) + "px");
    },
    opened(compact) {
      if (compact) try { history.pushState({ ...route, sheet: 1, scrollTop: currentScroll() }, ""); accountSheet = true; } catch {}
    },
    closed({ keepEntry, navigating }) {
      if (accountSheet) { accountSheet = false; if (!keepEntry && history.state?.sheet) { skipPop = true; history.back(); } }
      if (LIVE.pending && !navigating) setTimeout(() => { if (LIVE.pending && !viewerEl && !accountChrome.open) refresh(); }, 0);
    },
    navigate(href) {
      if (!accountSheet) return false;
      leaveAccountSheet(() => location.assign(href)); return true;
    },
    submit(form) {
      if (!accountSheet) return false;
      leaveAccountSheet(() => form.submit()); return true;
    },
  });
  function closeAccountMenu(keepEntry, navigating) { accountChrome.close({ keepEntry, navigating }); }
  // Step over the phone sheet before leaving so Back lands on the page, not a removed menu.
  function leaveAccountSheet(go) {
    accountSheet = false;
    if (history.state?.sheet) { skipPop = true; afterPop = go; history.back(); } else go();
  }
  function accountWidget(compact) {
    return ACCOUNT ? accountChrome.mount({ account: ACCOUNT, compact, wide: wideMode, onWideChange: () => setWideMode(!wideMode) }) : null;
  }
  function renderDrawerAccount() {
    const old = $("#account-drawer");
    if (old) { accountChrome.unmount(old); old.remove(); }
    if (!ACCOUNT) return;
    const widget = accountWidget(true); widget.id = "account-drawer"; $("#sidebar").append(widget);
  }
  // ---- Stable order: a list of sessions keeps its rows where they are while the reader can see it ------------------------------------
  // Each list sorted by recency keeps its order as a snapshot of session ids. A live update refreshes every row in place and never
  // reorders one: a reorder is held. A session new since the snapshot is put at the top (a pure insertion, like a message arriving)
  // only while the list's top is in view and it is untouched (no pointer down, no mouse over one of its rows, no keyboard focus on
  // one); otherwise it is held too, and so is a list new since the snapshot. Nothing tells the reader what is held. Held order is
  // applied when the list is out of sight, so that no row ever moves while it is looked at:
  //   - the phone's drawer: when it opens (before it shows) and once it has closed;
  //   - the page: when the route changes (the new page is drawn sorted) and when the tab comes back from the background;
  //   - the sidebar on a wide screen, which is always in view: ORD_IDLE_MS after something was first held or the reader last touched it
  //     (a pointer over it, focus in it), once the pointer and focus are off it. The Sessions page has no timer: it is what is read.
  // The list also re-sorts when the reader changes what it is (the search, the filters, the grouping).
  const ORD = new Map(), ordTouch = { down: false }, byLast = (a, b) => b.last - a.last;
  const pageSig = () => JSON.stringify([query, groupBy, sessionFilters]);
  document.addEventListener("pointerdown", () => { ordTouch.down = true; }, true);
  for (const t of ["pointerup", "pointercancel"]) document.addEventListener(t, () => { ordTouch.down = false; }, true);
  window.addEventListener("blur", () => { ordTouch.down = false; });
  // A tab that comes back from the background applies what was held, once, before it paints, from the data it has. What the catch-up
  // poll brings after that is held like any other update.
  document.addEventListener("visibilitychange", () => { ordTouch.down = false; if (visible()) { orderApply("page"); orderApply("side"); } });
  // How many rows must move to put `rows` in `cmp` order: all of them but the longest run already in it.
  function ordMoves(rows, cmp) {
    const tails = [];
    for (const x of rows) {
      let lo = 0, hi = tails.length;
      while (lo < hi) { const mid = (lo + hi) >> 1; if (cmp(tails[mid], x) > 0) hi = mid; else lo = mid + 1; }
      tails[lo] = x;
    }
    return rows.length - tails.length;
  }
  // Whether the reader could be using a screen's list: is its top in view, is it touched. Read before a draw empties the list (an
  // emptied scroller clamps to 0 and the rows and their hover are gone). Focus counts only when it is keyboard focus on a row of this
  // list, a mouse only over one of its rows.
  function ordState(name) {
    const side = name === "side", rows = side ? "#lanes .treeitem" : "#page .nrow", a = document.activeElement;
    const inView = side ? sideRegion().scrollTop <= 1 : !$(rows) || $(rows).getBoundingClientRect().top >= $("#topbar").getBoundingClientRect().bottom - 1;
    const touched = ordTouch.down || (!!a?.matches?.(":focus-visible") && !!a.closest(side ? "#lanes" : rows)) || (matchMedia("(hover: hover)").matches && !!$(rows + ":hover"));
    return { inView, touched };
  }
  let ordPageState = null;
  const pageState = () => ordPageState ?? ordState("page");
  // One draw of a screen's lists. `sig` is what the reader chose to show, `tie` the route it belongs to (null: the sidebar, which
  // outlives routes); a draw with another sig or tie starts from the sorted order. `st` is ordState from before the draw.
  function orderScope(name, sig, tie, st) {
    const prev = ORD.get(name), keep = !!prev && prev.sig === sig && prev.tie === tie;
    const scope = { sig, tie, keep, calm: st.inView && !st.touched, // calm: a new session may be put at the top
      prev: keep ? prev.lists : new Map(), prevKids: keep ? prev.kids : new Set(), kids: new Set(), prevExp: keep ? prev.exp : null, exp: null, reseed: false, lists: new Map(), n: 0 };
    ORD.set(name, scope); return scope;
  }
  // The rows of one list, in snapshot order, with a new session put first when `scope.calm` and held otherwise. `must` names ids that
  // are never held (the open session and what leads to it); `limit`
  // is how many rows the screen shows, so a session moving below them is not news; `quiet` is a list nobody sees (a collapsed
  // parent's children), drawn sorted and not counted; `seed` is a list the reader just opened, drawn sorted. Any other list with no
  // snapshot on a screen that has one is an empty snapshot.
  function orderList(scope, key, items, cmp, { must = null, limit = Infinity, quiet = false, seed = false } = {}) {
    const sorted = [...items].sort(cmp), by = new Map(items.map((s) => [s.id, s]));
    let ids = sorted.map((s) => s.id);
    if (scope.keep && !quiet && !(seed && !scope.prev.has(key))) {
      const existed = scope.prev.has(key), old = (scope.prev.get(key) ?? []).filter((id) => by.has(id)), have = new Set(old);
      const fresh = sorted.filter((s) => !have.has(s.id)), up = fresh.filter((s) => must?.has(s.id) || (scope.calm && existed));
      ids = [...up.map((s) => s.id), ...old];
      const shown = ids.slice(0, limit), inShown = new Set(shown);
      const top = new Set(sorted.slice(0, limit).map((s) => s.id)), heldNew = fresh.filter((s) => top.has(s.id) && !up.includes(s)).length;
      scope.n += heldNew + sorted.slice(0, limit).filter((s) => have.has(s.id) && !inShown.has(s.id)).length + ordMoves(shown.map((id) => by.get(id)), cmp);
    }
    scope.lists.set(key, ids);
    return ids.map((id) => by.get(id));
  }
  const sideRegion = () => $("#side-list") ?? $("#sidebar");
  // How long the wide screen's sidebar is left alone before it applies what it holds. Read once, at load, with 10 s as the default; a
  // browser check may set window.__semonOrderIdleMs to a finite number from 200 to 60000 ms before load; other values use the default.
  const ordIdleMs = window.__semonOrderIdleMs;
  const ORD_IDLE_MS = Number.isFinite(ordIdleMs) && ordIdleMs >= 200 && ordIdleMs <= 60000 ? ordIdleMs : 10000;
  const ORD_DRAWER_MS = 320; // the drawer's slide (0.24 s) and a little
  // Applies what a screen holds: the list is drawn sorted, from scratch.
  function orderApply(name) {
    const sc = ORD.get(name); if (!sc?.n || (name === "page" && (sc.tie !== route || rendered !== route || viewerEl || $("#page").hasAttribute("aria-busy")))) return;
    ORD.delete(name);
    if (name === "side") renderLanes(); else { const st = capture(); render(); restore(st); }
  }
  // The wide screen's sidebar is always in view: what it holds is applied once it has been left alone for ORD_IDLE_MS. Left alone means
  // no pointer over it or down, no keyboard focus or focused text field in it, and no menu or dialog open (the session menu hangs from
  // the top bar, outside the sidebar, so it is named here); anything the reader does to it starts the wait again.
  let ordIdle = null;
  function ordIdleArm() {
    clearTimeout(ordIdle); ordIdle = null;
    if (phone.matches || !ORD.get("side")?.n) return;
    ordIdle = setTimeout(() => {
      ordIdle = null; const bar = $("#sidebar"), a = document.activeElement;
      const menu = $(".session-menu, .account-popover, .runs-popover, .filters.pop:not([hidden])");
      if (ordTouch.down || bar.matches(":hover") || (bar.contains(a) && (a.matches(":focus-visible") || a.matches("input, textarea, select, [contenteditable]"))) || menu || viewerEl) ordIdleArm(); else orderApply("side");
    }, ORD_IDLE_MS);
  }
  for (const t of ["pointermove", "pointerdown", "pointerleave", "focusin", "focusout", "wheel", "keydown"]) $("#sidebar").addEventListener(t, () => { if (ordIdle) ordIdleArm(); }, { passive: true });
  function renderNav() {
    const nav = $("#nav"); nav.replaceChildren();
    // A session or a trace sits under Sessions, a machine under Machines.
    const under = { home: ["home"], analytics: ["analytics"], sessions: ["sessions", "session", "trace"], machines: ["machines", "machine"] };
    const item = (v, label, ic, count, hot) => { const b = el("button", "nav-item"); b.type = "button"; b.dataset.go = v; if (under[v].includes(route.v)) b.setAttribute("aria-current", "page"); b.append(icon(ic, "icon"), el("span", null, label)); if (count) b.append(el("span", "cnt" + (hot ? " hot" : ""), String(count))); b.addEventListener("click", () => go({ v })); nav.append(b); };
    item("home", "Home", I.home, inbox().length, true);
    item("sessions", "Sessions", I.sessions);
    item("analytics", "Analytics", I.chart);
    item("machines", "Machines", I.machine, Object.keys(MACHINE).filter((m) => !MACHINE_UP[m]).length, true);
  }
  // Sessions match by name, repo, branch, machine, harness and the messages that started their turns.
  const sessMatch = (s, q) => !q || [s.name, s.repo, s.branch, MACHINE[s.machine], s.movedFrom ? MACHINE[s.movedFrom] : "", HARNESS[s.harness], s.role ? "role no repo" : "", ...(TURNS[s.id] ?? []).map((t) => t.start?.brief ?? t.u?.text ?? "")].join(" ").toLowerCase().includes(q.toLowerCase());
  // Built once per model and per render (both drop it) and shared: callers copy an array before reordering it.
  let CHILDREN = null;
  const sessionChildren = () => {
    if (CHILDREN) return CHILDREN;
    const children = new Map();
    for (const s of Object.values(SESS)) { const parent = parentOf(s.id); if (parent && SESS[parent]) { if (!children.has(parent)) children.set(parent, []); children.get(parent).push(s); } }
    for (const xs of children.values()) xs.sort((a, b) => b.last - a.last);
    return (CHILDREN = children);
  };
  const isApprovalReview = (s) => s.kind === "Approval review";
  const visibleInNavigation = (s, path = routedPath()) => showApprovalReviews || !isApprovalReview(s) || s.id === path.current || path.ancestors.has(s.id);
  // Hide review rows while forwarding their visible descendants to the nearest shown parent; roots are promoted the same way.
  const navigationTree = (path = routedPath()) => {
    const canonical = sessionChildren(), roots = [], children = new Map(), visited = new Set();
    const visit = (session, parent) => {
      if (visited.has(session.id)) return;
      visited.add(session.id);
      let nearest = parent;
      if (visibleInNavigation(session, path)) {
        if (parent) { if (!children.has(parent)) children.set(parent, []); children.get(parent).push(session); }
        else roots.push(session);
        nearest = session.id;
      }
      for (const child of canonical.get(session.id) ?? []) visit(child, nearest);
    };
    for (const root of Object.values(SESS).filter((s) => s.lane && !parentOf(s.id))) visit(root, null);
    roots.sort(byLast);
    for (const kids of children.values()) kids.sort(byLast);
    return { roots, children };
  };
  // Children in the order their handoffs were sent; the sidebar list stays newest-first.
  const childSessions = (sid) => [...(sessionChildren().get(sid) ?? [])].sort((a, b) => (originHandoff(a.id)?.at ?? a.last) - (originHandoff(b.id)?.at ?? b.last));
  const descendantsOf = (sid, children, out = [], seen = new Set([sid])) => {
    for (const child of children.get(sid) ?? []) if (!seen.has(child.id)) { seen.add(child.id); out.push(child); descendantsOf(child.id, children, out, seen); }
    return out;
  };
  const TOTAL_TOKEN_KINDS = ["input", "output", "cache_write", "cache_read"];
  const TOKEN_KINDS = [["input", "Input"], ["output", "Output"], ["cache_read", "Cache read"], ["cache_write_5m", "Cache write · 5m"], ["cache_write_1h", "Cache write · 1h"], ["web_search", "Web search"]];
  const asMoney = (usd) => "$" + usd.toFixed(2), shortMoney = (usd) => "$" + usd.toFixed(1);
  const usageTotal = (s) => Object.values(s.tokens_by_model ?? {}).reduce((sum, usage) => sum + TOTAL_TOKEN_KINDS.reduce((n, key) => n + (Number(usage[key]) || 0), 0), 0);
  function costForSessions(sessions) {
    const total = { usd: 0, unpriced_models: [], split_unknown_messages: 0, by_model: {}, by_day: {} }, unpriced = new Set(); let allPriced = true;
    for (const s of sessions) {
      const cost = s.cost ?? {};
      if (cost.usd == null) allPriced = false; else total.usd += Number(cost.usd) || 0;
      for (const model of cost.unpriced_models ?? []) unpriced.add(model);
      total.split_unknown_messages += Number(cost.split_unknown_messages) || 0;
      for (const [day, amount] of Object.entries(cost.by_day ?? {})) total.by_day[day] = (total.by_day[day] ?? 0) + (Number(amount) || 0);
      for (const [modelId, model] of Object.entries(cost.by_model ?? {})) {
        const current = total.by_model[modelId] ?? { usd: 0, tokens: {}, usd_by_kind: {} };
        if (model.usd == null) current.usd = null; else if (current.usd != null) current.usd += Number(model.usd) || 0;
        for (const [key, amount] of Object.entries(model.tokens ?? {})) current.tokens[key] = (current.tokens[key] ?? 0) + (Number(amount) || 0);
        for (const [key, amount] of Object.entries(model.usd_by_kind ?? {})) current.usd_by_kind[key] = (current.usd_by_kind[key] ?? 0) + (Number(amount) || 0);
        total.by_model[modelId] = current;
      }
    }
    total.unpriced_models = [...unpriced].sort();
    if (!allPriced || unpriced.size) total.usd = null;
    return total;
  }
  const costForSession = (sid, includeRuns = false) => costForSessions(SESS[sid] ? [SESS[sid], ...(includeRuns ? descendantsOf(sid, sessionChildren()) : [])] : []);
  const costText = (cost) => cost.usd == null || cost.unpriced_models?.length ? "—" : asMoney(cost.usd);
  const costMissing = (cost) => cost.unpriced_models ?? [];
  const COST_TIP = "What these tokens would cost at API rates. Subscriptions (Claude Max, ChatGPT plans) aren't billed this way.";
  // The icon is the only place this text is; it takes keyboard focus so the tip is reachable without a pointer.
  let metricSeq = 0;
  function costInfoTip() { const b = el("span", "cost-info"); b.dataset.tip = COST_TIP; b.tabIndex = 0; b.setAttribute("role", "img"); b.setAttribute("aria-label", COST_TIP); b.append(icon(I.q)); return b; }
  const TREE_RANK = { wait: 0, work: 1, err: 2, idle: 3, done: 4 };
  const urgentDescendant = (sid, children) => descendantsOf(sid, children).filter((s) => s.state in TREE_RANK).sort((a, b) => TREE_RANK[a.state] - TREE_RANK[b.state] || b.last - a.last)[0]?.state;
  // What a parent's descendants are doing, as parts to join: the runs, then only the non-zero needs-you, working and failed counts (the viewer's own state words).
  const childParts = (all) => { const n = (state) => all.filter((x) => x.state === state).length, wait = n("wait"), work = n("work"), err = n("err"); return [all.length + (all.length === 1 ? " run" : " runs"), wait && wait + " needs you", work && work + " working", err && err + " failed"].filter(Boolean); };
  const defaultTreeOpen = (sid, children) => descendantsOf(sid, children).some((s) => s.state === "wait" || s.state === "work");
  // An open parent lists its waiting children, then its running ones (at most 8), then the newest finished ones until three rows are
  // listed. "All N" opens the rest: a sheet on a phone, the whole list in the tree on a wide screen.
  const TREE_ACTIVE = 8, TREE_ROWS = 3;
  // The open session and the sessions above it. Only the open one is marked current; its ancestors are opened in the tree for this render
  // (nothing is saved) and are always listed, so the current row can always be found.
  function routedPath() {
    const current = route.v === "session" ? route.id : route.v === "trace" ? route.sid : null, ancestors = new Set();
    for (let id = current && SESS[current] ? parentOf(current) : null; id && SESS[id] && id !== current && !ancestors.has(id); id = parentOf(id)) ancestors.add(id);
    return { current, ancestors };
  }
  // Those ancestors open once per navigation, held in memory: a parent collapsed after that stays collapsed until the next one.
  let forcedOpen = { route: null, ids: new Set() };
  function forcedOpenIds() { if (forcedOpen.route !== route) forcedOpen = { route, ids: routedPath().ancestors }; return forcedOpen.ids; }
  // Waiting is 0, running 1, finished 2. A finished child with a waiting or running session below it ranks as that session does.
  function kidRank(c, children) {
    let rank = c.state === "wait" ? 0 : c.state === "work" ? 1 : 2;
    if (rank) for (const d of descendantsOf(c.id, children)) { if (d.state === "wait") return 0; if (d.state === "work") rank = 1; }
    return rank;
  }
  // The one parent whose whole list is open in the tree (wide screens only). Nothing saves it: a reload starts with the short lists.
  // The parents above it stay listed and open (`expandedPath`) and everything below it is listed in full and open (`expandedUnder`), so
  // its "All N" and the rows it reveals agree.
  let expandedAll = null, revealedFor = null, expandedPath = new Set(), expandedUnder = new Set(), sideOrder = null;
  const ancestorsOf = (id) => { const out = new Set(); for (let p = id && SESS[id] ? parentOf(id) : null; p && SESS[p] && p !== id && !out.has(p); p = parentOf(p)) out.add(p); return out; };
  // Fills a parent's group and says whether it holds the parent's whole list, which sticks the parent's row (stickRow).
  function treeGroupFill(group, parent, kids, children, depth, rail, open) {
    const { current, ancestors } = routedPath(), rank = new Map(kids.map((c) => [c.id, kidRank(c, children)]));
    const byRank = (a, b) => rank.get(a.id) - rank.get(b.id) || b.last - a.last, sorted = [...kids].sort(byRank), keep = new Set();
    for (const c of sorted) if (rank.get(c.id) < 2 && keep.size < TREE_ACTIVE) keep.add(c.id);
    for (const c of sorted) if (c.id === current || ancestors.has(c.id) || expandedPath.has(c.id)) keep.add(c.id);
    for (const c of sorted) if (keep.size < TREE_ROWS) keep.add(c.id);
    const listed = sorted.filter((c) => keep.has(c.id)), hidden = kids.length - listed.length;
    if (!hidden && expandedAll === parent.id) expandedAll = null; // nothing is left to open: "Show fewer" would have nothing to fold
    const full = hidden > 0 && !rail && (expandedAll === parent.id || expandedUnder.has(parent.id));
    group.replaceChildren();
    // The rows keep the order they had (see "Stable order"); a child new to the list is held, unless it is the open session or leads to it.
    // A collapsed parent's children are nobody's to see: drawn sorted, not counted. A list new to a screen that has a snapshot is held
    // whole, with no "All N" row either, so that nothing appears in the tree.
    const key = (full ? "a:" : "k:") + parent.id, unseen = !!sideOrder?.keep && open && !full && !sideOrder.reseed && sideOrder.seen.has(parent.id) && !sideOrder.prev.has(key);
    const shown = sideOrder ? orderList(sideOrder, key, full ? sorted : listed, byRank, { must: new Set([current, ...ancestors, ...expandedPath]), quiet: !open, seed: full || sideOrder.reseed || !sideOrder.seen.has(parent.id) }) : full ? sorted : listed;
    for (const child of shown) group.append(buildLaneItem(child, depth + 1, children, rail));
    if (kids.length === shown.length || full || unseen) return full && expandedAll === parent.id;
    const total = descendantsOf(parent.id, children).length, button = el("button", "tree-all");
    button.type = "button"; button.dataset.id = parent.id; button.setAttribute("role", "treeitem"); if (phone.matches) button.setAttribute("aria-haspopup", "dialog");
    button.setAttribute("aria-label", "All " + total + " sessions under " + parent.name);
    button.append(el("span", null, "All " + total), icon(I.chev));
    button.addEventListener("click", (e) => {
      e.stopPropagation();
      if (phone.matches) { openKidsSheet(parent, button); return; }
      expandedAll = parent.id; renderLanes();
      $('#lanes .treeitem[data-id="' + CSS.escape(parent.id) + '"] > .tree-row .tree-fewer')?.focus();
    });
    group.append(button);
    return false;
  }
  // While a parent's whole list is open its row sticks to the top of the sidebar and carries the control that folds the list again,
  // so folding never needs a scroll: the row is scrolled back into view and keeps focus.
  function stickRow(line, s) {
    line.classList.add("stuck");
    const fewer = el("button", "tree-fewer"); fewer.type = "button"; fewer.setAttribute("aria-label", "Show fewer sessions under " + s.name); fewer.append(el("span", null, "Show fewer"), icon(I.chev));
    fewer.addEventListener("click", (e) => {
      e.stopPropagation(); expandedAll = null; renderLanes();
      const row = $('#lanes .srow[data-id="' + CSS.escape(s.id) + '"]'); scrollProgrammatically(() => row?.scrollIntoView({ block: "nearest" })); row?.focus({ preventScroll: true });
    });
    line.append(fewer);
  }
  // A phone's "All N": every session below the parent in one sheet, waiting first, then running, then finished, newest first in each.
  function openKidsSheet(parent, trigger) {
    const all = descendantsOf(parent.id, navigationTree().children), bucket = (s) => s.state === "wait" ? 0 : s.state === "work" ? 1 : 2;
    const d = el("dialog", "viewer kids-sheet"), head = el("div", "vh"), title = el("div", "vt"), close = el("button", "vclose");
    d.setAttribute("aria-label", "All sessions under " + parent.name);
    title.append(el("span", null, parent.name)); close.type = "button"; close.setAttribute("aria-label", "Close"); close.append(icon(I.x)); close.addEventListener("click", () => d.close());
    head.append(title, el("div", "vm", all.length + (all.length === 1 ? " session" : " sessions")), close);
    const search = el("label", "kids-search"), input = el("input"), body = el("div", "vb"), list = el("div", "kids-list"); let picked = null;
    input.type = "search"; input.placeholder = "Search these sessions"; input.setAttribute("aria-label", "Search these sessions"); search.append(icon(I.search), input); body.append(list);
    const draw = () => {
      list.replaceChildren(); const q = input.value.trim(), rows = all.filter((s) => sessMatch(s, q));
      if (!rows.length) list.append(el("p", "empty", "No sessions match “" + q + "”."));
      ["Waiting for you", "Running", "Finished"].forEach((label, i) => {
        const xs = rows.filter((s) => bucket(s) === i).sort((a, b) => b.last - a.last); if (!xs.length) return;
        const sec = el("section", "kids-sec"); sec.append(el("h3", "kids-h", label + " (" + xs.length + ")"));
        for (const s of xs) {
          const row = el("button", "kids-row"); row.type = "button"; row.dataset.id = s.id; row.setAttribute("aria-label", s.name + ", " + (STATE[s.state] ?? s.state));
          row.append(dot(s.state), el("span", "nm", s.name));
          const above = parentOf(s.id); if (above && above !== parent.id && SESS[above]) row.append(el("span", "under", "under " + SESS[above].name));
          row.append(el("span", "ag", ago(s.last)));
          row.addEventListener("click", () => { picked = s.id; pendingSessionOpen = s.id; d.close(); }); sec.append(row);
        }
        list.append(sec);
      });
    };
    input.addEventListener("input", draw); d.append(head, search, body); document.body.append(d); draw();
    d.addEventListener("click", (ev) => { if (ev.target === d) d.close(); });
    d.addEventListener("close", () => {
      d.remove(); document.documentElement.classList.remove("viewer-open");
      if (viewerEl === d) { viewerEl = null; if (!SIDEBAR_ONLY && history.state?.sheet) { skipPop = true; history.back(); } else if (pendingSessionOpen) { const id = pendingSessionOpen; pendingSessionOpen = null; goSession(id); } }
      if (!picked) ($('#lanes .tree-all[data-id="' + CSS.escape(parent.id) + '"]') ?? trigger).focus();
      if (LIVE.pending) refresh();
    });
    viewerEl = d; document.documentElement.classList.add("viewer-open"); d.showModal(); close.focus({ focusVisible: false }); if (!SIDEBAR_ONLY) try { history.pushState({ ...route, sheet: 1, scrollTop: currentScroll() }, ""); } catch {} // an embedding page's history is its own
  }
  // The sidebar keeps the 8 most recently active top-level sessions, with children nested beneath their parent.
  // Sessions and the sidebar share the same two-line row. Metadata disappears as whole values,
  // from the end, so the harness and model remain readable even in the narrow drawer.
  function sessionRow(s, { density = "list", rail = false } = {}) {
    const row = el("button", density === "compact" ? "srow" : "nrow"); row.type = "button"; row.dataset.id = s.id; row.dataset.sessionRow = density;
    const main = el("span", "session-row-main srow-main"), nm = el("span", "nm", s.name), ag = el("span", "ag", ago(s.last)); nm.dataset.tip = s.name; nm.dataset.tipClipped = "";
    main.append(dot(s.state, !rail), nm, ag);
    const meta = el("span", "session-row-meta srow-meta for");
    const mark = harnessIcon(s.harness, { size: 14, label: true }); if (mark) meta.append(mark);
    const model = el("span", "row-model", shortModel(s.model ?? s.modelId)); model.dataset.tip = "Model: " + modelIdOf(s); meta.append(model);
    if (density === "list" && parentOf(s.id)) { const delegation = icon(I.spawn ?? I.stack); delegation.classList.add("row-delegation"); delegation.dataset.tip = "Delegated session"; meta.append(delegation); }
    const add = (cls, text, priority, tip, glyph) => { const value = el("span", cls); value.dataset.drop = priority; if (tip) value.dataset.tip = tip; if (glyph) { value.classList.add("row-field"); value.append(icon(glyph), el("span", "field-value", text)); } else value.textContent = text; meta.append(value); };
    add("row-duration", dur(s.start, s.state === "work" || s.state === "wait" ? null : s.last), 1, "Duration", I.duration);
    if (Object.keys(MACHINE).length > 1) add("row-machine host", shortHost(s), 2, "Machine: " + hostOf(s), I.machine);
    if (s.repo) add("repo-short", s.repo, 3, "Repo: " + s.repo, I.repo);
    const counts = density === "list" ? childRuns(s.id) : ""; if (counts) add("row-counts", counts.replace(/Codex runs/g, "runs"), 4);
    row.append(main, meta); row.addEventListener("click", () => goSession(s.id));
    requestAnimationFrame(() => fitSessionRowMeta(meta)); return row;
  }
  function fitSessionRowMeta(meta) {
    if (!meta.isConnected || !meta.clientWidth) return;
    const compact = meta.closest('[data-session-row="compact"]'), values = [...meta.querySelectorAll("[data-drop]")];
    for (const value of values) value.hidden = !!compact && meta.clientWidth < 280 && value.classList.contains("row-duration");
    for (const value of values.sort((a, b) => Number(b.dataset.drop) - Number(a.dataset.drop))) { if (meta.scrollWidth <= meta.clientWidth + 1) break; value.hidden = true; }
  }
  window.addEventListener("resize", () => { for (const meta of document.querySelectorAll(".session-row-meta")) fitSessionRowMeta(meta); }, { passive: true });
  function buildLaneItem(s, depth, children, rail) {
    const kids = children.get(s.id) ?? [], allKids = descendantsOf(s.id, children), item = el("div", "treeitem");
    item.dataset.id = s.id; item.setAttribute("role", "treeitem"); item.setAttribute("aria-label", s.name); item.tabIndex = 0;
    const { current, ancestors } = routedPath(), saved = treePrefs[s.id];
    const open = forcedOpenIds().has(s.id) || expandedPath.has(s.id) || (typeof saved?.open === "boolean" ? saved.open : defaultTreeOpen(s.id, children) || expandedUnder.has(s.id));
    if (kids.length && !rail) item.setAttribute("aria-expanded", String(open));
    const line = el("div", "tree-row");
    let lineToggle = null;
    if (kids.length && !rail) {
      const toggle = el("button", "tree-toggle"); toggle.type = "button"; toggle.dataset.treeToggle = s.id; toggle.setAttribute("aria-label", (open ? "Collapse " : "Expand ") + s.name); toggle.setAttribute("aria-expanded", String(open)); toggle.append(icon(I.chev));
      toggle.addEventListener("click", (e) => { e.stopPropagation(); const value = item.getAttribute("aria-expanded") !== "true"; item.setAttribute("aria-expanded", String(value)); toggle.setAttribute("aria-expanded", String(value)); toggle.setAttribute("aria-label", (value ? "Collapse " : "Expand ") + s.name); if (!value) forcedOpenIds().delete(s.id); saveTreePref(s.id, value); if (!value && (expandedAll === s.id || expandedPath.has(s.id))) { expandedAll = null; renderLanes(); $('#lanes .treeitem[data-id="' + CSS.escape(s.id) + '"] > .tree-row .tree-toggle')?.focus(); } });
      lineToggle = toggle; line.classList.add("has-toggle");
    }
    const row = sessionRow(s, { density: "compact", rail }); if (rail) row.dataset.tip = s.name; // the collapsed rail shows only a dot; otherwise the name has a tip while it is cut off
    row.setAttribute("aria-label", s.name + ", " + (STATE[s.state] ?? s.state) + ", " + (HARNESS[s.harness] ?? s.harness) + ", " + shortHost(s));
    const parts = allKids.length ? childParts(allKids) : []; if (parts.length) row.setAttribute("aria-label", row.getAttribute("aria-label") + ", " + parts.join(", "));
    if (current === s.id) row.setAttribute("aria-current", "page");
    if (rail && ancestors.has(s.id)) { row.classList.add("on-path"); row.setAttribute("aria-current", "true"); }
    const main = row.querySelector(".session-row-main"), ag = main.querySelector(".ag");
    if (rail && allKids.some((x) => x.state === "work" || x.state === "wait")) { const childDot = dot(urgentDescendant(s.id, children) ?? "work", false); childDot.classList.add("child-dot"); childDot.setAttribute("aria-hidden", "true"); main.append(childDot); }
    // A parent's row (open or collapsed) shows a small dot beside its time only when a session under it needs you (amber) or failed (red); the label says which, so the dot is decorative.
    const flag = kids.length && !rail ? (allKids.some((x) => x.state === "wait") ? "wait" : allKids.some((x) => x.state === "err") ? "err" : null) : null;
    if (flag) { const f = el("span", "kid-flag " + flag); f.dataset.tip = allKids.filter((x) => x.state === "wait").length + " needs you · " + allKids.filter((x) => x.state === "err").length + " failed"; f.setAttribute("aria-hidden", "true"); ag.before(f); }
    line.append(row); if (lineToggle) line.append(lineToggle); item.append(line);
    item.addEventListener("keydown", (e) => {
      if (kids.length && !rail && (e.key === "ArrowLeft" || e.key === "ArrowRight")) { if (e.target !== item && e.target !== row && e.target !== lineToggle) return; const next = e.key === "ArrowRight"; if ((item.getAttribute("aria-expanded") === "true") !== next) { e.preventDefault(); item.querySelector(":scope > .tree-row .tree-toggle")?.click(); } }
      else if ((e.key === "Enter" || e.key === " ") && e.target === item) { e.preventDefault(); goSession(s.id); }
    });
    if (kids.length && !rail) { const group = el("div", "tree-group"); group.dataset.depth = String(Math.min(depth + 1, 4)); group.setAttribute("role", "group"); group.setAttribute("aria-label", "Sessions spawned by " + s.name); const full = treeGroupFill(group, s, kids, children, depth, rail, open); item.append(group); if (full && open) stickRow(line, s); }
    return item;
  }
  // The focused control in the tree, so a redraw (a live update, a fold) can put focus back on it or, failing that, on its parent's row.
  function laneFocus() {
    const a = document.activeElement, item = a?.closest?.(".treeitem");
    if (!a || !$("#lanes").contains(a)) return null;
    const kind = ["srow", "tree-all", "tree-fewer", "tree-toggle"].find((c) => a.classList.contains(c)) ?? (a === item ? "treeitem" : null);
    const id = kind === "tree-toggle" ? a.dataset.treeToggle : kind === "srow" || kind === "tree-all" ? a.dataset.id : item?.dataset.id;
    return kind && id ? { kind, id, visible: a.matches(":focus-visible") } : null;
  }
  function restoreLaneFocus(f) {
    if (!f || document.activeElement !== document.body) return;
    const q = (sel) => $("#lanes " + sel), id = CSS.escape(f.id);
    const target = f.kind === "srow" ? q('.srow[data-id="' + id + '"]') : f.kind === "tree-all" ? q('.tree-all[data-id="' + id + '"]') : f.kind === "tree-fewer" ? q('.treeitem[data-id="' + id + '"] > .tree-row .tree-fewer')
      : f.kind === "tree-toggle" ? q('.tree-toggle[data-tree-toggle="' + id + '"]') : q('.treeitem[data-id="' + id + '"]');
    (target ?? q('.srow[data-id="' + id + '"]'))?.focus({ preventScroll: true, focusVisible: f.visible });
  }
  function renderLanes() {
    const rail = railMode && !phone.matches, prevSide = ORD.get("side");
    if (rail || prevSide?.rail !== rail) ORD.delete("side"); // the rail draws sorted, and so does a change into or out of it
    const sideState = ordState("side"), focus = laneFocus(), path = routedPath(), { current, ancestors } = path, { roots: lanes, children: everyone } = navigationTree(path);
    sideOrder = orderScope("side", "", null, sideState); sideOrder.rail = rail;
    // The rows drawn now, before the list is emptied: which parents the reader can see, and which sessions were drawn at all.
    const box = $("#lanes"), drawn = [...box.querySelectorAll(".treeitem")];
    sideOrder.seen = new Set(drawn.map((r) => r.dataset.id)); const seeing = new Set(drawn.filter((r) => r.querySelector(":scope > .tree-row .srow")?.getClientRects().length).map((r) => r.dataset.id));
    // A session the reader can see, with no children in the last draw, whose first child arrives while the tree is held, stays as it
    // was: a toggle and a count on its row would move the rows below it. Its new children are held (and counted) as a whole. A parent
    // nobody sees, or one just put in the tree, shows its children with it.
    let children = everyone;
    if (sideOrder.keep) {
      const held = [...everyone.keys()].filter((p) => !sideOrder.prevKids.has(p) && seeing.has(p) && p !== current && !ancestors.has(p));
      if (held.length) { children = new Map([...everyone].filter(([p]) => !held.includes(p))); sideOrder.n += held.reduce((n, p) => n + everyone.get(p).length, 0); }
    }
    sideOrder.kids = new Set(children.keys());
    if (expandedAll && (phone.matches || railMode || !SESS[expandedAll])) expandedAll = null;
    // Opening or folding a parent's whole list, or crossing into or out of the rail, changes which lists are drawn: those the reader
    // just brought back are drawn sorted, not held as new.
    sideOrder.exp = JSON.stringify([expandedAll, railMode && !phone.matches]); sideOrder.reseed = sideOrder.keep && sideOrder.prevExp !== sideOrder.exp;
    expandedPath = expandedAll ? ancestorsOf(expandedAll) : new Set(); expandedUnder = expandedAll ? new Set(descendantsOf(expandedAll, children).map((x) => x.id)) : new Set();
    box.replaceChildren();
    for (const s of orderList(sideOrder, "lanes", lanes, byLast, { limit: 8, must: new Set([current, ...ancestors]) }).slice(0, 8)) box.append(buildLaneItem(s, 0, children, railMode && !phone.matches));
    if (!sideOrder.n) { clearTimeout(ordIdle); ordIdle = null; } else if (!ordIdle) ordIdleArm(); // (counted from when something was first held)
    if (!lanes.length) { const empty = el("p", "ghead", "No sessions match"); empty.setAttribute("role", "none"); box.append(empty); }
    const q = $("#q"); if (q && document.activeElement !== q) q.value = query;
    restoreLaneFocus(focus);
    // A stuck row covers the top of the sidebar: what is scrolled into view (the open session, after a navigation) stays clear of it.
    const stuck = box.querySelector(".tree-row.stuck"), navigated = revealedFor !== route; revealedFor = route;
    ($("#side-list") ?? $("#sidebar")).style.scrollPaddingTop = stuck ? stuck.offsetHeight + 8 + "px" : "";
    if (stuck && navigated) scrollProgrammatically(() => box.querySelector('.srow[aria-current="page"]')?.scrollIntoView({ block: "nearest" }));
  }

  // ---- Top bar ---------------------------------------------------------------------------------------
  // The same on every page: the menu button (phones), the title, and at most two actions. A detail page adds a crumb up a
  // level and a second line of labels. Labels are information, never a control: their full values are in the tooltip and in
  // the session menu. On a session, Find takes over the bar and the filters sit under it as chips.
  const btn = (cls, text, label) => { const b = el("button", cls, text); b.type = "button"; if (label) b.setAttribute("aria-label", label); return b; };
  const kindText = (s) => s.kind ?? HARNESS[s.harness];
  // The model the session is on (what the line's label abbreviates), so the label, its tip and the Details row name the same one; a session that used more than one is priced per model in the cost section.
  const modelIdOf = (s) => s.model ?? Object.keys(s.tokens_by_model ?? {})[0];
  function renderTopbar(title, crumb, opts = {}) {
    closeAccountMenu(); // the bar is redrawn from scratch, the desktop menu with it: close it properly, not by detaching it
    document.querySelectorAll("#topbar .account-widget").forEach((root) => accountChrome.unmount(root));
    const bar = $("#topbar"), s = opts.session ?? opts.traceSession; clearBox(bar, route); bar.classList.remove("scrolled"); bar.classList.toggle("session-bar", !!s);
    // What the bar holds is added through `put`, so the range control on Analytics (a persistent control) stays where it is.
    const put = placer(bar), sink = { append: put };
    const account = (into) => { const a = accountWidget(false); if (a) (into ? into.append(a) : put(a)); };
    if (s && errOn(s.id)) { errorsBar(sink); account(); put.done(); return; }
    if (s && findOpen) { findBar(sink, s, account); put.done(); return; }
    const m = btn("ibtn lead", null, opts.traceSession ? "Back to " + s.name : "Open navigation"); m.id = "lead-btn"; m.setAttribute("aria-controls", "sidebar"); m.setAttribute("aria-expanded", "false"); m.append(icon(opts.traceSession ? I.chev : I.menu)); if (opts.traceSession) m.classList.add("trace-back"); m.addEventListener("click", opts.traceSession ? () => goSession(s.id, route.turn) : openDrawer); put(m);
    const t = el("div", "ttl"), l1 = el("div", "l1");
    // Ancestors are crumbs on a desktop; a phone's child session has none (its ⋯ menu lists the path, and holds the status the dot would show).
    if (opts.lineage?.length) { if (!phone.matches) opts.lineage.forEach((item) => { const c = btn("crumb", item.name, "Open " + item.name); c.addEventListener("click", () => goSession(item.id)); l1.append(c, el("span", "crumb-sep", "›")); }); }
    else if (crumb) { const c = btn("crumb", crumb.label, "Back to " + crumb.label); c.addEventListener("click", crumb.go); l1.append(c, el("span", "crumb-sep", "›")); }
    const tt = el("span", "t", title); tt.dataset.tip = title; tt.dataset.tipClipped = ""; if (s && !opts.traceSession && !(phone.matches && opts.lineage?.length)) l1.append(stateLead(s)); l1.append(tt); t.append(l1);
    if (opts.line2) { const l2 = el("div", "meta-line"); opts.line2(l2); t.append(l2); if (s) requestAnimationFrame(() => { if (l2.isConnected) fitMeta(l2); }); }
    put(t);
    if (opts.analytics) { put(rangeControl(bar)); account(); put.done(); return; }
    if (!s) { account(); put.done(); return; }
    const fb = btn("ibtn", null, "Find and filter"); fb.id = "find-btn"; fb.append(icon(I.search)); fb.addEventListener("click", () => { findOpen = true; render(); $("#find")?.focus(); });
    const mb = btn("ibtn", null, "Session menu: details, cost and actions"); mb.id = "more-btn"; mb.setAttribute("aria-haspopup", "dialog"); mb.setAttribute("aria-expanded", "false"); mb.append(icon(I.more)); mb.addEventListener("click", () => openSessionMenu(SESS[s.id] ?? s, mb));
    if (!opts.traceSession) put(fb); put(mb); account(); put.done();
  }
  // The Analytics range control, a persistent control of the bar: each redraw keeps it and only sets which button is pressed.
  function rangeControl(bar) {
    const group = slot("range", bar, () => {
      const g = el("div", "analytics-range"); g.setAttribute("role", "group"); g.setAttribute("aria-label", "Analytics range");
      for (const [days, label] of [[1, "24 h"], [7, "7 d"], [30, "30 d"]]) {
        const b = el("button", null, label); b.type = "button"; b.dataset.e = "analytics-range:" + days;
        b.addEventListener("click", () => { if (analyticsRange === days) return; const top = currentScroll(); analyticsRange = days; render(); restoreScroll(top); refreshAnalytics(true); }); g.append(b);
      }
      return g;
    }).el;
    for (const b of group.children) b.setAttribute("aria-pressed", String(analyticsRange === Number(b.dataset.e.split(":")[1])));
    return group;
  }
  function lineageOf(sid) {
    const path = [], seen = new Set(); let id = sid;
    while (id && SESS[id] && !seen.has(id)) { seen.add(id); path.push(SESS[id]); id = parentOf(id); }
    return path.reverse();
  }
  // The label and buttons in place, so focus stays where it is.
  // ---- Errors mode: "N errors" steps through the session's failed steps ---------------------------------------------------------
  // The bar reads "Error k of N" with previous and next, and a close button (Escape). /api/tx?errors=1 says where every failed
  // step is (its slot), so a step on a page not loaded yet is reachable: a page next to the loaded range is added to it, one
  // further away replaces it with the page around the step. Each step is scrolled to the middle and marked, never opened;
  // its tool group opens so it shows. Closing puts back the pages, what was open and the scroll position from before. The
  // mode is its own controller, apart from find, so the two can become one mode later.
  const ERR = { mode: "errors", on: false, sid: null, slots: [], listed: false, count: 0, version: null, k: -1, slot: null, saved: null, range: null, tools: true, chain: Promise.resolve(), gen: 0 };
  const ERR_NEAR = 400, ERR_AROUND = 40; // slots: a page (at most 200 entries) or two away is added; 40 entries of context above
  const errLive = el("div", "sr-only"); errLive.setAttribute("role", "status"); errLive.setAttribute("aria-live", "polite"); if (!SIDEBAR_ONLY) document.body.append(errLive);
  const signalCount = (s) => Object.values(s?.signals ?? {}).reduce((n, x) => n + x, 0);
  const errText = () => ERR.k < 0 ? (ERR.count ? "Finding " + ERR.mode + "…" : "No " + ERR.mode) : (ERR.mode === "signals" ? "Signal " : "Error ") + (ERR.k + 1) + " of " + ERR.count;
  const errOn = (sid) => ERR.on && ERR.sid === sid;
  function errorsBar(bar) {
    const close = el("button", "ibtn"); close.type = "button"; close.id = "err-close"; close.setAttribute("aria-label", "Close " + ERR.mode); close.append(icon(I.x)); close.addEventListener("click", () => closeErrors());
    const pill = el("div", "find errnav"), mark = el("span", "errs-dot"); mark.setAttribute("aria-hidden", "true"); pill.append(mark, el("span", "errnav-count", errText()));
    const prev = el("button", "ibtn errnav-btn"); prev.type = "button"; prev.id = "err-prev"; prev.setAttribute("aria-label", ERR.mode === "signals" ? "Previous signal" : "Previous error"); prev.append(icon(I.up)); prev.addEventListener("click", () => stepErrors(-1));
    const next = el("button", "ibtn errnav-btn"); next.type = "button"; next.id = "err-next"; next.setAttribute("aria-label", ERR.mode === "signals" ? "Next signal" : "Next error"); next.append(icon(I.dn)); next.addEventListener("click", () => stepErrors(1));
    prev.disabled = next.disabled = ERR.listed && !ERR.slots.length;
    const group = el("div", "errnav-bar"); group.setAttribute("role", "group"); group.setAttribute("aria-label", ERR.mode === "signals" ? "Session signals" : "Failed steps"); group.append(close, pill, prev, next);
    bar.append(group);
  }
  // The label and buttons in place, so focus stays where it is.
  function errLabel(announce) {
    const t = $("#topbar .errnav-count"); if (t) t.textContent = errText();
    for (const id of ["err-prev", "err-next"]) { const b = document.getElementById(id); if (b) b.disabled = ERR.listed && !ERR.slots.length; }
    if (announce) errLive.textContent = errText();
  }
  const drawSessionBar = () => {
    const s = SESS[route.id]; if (route.v !== "session" || !s) return;
    renderTopbar(s.name, null, { session: s, lineage: lineageOf(route.id).slice(0, -1), line2: sessionLine(s) });
    document.documentElement.style.setProperty("--barh", $("#topbar").offsetHeight + "px"); syncBarLine();
  };
  // A redraw keeps focus on the bar's control that had it.
  const keepFocus = (fn) => { const id = document.activeElement?.id; fn(); const n = id && document.getElementById(id); if (n && n !== document.activeElement) n.focus({ preventScroll: true }); };
  function openErrors(sid, mode = "errors") {
    if (ERR.on || route.v !== "session" || route.id !== sid || !TXM[sid]) return;
    resetPagerInput(); stopOpeningEndPin(); find = "";
    Object.assign(ERR, { mode, on: true, sid, slots: [], listed: false, count: mode === "signals" ? signalCount(SESS[sid]) : countOf(SESS[sid], "errors") ?? 0, version: null, k: -1, slot: null, saved: capture(), range: { tx: TX[sid], m: { ...TXM[sid] } }, tools: show.tools, gen: ERR.gen + 1 });
    if (!show.tools) { show.tools = true; render(); } else drawSessionBar();
    document.getElementById("err-next")?.focus({ preventScroll: true }); errLabel(true);
    const gen = ERR.gen;
    fetchErrors(sid).then(() => { if (ERR.gen === gen && ERR.on && ERR.slots.length) { ERR.k = 0; showError(true); } else errLabel(true); }, () => { if (ERR.gen === gen && ERR.on) { errLive.textContent = "Couldn't list " + ERR.mode; const t = $("#topbar .errnav-count"); if (t) t.textContent = "Couldn't list " + ERR.mode; } });
  }
  // The list, or nothing new (304) when the model hasn't moved since it was fetched.
  function fetchErrors(sid) {
    return fetch("/api/tx?sid=" + enc(sid) + "&" + ERR.mode + "=1" + (ERR.version ? "&since=" + enc(ERR.version) : ""), { credentials: "same-origin" })
      .then((r) => r.status === 304 ? null : r.ok ? r.json() : Promise.reject(Object.assign(new Error(r.status + " " + r.statusText), { status: r.status })), (e) => Promise.reject(Object.assign(e, { status: 0 })))
      .then((x) => {
        if (!x || !errOn(sid)) return;
        ERR.listed = true; ERR.slots = Array.isArray(x.slots) ? x.slots.filter(Number.isInteger) : []; ERR.count = Number.isInteger(x[ERR.mode]) ? Math.max(x[ERR.mode], ERR.slots.length) : ERR.slots.length; ERR.version = typeof x.version === "string" ? x.version : null;
        // The current step stays current wherever it now is in the list; one no longer failed gives way to the next after it.
        if (ERR.slot != null) {
          const at = ERR.slots.indexOf(ERR.slot), after = ERR.slots.findIndex((slot) => slot > ERR.slot);
          ERR.k = at >= 0 ? at : !ERR.slots.length ? -1 : after >= 0 ? after : ERR.slots.length - 1;
          if (at < 0) ERR.slot = ERR.k >= 0 ? ERR.slots[ERR.k] : null;
        }
      });
  }
  function stepErrors(delta) {
    if (!ERR.on || !ERR.slots.length) return;
    const n = ERR.slots.length; ERR.k = ERR.k < 0 ? 0 : (((ERR.k + delta) % n) + n) % n; showError(true);
  }
  const hasSlot = (sid, slot) => { const m = TXM[sid]; return !!m && slot >= m.from && slot < m.to; };
  // Loads the page holding `slot` when it isn't loaded: resolves true when the loaded range changed.
  function loadSlot(sid, slot) {
    if (hasSlot(sid, slot)) return Promise.resolve(false);
    const m = TXM[sid], up = slot < m.from, near = up ? m.from - slot <= ERR_NEAR : slot - m.to < ERR_NEAR;
    let tries = 0;
    const extend = () => hasSlot(sid, slot) || tries++ >= 3 ? null : fetchTx(sid, up ? "before=" + TXM[sid].from : "after=" + TXM[sid].to, up ? "before" : "after").then(extend);
    const around = () => hasSlot(sid, slot) ? null : fetchTx(sid, "after=" + Math.max(0, slot - ERR_AROUND)).then(() => (hasSlot(sid, slot) ? null : fetchTx(sid, "after=" + slot)));
    return Promise.resolve(near ? extend() : null).then(around).then(() => true);
  }
  // The session's own step for a slot: not one in a child run's work drawn inside it.
  function errNode(sid, slot) {
    const e = (TX[sid] ?? []).find((x) => x.k === (ERR.mode === "signals" ? "signal" : "tool") && x.slot === slot); if (!e?.key) return null;
    return [...$("#page").querySelectorAll(".turns [data-e]")].find((n) => n.dataset.e === e.key) ?? null;
  }
  // Marks the current step (its group opened so it shows); with `ring`, rings it for a moment and centres it under the bar.
  function markError(ring) {
    for (const n of $("#page").querySelectorAll("[data-e].err-current")) n.classList.remove("err-current", "err-ring");
    if (!ERR.on || ERR.slot == null || route.v !== "session" || route.id !== ERR.sid) return null;
    const node = errNode(ERR.sid, ERR.slot); if (!node) return null;
    const g = node.closest(".tgroup"), sum = g && opener(g); if (sum?.getAttribute("aria-expanded") === "false") sum.click();
    node.classList.add("err-current");
    if (ring) { node.classList.remove("err-ring"); void node.offsetWidth; node.classList.add("err-ring"); setTimeout(() => node.classList.remove("err-ring"), 1500); }
    return node;
  }
  function centre(node) {
    if (!node.isConnected) return;
    const sc = scroller(), r = (node.querySelector(":scope > button") ?? node).getBoundingClientRect(), bottom = phone.matches ? window.innerHeight : $("#main").getBoundingClientRect().bottom;
    const d = (r.top + r.bottom) / 2 - (edge() + bottom) / 2; scrollProgrammatically(() => { if (Math.abs(d) >= 1) sc.scrollTop += d; });
    syncBarLine(); syncJump(); saveHistoryScroll();
  }
  function showError(announce) {
    resetPagerInput();
    const sid = ERR.sid, slot = ERR.slots[ERR.k], gen = ERR.gen; ERR.slot = slot; errLabel(announce);
    ERR.chain = ERR.chain.then(() => {
      if (!ERR.on || ERR.gen !== gen || ERR.slot !== slot) return null; // a later step or a close since
      stopOpeningEndPin();
      return loadSlot(sid, slot).then((moved) => {
        if (!ERR.on || ERR.gen !== gen || ERR.slot !== slot || route.v !== "session" || route.id !== sid) return;
        if (moved) keepFocus(render); // render marks the current step again
        const node = markError(true);
        if (!node) { if (announce) errLive.textContent = errText() + ", not shown in this transcript"; return; }
        centre(node); requestAnimationFrame(() => requestAnimationFrame(() => { if (ERR.on && ERR.slot === slot && node.isConnected) centre(node); }));
      }, () => { if (ERR.on && ERR.gen === gen) errLive.textContent = "Couldn't load " + errText(); });
    }).catch((e) => { setTimeout(() => { throw e; }); }); // a fault on the page, reported as one; the next step still runs
  }
  // Leaves the mode. By navigation (`away`), a range the mode moved is dropped, so the next visit loads the end afresh and
  // is tailed again; closeErrors puts the range from before back instead.
  function dropErrors(away) {
    if (!ERR.on) return;
    const sid = ERR.sid, range = ERR.range, m = TXM[sid];
    ERR.on = false; ERR.gen++; show.tools = ERR.tools; ERR.saved = ERR.range = null; errLive.textContent = "";
    if (away && range && m && (m.from !== range.m.from || m.to < range.m.to)) dropTx(sid);
  }
  function closeErrors() {
    if (!ERR.on) return;
    const sid = ERR.sid, saved = ERR.saved, range = ERR.range, m = TXM[sid]; dropErrors();
    let p = Promise.resolve();
    // Pages loaded above the range (or a range replaced by a page further off) move its entries' keys: the range from before
    // comes back, caught up with the tail when it reached the end and the session grew since.
    if (range && m && TX[sid] && (m.from !== range.m.from || m.to < range.m.to)) {
      TX[sid] = range.tx; TXM[sid] = range.m; spread(sid);
      if (range.m.to >= range.m.total && range.m.tok !== TOK[sid]) p = tail(sid).catch(() => null);
    }
    p.then(() => {
      if (route.v !== "session" || route.id !== sid || ERR.on) return;
      render(); if (saved) restore(saved);
      const b0 = $("#topbar .lab-errs") ?? $('#topbar .chip[data-filter="failures"]'), b = b0 && !b0.getClientRects().length ? $("#more-btn") : b0; /* on a phone the line is not drawn: focus goes to ⋯ */ if (b && !b.hidden && document.activeElement !== b && (!document.activeElement || document.activeElement === document.body || !document.activeElement.isConnected)) b.focus({ preventScroll: true });
    });
  }
  // Keys while the mode is on: n and p (and Enter, Shift+Enter in the bar) step, Escape closes. Not while typing, and not
  // under an open sheet.
  if (!SIDEBAR_ONLY) document.addEventListener("keydown", (e) => {
    if (!ERR.on || e.defaultPrevented || e.altKey || e.ctrlKey || e.metaKey || document.querySelector("dialog[open]")) return;
    // The drawer and an open menu have the keys first: Escape closes them and leaves the mode on.
    if (document.body.classList.contains("drawer-open") || document.querySelector(".menu")) return;
    if (e.target.closest?.("input, textarea, select, [contenteditable='true']")) return;
    const inBar = !!e.target.closest?.("#topbar .errnav-bar"), onButton = e.target.tagName === "BUTTON";
    if (e.key === "Escape") { e.preventDefault(); closeErrors(); return; }
    if (e.key === "n" || e.key === "N") { e.preventDefault(); stepErrors(1); return; }
    if (e.key === "p" || e.key === "P") { e.preventDefault(); stepErrors(-1); return; }
    if (e.key === "Enter" && (inBar || e.target === document.body)) {
      if (e.shiftKey) { e.preventDefault(); stepErrors(-1); }
      else if (!onButton) { e.preventDefault(); stepErrors(1); }
    }
  });
  // Live: a new model lists the errors again; N grows, the current one stays.
  function errorsLive() {
    if (!ERR.on || route.v !== "session" || route.id !== ERR.sid) return null;
    const sid = ERR.sid, gen = ERR.gen, was = ERR.count;
    return fetchErrors(sid).then(() => {
      if (!ERR.on || ERR.gen !== gen) return;
      if (ERR.k < 0 && ERR.slots.length) { ERR.k = 0; showError(true); return; }
      errLabel(ERR.count !== was); markError(false);
    });
  }
  // Labels drop from the end, least important first, until the line fits. State always stays.
  function fitMeta(l2) {
    const labs = [...l2.querySelectorAll(".lab")]; labs.forEach((n) => { n.hidden = false; });
    const fits = () => l2.scrollWidth <= l2.clientWidth + 1;
    const order = labs.filter((n) => !n.classList.contains("state")).sort((a, b) => Number(b.dataset.drop ?? 0) - Number(a.dataset.drop ?? 0));
    for (const n of order) { if (fits()) break; n.hidden = true; }
  }
  // A label is information; one that leads somewhere (`act`) is a button that looks the same, with its hit area padded to the tap size.
  const turnsLabel = (s) => { const n = (TURNS[s.id] ?? []).filter(hasTurn).length; return n + (n === 1 ? " turn" : " turns"); };
  // On a phone the line of labels leaves the bar and the state is the small dot before the title. The dot names the state for a screen reader, and
  // its tip (a tap on a phone) adds the turn count. A desktop hides it, since the line shows the state there.
  const stateLead = (s) => { const lead = el("span", "l1-state"); lead.dataset.tip = "Status: " + STATE[s.state] + " · " + turnsLabel(s); lead.append(dot(s.state, false)); return lead; };
  const lab = (text, tip, drop, cls, act) => { const x = el(act ? "button" : "span", "lab" + (act ? " lab-btn" : "") + (cls ? " " + cls : ""), text); if (tip) { x.dataset.tip = tip; if (act) x.setAttribute("aria-label", tip); } if (act) { x.type = "button"; x.addEventListener("click", act); } x.dataset.drop = String(drop); return x; };
  // "Started 21:57 on <machine>" stays on one line: the machine name ellipsises (its tip, only while cut off, has the whole name).
  const startedDivider = (sid) => { const d = el("div", "divider started"), name = MACHINE[SESS[sid].movedFrom ?? SESS[sid].machine], line = el("span", "dv-text"), m = el("span", "dv-machine", name); m.dataset.tip = name; m.dataset.tipClipped = ""; line.append(el("span", "dv-lead", "Started " + clock(SESS[sid].start) + " on\u00a0"), m); d.append(line); return d; };
  // A session's line: its state, then kind, model, failed steps, machine, branch and API-equivalent cost, each a plain label.
  const sessionLine = (s) => (l2) => {
    const failed = countOf(s, "errors") ?? 0;
    const st = el("span", "lab state " + s.state); st.append(dot(s.state, false), el("span", null, STATE[s.state])); l2.append(st);
    l2.append(lab(kindText(s), s.kind ? s.kind + " · " + HARNESS[s.harness] : null, 2));
    const model = lab(shortModel(s.model), "Model: " + modelIdOf(s) + (s.effort ? ". Reasoning effort: " + s.effort : ""), 3, "meta-model"); if (s.effort) { const eff = el("span", "meta-effort"); eff.append(el("span", "meta-sep", "·"), s.effort); model.append(eff); } l2.append(model);
    // Failed steps and runs are the two labels that lead somewhere, so they are the last two the fitter drops.
    if (failed) l2.append(lab(failed + " failed", failed + (failed === 1 ? " failed step" : " failed steps") + ": step through them", 0, "lab-errs", () => openErrors(s.id)));
    const runs = descendantsOf(s.id, sessionChildren());
    if (runs.length) l2.append(lab(runs.length + (runs.length === 1 ? " run" : " runs"), runs.length + (runs.length === 1 ? " run" : " runs") + " under this session: open the list with their cost", 1, "lab-runs", () => openSessionMenu(SESS[s.id] ?? s, $("#more-btn"), ".runs")));
    l2.append(lab(MACHINE[s.machine], "Machine: " + MACHINE[s.machine] + " · " + hostOf(s), 5));
    if (s.branch) l2.append(lab(s.branch, "Branch: " + s.branch, 6));
    const cost = runs.length ? costForSessions([s, ...runs]) : costForSession(s.id);
    l2.append(lab(costText(cost), "API-equivalent cost" + (runs.length ? ", with " + runs.length + (runs.length === 1 ? " run" : " runs") : "") + ". Details in the session menu.", 7));
  };
  const machineLine = (m) => (l2) => { const here = onMachine(m), w = here.filter((s) => s.state === "work").length, up = MACHINE_UP[m];
    const st = el("span", "lab state " + (up ? "done" : "err")); st.append(dot(up ? (w ? "work" : "idle") : "err", false), el("span", null, up ? "Up" : "Not responding")); l2.append(st);
    const sessions = here.length + (here.length === 1 ? " session" : " sessions");
    l2.append(lab(up ? w + " working · " + sessions : movedOff(m).length ? movedOff(m).length + " moved off" : [MACHINE_LAST[m] != null ? "Last seen " + clock(MACHINE_LAST[m]) : null, sessions].filter(Boolean).join(" · "), null, 1)); };
  // What Find counts: the matches on the page when there is a search or a filter, else nothing.
  const matchCount = () => find || !show.messages || !show.tools || !show.thinking ? $("#page").querySelectorAll(".turns .msg, .turns .bubble, .turns .step:not(.bgend), .turns .event, .turns .child-card").length : null;
  const matchText = (n) => n == null ? "" : n ? n + (n === 1 ? " match" : " matches") : "No matches";
  // Find and filter: one mode. Search takes over the bar and the filters sit under it as chips, one choice at a time.
  function findBar(bar, s, account) {
    const row = el("div", "find-row");
    const back = btn("ibtn", null, "Close find"); back.append(icon(I.back)); back.addEventListener("click", () => { findOpen = false; find = ""; show = { ...SHOW_ALL }; render(); });
    const fr = el("label", "search"); const fi = el("input"); fi.id = "find"; fi.type = "search"; fi.placeholder = "Find in " + s.name; fi.setAttribute("aria-label", "Find in transcript"); fi.value = find; fr.append(icon(I.search), fi);
    fi.addEventListener("input", () => { find = fi.value.toLowerCase(); const pos = fi.selectionStart; render(); const a = $("#find"); a?.focus(); a?.setSelectionRange(pos, pos); });
    fi.addEventListener("keydown", (e) => { if (e.key === "Escape") { e.stopPropagation(); back.click(); } });
    const c = el("span", "fcount", matchText(matchCount())); c.setAttribute("aria-live", "polite");
    row.append(back, fr, c); account(row); bar.append(row);
    const chips = el("div", "find-chips"); chips.setAttribute("role", "group"); chips.setAttribute("aria-label", "Show");
    const failed = countOf(s, "errors") ?? 0;
    // Message and step filters; errors and signals open a positional stepper.
    const MODES = { all: { ...SHOW_ALL }, messages: { messages: true, tools: false, thinking: false }, steps: { messages: false, tools: true, thinking: false } };
    const mode = show.messages && show.tools ? "all" : show.messages ? "messages" : "steps";
    const chip = (key, label, count) => { const b = btn("chip"); b.dataset.filter = key; b.setAttribute("aria-pressed", String(mode === key)); b.append(el("span", null, label)); if (count != null) b.append(el("span", "n", String(count))); b.addEventListener("click", () => { if (key === "failures" || key === "signals") { openErrors(s.id, key === "signals" ? "signals" : "errors"); return; } show = { ...MODES[key] }; render(); }); chips.append(b); };
    chip("all", "All"); chip("messages", "Messages"); chip("steps", "Steps"); if (failed) chip("failures", "Failed steps", failed); if (signalCount(s)) chip("signals", "Signals", signalCount(s));
    bar.append(chips);
  }
  // One observer for the current page title; the previous page's is disconnected so it can't flip the new bar.
  let titleObs = null;
  function observeTitle() { syncBarLine(); }
  // The bar's divider shows only once the page has scrolled.
  function syncBarLine() { const y = phone.matches ? window.scrollY : $("#main").scrollTop; $("#topbar").classList.toggle("scrolled", y > 4); }
  if (!SIDEBAR_ONLY) { window.addEventListener("scroll", syncBarLine, { passive: true }); $("#main").addEventListener("scroll", syncBarLine, { passive: true }); }
  if (!SIDEBAR_ONLY) window.addEventListener("resize", () => { const l2 = $("#topbar .meta-line"); if (l2 && route.v === "session") fitMeta(l2); syncLayoutPrefs(); syncJump(); }, { passive: true });

  // ---- Panels: one builder for the sheets and menus opened from the top bar --------------------------------------------
  // A phone gets a bottom sheet; a desktop a dialog, or for the session menu a panel that hangs from its button. Each is a
  // history entry, so back closes it without leaving the page, and a live update waits until it closes.
  function panel(title, opts = {}) {
    const d = el("dialog", "panel" + (opts.cls ? " " + opts.cls : "")); d.setAttribute("aria-label", opts.label ?? title);
    const head = el("div", "panel-h"); head.append(el("div", "panel-t", title)); if (opts.sub) head.append(el("div", "panel-sub", opts.sub));
    const close = btn("ibtn", null, "Close"); close.append(icon(I.x)); close.addEventListener("click", () => d.close()); head.append(close);
    const body = el("div", "panel-b"); body.tabIndex = -1; d.append(head, body); document.body.append(d);
    d.addEventListener("click", (ev) => { if (ev.target === d) d.close(); });
    d.addEventListener("close", () => { d.remove(); document.documentElement.classList.remove("panel-open"); opts.onClose?.(); if (viewerEl === d) { viewerEl = null; if (history.state?.sheet) { skipPop = true; history.back(); } else if (pendingSessionOpen) { const id = pendingSessionOpen; pendingSessionOpen = null; goSession(id); } } if (LIVE.pending) refresh(); });
    const hold = (e) => { if (!d.open || !d.isConnected) return; if (!body.contains(e.target) || body.scrollHeight <= body.clientHeight + 1) e.preventDefault(); };
    d.addEventListener("close", () => { for (const type of ["wheel", "touchmove"]) document.removeEventListener(type, hold, { capture: true }); });
    const open = () => { viewerEl = d; for (const type of ["wheel", "touchmove"]) document.addEventListener(type, hold, { capture: true, passive: false }); document.documentElement.classList.add("panel-open"); d.showModal(); body.focus({ preventScroll: true }); try { history.pushState({ ...route, sheet: 1, scrollTop: currentScroll() }, ""); } catch {} };
    return { d, body, show: open };
  }

  // The session menu: actions, then details, then cost. It is the one place for all three.
  const RUNS_CAP = 5;
  // Shown once in the UI, at the foot of the session menu; NOTICE.md and the README carry it too. The harness marks are the property of their owners.
  const TRADEMARK_NOTICE = "Third-party trademarks are the property of their respective owners. Semon is not affiliated with or endorsed by these companies.";
  function openSessionMenu(s, anchor, scrollTo) {
    const kids = descendantsOf(s.id, sessionChildren());
    const { d, body, show: open } = panel(s.name, { cls: "anchored session-menu", label: "Session menu for " + s.name, sub: [STATE[s.state], kindText(s), shortModel(s.model)].join(" · "), onClose: () => { anchor?.setAttribute("aria-expanded", "false"); anchor?.focus({ focusVisible: false }); } });
    const acts = el("div", "menu-list"); acts.setAttribute("role", "menu");
    const traceTurn = route.v === "trace" ? TURN.get(route.turn) : (() => { const top = $("#topbar").getBoundingClientRect().bottom; const nodes = [...document.querySelectorAll("#page .turn[data-turn]")].filter((n) => !n.closest(".cw-body")); const visible = nodes.find((n) => n.getBoundingClientRect().bottom > top); return TURN.get(visible?.dataset.turn) ?? (TURNS[s.id] ?? []).at(-1); })();
    if (traceTurn?.out.length) { const b = btn("menu-item menu-trace"); b.setAttribute("role", "menuitem"); b.append(icon(I.trace, "icon"), el("span", null, "Trace this turn")); b.addEventListener("click", () => { afterPop = () => goTrace(traceTurn.id); d.close(); }); acts.append(b); }
    const cmd = s.harness === "codex" ? "codex resume " + s.id : "claude --resume " + (s.sessionId ?? s.id);
    const copy = btn("menu-item"); copy.setAttribute("role", "menuitem"); copy.append(icon(I.copy, "icon"), el("span", null, "Copy resume command")); copy.addEventListener("click", () => { navigator.clipboard?.writeText(cmd).then(() => { copy.children[1].textContent = "Copied"; }, () => { copy.children[1].textContent = cmd; }); }); acts.append(copy);
    if (s.harness === "claude") { const a = btn("menu-item"); a.setAttribute("role", "menuitem"); a.append(icon(I.ext, "icon"), el("span", null, "Open in claude.ai")); acts.append(a); }
    if (!phone.matches) { const w = btn("menu-item"); w.setAttribute("role", "menuitemcheckbox"); w.setAttribute("aria-checked", String(wideMode)); w.append(icon(I.wide, "icon"), el("span", null, "Wide transcript"), el("span", "switch")); w.addEventListener("click", () => { setWideMode(!wideMode); w.setAttribute("aria-checked", String(wideMode)); }); acts.append(w); }
    const signals = signalCount(s);
    if (signals) { const b = btn("menu-item menu-signals"); b.setAttribute("role", "menuitem"); b.append(el("span", null, signals + " signals"), el("span", "menu-note", "Step through")); b.addEventListener("click", () => { d.close(); openErrors(s.id, "signals"); }); acts.append(b); }
    // On a phone the line of labels is not in the bar, so what it held that leads somewhere is reached here.
    if (phone.matches) {
      const failed = countOf(s, "errors") ?? 0;
      if (failed) { const e = btn("menu-item menu-errors"); e.setAttribute("role", "menuitem"); const mark = el("span", "dot err"); mark.setAttribute("aria-hidden", "true"); e.append(mark, el("span", null, failed + " failed"), el("span", "menu-note", "Step through")); e.addEventListener("click", () => { d.close(); openErrors(s.id); }); acts.append(e); }
      if (kids.length) { const q = btn("menu-item menu-runs"); q.setAttribute("role", "menuitem"); q.append(icon(I.stack, "icon"), el("span", null, "Runs · " + kids.length)); q.addEventListener("click", () => { body.querySelector(".runs")?.scrollIntoView({ block: "start" }); }); acts.append(q); }
    }
    // A child session's phone bar has neither the chevron nor the state dot: the menu holds its status and the path up.
    const path = phone.matches ? lineageOf(s.id) : [];
    if (path.length > 1) {
      // The menu is rebuilt each time it opens, so this status uses the latest state and turn count.
      const status = el("div", "menu-status"); status.setAttribute("role", "presentation"); const statusDot = dot(s.state, false); statusDot.setAttribute("aria-hidden", "true"); status.append(statusDot, el("span", null, STATE[s.state] + " · " + turnsLabel(s)));
      const group = el("div", "menu-path-group"), heading = el("div", "menu-section-heading", "Session path"); heading.id = "menu-path-heading"; group.setAttribute("role", "group"); group.setAttribute("aria-labelledby", heading.id); group.append(heading);
      const ancestors = path.slice(0, -1);
      ancestors.forEach((ancestor, i) => {
        const item = btn("menu-item menu-path-item"); item.setAttribute("role", "menuitem");
        const chevron = i === ancestors.length - 1, slot = el("span", "menu-path-chevron" + (chevron ? "" : " blank")); if (chevron) item.setAttribute("aria-label", "Up to " + ancestor.name); else slot.setAttribute("aria-hidden", "true"); item.append(slot);
        item.append(el("span", "menu-path-name", ancestor.name), harnessName(ancestor.harness));
        item.addEventListener("click", () => { pendingSessionOpen = ancestor.id; d.close(); }); group.append(item);
      });
      // The group of menuitems sits inside a role=menu container, as the actions below do.
      const pathMenu = el("div", "menu-list menu-path-menu"); pathMenu.setAttribute("role", "menu"); pathMenu.append(group);
      const separator = el("div", "menu-separator"); separator.setAttribute("role", "separator");
      body.append(status, pathMenu, separator);
    }
    const a1 = el("section", "panel-sec"); a1.append(acts); body.append(a1);
    const det = el("section", "panel-sec"); det.append(el("h3", null, "Details"));
    const dl = el("dl", "kv"), machine = MACHINE[s.machine] ?? s.machine ?? "Unknown machine";
    const calls = countOf(s, "calls"), errorCount = countOf(s, "errors") ?? 0;
    const rows = [["Status", STATE[s.state] + " · " + turnsLabel(s)], ...(s.kind ? [["Kind", s.kind]] : []), ["Harness", withHarnessIcon(el("span", null, HARNESS[s.harness] ?? s.harness), s.harness, { size: 16 })], ["Model", modelIdOf(s), true], ...(s.effort ? [["Effort", s.effort]] : []), ["Machine", machine + (hostOf(s) !== machine ? " · " + hostOf(s) : "") + (s.movedFrom ? " (moved from " + (MACHINE[s.movedFrom] ?? s.movedFrom) + ")" : "")], ["Directory", s.cwd ?? s.dir ?? s.directory, true], [s.worktree ? "Worktree" : "Branch", branchOf(s), true], ["Tool calls", calls == null ? "—" : String(calls)], ...(errorCount ? [["Errors", String(errorCount)]] : []), ["Started", clock(s.start)], ["Duration", dur(s.start, s.state === "work" ? null : s.last)], ["Process id", s.pid, true], ["Session id", s.sessionId ?? s.id, true]];
    for (const [k, v, mono] of rows) { if (v == null || v === "") continue; dl.append(el("dt", null, k), (() => { const d = el("dd", mono ? "mono" : null, v instanceof Node ? null : String(v)); if (v instanceof Node) d.append(v); return d; })()); }
    for (const [kind, n] of Object.entries(s.signals ?? {})) dl.append(el("dt", null, "Signals · " + kind), el("dd", null, String(n)));
    det.append(dl); body.append(det, costSection(s, kids, d), el("p", "third-party", TRADEMARK_NOTICE));
    anchor?.setAttribute("aria-expanded", "true");
    open(); if (scrollTo) body.querySelector(scrollTo)?.scrollIntoView({ block: "nearest" }); return d;
  }
  // 739,682 reads "740k" and 12,422,228 "12.4M"; the exact figure is the cell's tooltip.
  const compactCount = (n) => { if (n < 1e3) return String(n); if (n < 1e4) return +(n / 1e3).toFixed(1) + "k"; const k = Math.round(n / 1e3); return k < 1e3 ? k + "k" : +(n / 1e6).toFixed(1) + "M"; };
  const MENU_KINDS = [["Input", ["input"]], ["Output", ["output"]], ["Cache write", ["cache_write_5m", "cache_write_1h"]], ["Cache read", ["cache_read"]], ["Web search", ["web_search"]]];
  const COST_NOTE = "What these tokens would cost at API rates. Subscriptions aren't billed this way.";
  function costSection(s, kids, dialog) {
    const sec = el("section", "panel-sec cost"); sec.append(el("h3", null, "Cost"));
    const own = costForSession(s.id), all = costForSession(s.id, true);
    const fig = el("div", "cost-fig"); fig.append(el("span", "cost-big", costText(kids.length ? all : own)), el("span", "cost-cap", kids.length ? "this session and its " + kids.length + (kids.length === 1 ? " run" : " runs") : "this session"));
    const missing = costMissing(all);
    sec.append(fig, el("p", "cost-note", COST_NOTE + (missing.length ? " No price for " + missing.join(", ") + "." : "")));
    const dl = el("dl", "kv");
    if (kids.length) dl.append(el("dt", null, "This session"), el("dd", null, costText(own)), el("dt", null, kids.length === 1 ? "Its run" : "Its " + kids.length + " runs"), el("dd", null, costText(costForSessions(kids))));
    const reports = s.reported_runs ?? [], reported = reports.filter((r) => Number.isFinite(r.cost_usd));
    if (reports.length) dl.append(el("dt", null, HARNESS[s.harness] + "'s own figure"), el("dd", null, reported.length ? asMoney(reported.reduce((n, r) => n + r.cost_usd, 0)) + (reported.length === 1 ? ", last run" : ", last " + reported.length + " runs") : "not reported"));
    if (dl.childElementCount) sec.append(dl);
    const mismatch = [...(s.cost_check ?? [])].reverse().find((c) => c.ok === false && Number.isFinite(c.computed_usd) && Number.isFinite(c.reported_usd));
    if (mismatch) { const pct = mismatch.reported_usd === 0 ? 100 : Math.round(Math.abs(mismatch.computed_usd - mismatch.reported_usd) / Math.abs(mismatch.reported_usd) * 100); sec.append(el("p", "cost-note", "Semon's estimate for that run is " + pct + "% " + (mismatch.computed_usd > mismatch.reported_usd ? "above" : "below") + " " + HARNESS[s.harness] + "'s figure: API rates differ from what a plan is charged.")); }
    if (kids.length) {
      const list = el("div", "runs"); list.setAttribute("aria-label", "Runs and their cost");
      const children = sessionChildren(), rows = [];
      const walk = (id, depth) => { for (const c of [...(children.get(id) ?? [])].sort((a, b) => b.last - a.last)) { rows.push([c, depth]); walk(c.id, depth + 1); } };
      walk(s.id, 0);
      rows.forEach(([c, depth], i) => { const r = btn("run-row depth" + Math.min(depth, 1)); r.hidden = i >= RUNS_CAP; r.setAttribute("aria-label", "Open " + c.name + ", " + kindText(c) + ", " + STATE[c.state] + ", " + costText(costForSession(c.id))); const nm = el("span", "nm", c.name); nm.append(el("span", "kind", "· " + kindText(c))); r.append(dot(c.state), nm, el("span", "v", costText(costForSession(c.id))), icon(I.chev, "chev")); r.addEventListener("click", () => { pendingSessionOpen = c.id; dialog.close(); }); list.append(r); });
      if (rows.length > RUNS_CAP) { const m = btn("link", "Show " + (rows.length - RUNS_CAP) + " more"); m.addEventListener("click", () => { list.querySelectorAll(".run-row").forEach((r) => { r.hidden = false; }); m.remove(); }); list.append(m); }
      sec.append(list);
    }
    const t = btn("disclose"); t.setAttribute("aria-expanded", "false"); t.append(el("span", null, "Tokens by model" + (kids.length ? " · incl. runs" : "")), icon(I.chev, "chev"));
    const tb = el("div", "tokens"); tb.hidden = true;
    for (const [modelId, model] of Object.entries(all.by_model ?? {})) {
      const priced = model.usd != null && !missing.includes(modelId); tb.append(el("div", "tok-model", modelId));
      for (const [label, keys] of MENU_KINDS) { const tokens = keys.reduce((n, k) => n + (Number(model.tokens?.[k]) || 0), 0), amount = keys.reduce((n, k) => n + (Number(model.usd_by_kind?.[k]) || 0), 0), r = el("div", "tok-line"); if (tokens === 0 && (!priced || amount < 0.005)) continue; const count = el("span", null, tokens ? compactCount(tokens) : ""); if (tokens) { count.dataset.tip = tokens.toLocaleString() + " tokens"; count.append(el("span", "sr-only", " (" + tokens.toLocaleString() + ")")); } r.append(el("span", null, label), count, el("span", null, priced ? asMoney(amount) : "—")); tb.append(r); }
    }
    t.addEventListener("click", () => { tb.hidden = !tb.hidden; t.setAttribute("aria-expanded", String(!tb.hidden)); });
    sec.append(t, tb); return sec;
  }

  // ---- Home: what needs you, then what is running ------------------------------------------------------
  const secHead = (title, n) => { const s = el("div", "sec-h", title); s.append(el("span", "n", String(n))); return s; };
  const upCount = () => Object.keys(MACHINE).filter((m) => MACHINE_UP[m]).length;
  let allAnswered = false;
  function renderHome(page) {
    const open = inbox(), running = working(), many = Object.keys(MACHINE).length > 1;
    const w = orderList(orderScope("page", pageSig(), route, pageState()), "working", running, byLast); // the counts are the whole list's; the rows keep their order
    const head = el("div", "ph"); const h1 = el("h1", null, "Home"); head.append(h1);
    const sub = el("div", "sub"); for (const [v, l] of [[open.length, "waiting on you"], [running.length, "working"], [upCount() + " of " + Object.keys(MACHINE).length, Object.keys(MACHINE).length === 1 ? "machine up" : "machines up"]]) { const x = el("span"); x.append(el("b", null, String(v)), l); sub.append(x); }
    head.append(sub); page.append(head); observeTitle(h1);
    page.append(secHead("Needs you", open.length));
    const list = el("div", "list");
    for (const h of open) list.append(inboxItem(h, false));
    if (!open.length) list.append(el("p", "empty", "Nothing is waiting on you."));
    page.append(list);
    page.append(secHead("Working now", running.length));
    const live = el("div", "list");
    for (const s of w) live.append(liveRow(s, many));
    if (!running.length) live.append(el("p", "empty", "Nothing is running."));
    page.append(live);
    const done = H.filter((h) => h.kind === "toyou" && h.status === "done").sort((a, b) => b.at - a.at);
    if (done.length) {
      page.append(secHead("Answered", done.length)); const l2 = el("div", "list");
      (allAnswered ? done : done.slice(0, 3)).forEach((h) => l2.append(inboxItem(h, true)));
      if (!allAnswered && done.length > 3) { const more = el("button", "more", "Show all " + done.length + " answered"); more.type = "button"; more.addEventListener("click", () => { allAnswered = true; render(); }); l2.append(more); }
      page.append(l2);
    }
  }
  // Something a session sent you (or a move). Tapping it opens the turn it came from; Trace opens what that turn set off.
  // A link-like block rather than a button, so the Trace button isn't nested in one.
  function inboxItem(h, quiet) {
    const sid = h.kind === "move" ? h.to : h.from, t = HOLDS.get(h.id);
    const r = el("div", "ib" + (quiet ? " quiet" : "")); r.tabIndex = 0; r.setAttribute("role", "link"); r.dataset.h = h.id;
    const [ic, parts] = sentence(h, "you");
    r.append(icon(quiet && h.kind === "toyou" ? I.done : ic)); const ln = el("span", "ln"); ln.append(...parts);
    const time = el("span", "tm");
    time.append(ago(h.at)); r.append(ln, time);
    r.append(rich("span", "q", preview(h.brief)));
    const an = quiet ? answersOf(h) : null; if (an) r.append(el("span", "ans", an.length ? "You answered: " + an.join(" · ") : "Answered · reply not in these logs"));
    if (t) r.append(originLine(t));
    const ctx = el("span", "ctx"); const s = SESS[sid];
    ctx.append(withHarnessIcon(el("span", null, [HARNESS[s.harness], MACHINE[s.machine]].join(" · ")), s.harness, { size: 14 })); if (t?.out.length) ctx.append(traceBtn(t));
    r.append(ctx);
    const open = () => { if (isResult(h)) markSeenResults([h]); goSession(sid, t?.id); };
    r.addEventListener("click", () => { if (!getSelection().isCollapsed) return; open(); });
    r.addEventListener("keydown", (ev) => { if (ev.target === r && (ev.key === "Enter" || ev.key === " ")) { ev.preventDefault(); open(); } });
    return r;
  }
  const traceBtn = (t) => { const b = el("button", "tracebtn"); b.type = "button"; b.setAttribute("aria-label", "Trace what this turn set off"); b.append(icon(I.trace), el("span", null, "Trace")); b.addEventListener("click", (ev) => { ev.stopPropagation(); goTrace(t.id); }); return b; };
  // A session row: state, name, who it works for and on what (the start of its current turn), and its current tool call while it runs.
  function liveRow(s, showMachine) {
    const r = el("button", "nrow"); r.type = "button"; r.dataset.id = s.id; r.addEventListener("click", () => goSession(s.id));
    const ag = el("span", "ag", s.state === "work" ? HARNESS[s.harness] : ago(s.last)); if (s.state === "work") withHarnessIcon(ag, s.harness, { size: 14 });
    r.append(dot(s.state), el("span", "nm", s.name), ag);
    const cur = (TURNS[s.id] ?? []).at(-1), msg = cur?.start?.brief ?? cur?.u?.text;
    const inb = cur?.start ?? (cur?.u ? { from: "you" } : H.find((h) => h.to === s.id && h.kind !== "move"));
    r.append(el("span", "for", [showMachine ? MACHINE[s.machine] : null, inb ? (inb.from === "you" ? "for you" : "for " + nameOf(inb.from)) : null, msg ? oneLine(msg) : null].filter(Boolean).join(" · ")));
    if (s.activity && s.state === "work") { const a = el("span", "act"); a.append(el("span", "spin"), el("span", null, s.activity[0]), el("code", null, s.activity[1]), el("span", "el", Math.max(0, s.activity[2]) + "s")); r.append(a); }
    return r;
  }

  // ---- Machines: where sessions run, and what happens when a machine goes away ------------------------------
  const byState = (a, b) => (RANK[a.state] ?? 3) - (RANK[b.state] ?? 3) || b.last - a.last;
  const onMachine = (m) => Object.values(SESS).filter((s) => s.machine === m).sort(byState);
  const movedOff = (m) => Object.values(SESS).filter((s) => s.movedFrom === m);
  const movesOf = (m) => H.filter((h) => h.kind === "move" && (h.fromMachine === m || h.toMachine === m)).sort((a, b) => b.at - a.at);
  function renderMachines(page) {
    const ms = Object.keys(MACHINE);
    const head = el("div", "ph"); const h1 = el("h1", null, "Machines"); head.append(h1);
    const sub = el("div", "sub"); for (const [v, l] of [[upCount() + " of " + ms.length, "up"], [working().length, "sessions working"]]) { const x = el("span"); x.append(el("b", null, String(v)), l); sub.append(x); }
    head.append(sub); page.append(head); observeTitle(h1);
    const list = el("div", "list");
    for (const m of ms.sort((a, b) => MACHINE_UP[a] - MACHINE_UP[b])) {
      const here = onMachine(m), w = here.filter((s) => s.state === "work").length, up = MACHINE_UP[m];
      const r = el("button", "nrow"); r.type = "button"; r.dataset.m = m; r.addEventListener("click", () => go({ v: "machine", id: m }));
      r.append(dot(!up ? "err" : w ? "work" : "idle"), el("span", "nm", MACHINE[m]), el("span", "ag", !up ? "offline" : w ? "up" : "idle"));
      const mv = movedOff(m).length, mh = movesOf(m).find((h) => h.fromMachine === m);
      r.append(el("span", "for", up ? [w + " working", here.length + (here.length === 1 ? " session" : " sessions")].join(" · ")
        : ["Not responding" + (mh ? " since " + clock(mh.at) : MACHINE_LAST[m] != null ? " since " + clock(MACHINE_LAST[m]) : ""), mv ? mv + (mv === 1 ? " session" : " sessions") + " moved off" : null].filter(Boolean).join(" · ")));
      list.append(r);
    }
    if (ADMIN) { const a = el("button", "more", ADMIN.label); a.type = "button"; a.addEventListener("click", () => location.assign(ADMIN.href)); list.append(a); }
    page.append(list);
  }
  function renderMachine(page, m) {
    const here = onMachine(m), up = MACHINE_UP[m];
    const head = el("div", "ph sr"); const h1 = el("h1", null, MACHINE[m]); head.append(h1); page.append(head); observeTitle(h1);
    if (here.length) {
      page.append(secHead("Sessions", here.length));
      const list = el("div", "list"); for (const s of orderList(orderScope("page", pageSig(), route, pageState()), "machine:" + m, here, byState)) list.append(liveRow(s, false)); page.append(list);
    }
    const off = movedOff(m);
    if (off.length) {
      page.append(secHead("Moved off", off.length));
      const list = el("div", "list"); for (const s of off) { const r = liveRow(s, false); r.querySelector(".for").textContent = "Now on " + MACHINE[s.machine]; list.append(r); } page.append(list);
    }
    const mv = movesOf(m);
    if (mv.length) { page.append(secHead("Moves", mv.length)); const list = el("div", "list"); mv.forEach((h) => list.append(inboxItem(h, true))); page.append(list); }
    if (!here.length && !off.length) page.append(el("p", "empty", "No sessions have run here."));
  }

  // ---- Trace: one turn and what it set off -------------------------------------------------------------------------
  // The root is the turn. Each spawn or relay it sent leads to the turn that handoff started in the receiving session,
  // and on down from there; a message to you is a leaf. One rail, as everywhere: depth shows as a smaller node.
  // One observer measures expandable messages, and releases detached nodes after a redraw.
  const CLAMP_MORE = new Map(); let clampCleanup = null;
  const clampObserver = new ResizeObserver((entries) => {
    for (const { target } of entries) {
      const more = CLAMP_MORE.get(target);
      if (!target.isConnected) { clampObserver.unobserve(target); CLAMP_MORE.delete(target); continue; }
      if (!more || target.classList.contains("open") || !target.clientHeight) continue;
      const clipped = target.scrollHeight > target.clientHeight + 1;
      more.hidden = !clipped; target.classList.toggle("clipped", clipped);
    }
  });
  function observeClamp(body, more) {
    CLAMP_MORE.set(body, more); clampObserver.observe(body);
    pruneClamps();
  }
  function pruneClamps() {
    if (clampCleanup !== null) return;
    clampCleanup = requestAnimationFrame(() => {
      clampCleanup = null;
      for (const node of CLAMP_MORE.keys()) if (!node.isConnected) { clampObserver.unobserve(node); CLAMP_MORE.delete(node); }
    });
  }
  function clampBrief(body, text) {
    const br = markdown(text, "brief"); body.append(br);
    const more = el("button", "more", "Show more"); more.type = "button"; more.hidden = true; more.setAttribute("aria-expanded", "false");
    more.addEventListener("click", () => { const o = br.classList.toggle("open"); more.textContent = o ? "Show less" : "Show more"; more.setAttribute("aria-expanded", String(o)); });
    observeClamp(br, more);
    body.append(more);
  }
  function traceMeta(body, st, text, sid, turn, note, scope) {
    const meta = el("div", "meta"), s = SESS[sid]; const sw = el("span", "stat " + st); sw.append(st === "work" ? el("span", "spin") : dot(st, false), text); meta.append(sw);
    if (s) { const label = machineLabel(s, scope), chip = el("span", "chip-h " + hcls(sid), [s.kind ?? HARNESS[s.harness], label].filter(Boolean).join(" · ")); if (label) chip.dataset.tip = "Machine: " + hostOf(s); meta.append(withHarnessIcon(chip, s.harness, { size: 14, lead: false })); }
    if (note) meta.append(el("span", "gone", note));
    if (s && !s.stub) { const o = el("button", "open", "Open in " + s.name + " ›"); o.type = "button"; o.addEventListener("click", () => goSession(sid, turn?.id)); meta.append(o); }
    body.append(meta);
  }
  const RUN_EXPANDED = new Map(); let runChartObserver = null;
  const allRunNodes = (model) => { const rows = [], walk = (n) => { rows.push(n); n.children.forEach(walk); }; walk(model.rootNode); model.peers.forEach(walk); return rows; };
  // Walk only the turns connected by recorded spawns and relays; the relay rows remain peers in the view.
  function traceAgentTree(root) {
    const rootNode = { sid: root.sid, turn: root, handoff: null, kind: "root", depth: 0, children: [] };
    const peers = [], handoffs = [], spawns = [], seenTurns = new Set(), seenSessions = new Set([root.sid]);
    const walk = (node) => {
      const turn = node.turn;
      if (!turn || seenTurns.has(turn.id)) return;
      seenTurns.add(turn.id);
      const sent = [...(turn.sent ?? [])].sort((a, b) => a.at - b.at);
      handoffs.push(...sent);
      const children = [];
      for (const h of sent) {
        if (h.kind === "spawn") {
          spawns.push(h);
          const childTurn = STARTS.get(h.id);
          if (!SESS[h.to] || seenSessions.has(h.to)) continue;
          seenSessions.add(h.to);
          const child = { sid: h.to, turn: childTurn ?? null, handoff: h, kind: "spawn", depth: node.depth + 1, children: [] };
          node.children.push(child); children.push(child);
        } else if (h.kind === "relay" && SESS[h.to] && !seenSessions.has(h.to)) {
          const peerTurn = STARTS.get(h.id);
          seenSessions.add(h.to);
          const peer = { sid: h.to, turn: peerTurn ?? null, handoff: h, kind: "relay", depth: 0, children: [] };
          peers.push(peer); children.push(peer);
        }
      }
      for (const child of children) walk(child);
    };
    walk(rootNode);
    return { rootNode, peers, handoffs, spawns };
  }
  function renderAgents(root) {
    const model = traceAgentTree(root), session = SESS[root.sid];
    const start = root.at ?? root.start?.at ?? session.start;
    const nextTurn = (TURNS[root.sid] ?? []).find((t) => t.at > start);
    const upper = root.end?.at ?? nextTurn?.at ?? (session.state === "work" ? NOW : session.last) ?? start;
    const end = Math.max(start, upper, ...model.handoffs.map((h) => h.done).filter(Number.isFinite), ...allRunNodes(model).filter((n) => n.kind !== "root").map((n) => SESS[n.sid]?.state === "work" ? NOW : n.handoff?.done ?? SESS[n.sid]?.last).filter(Number.isFinite));
    const span = Math.max(1, end - start);
    const waits = allRunNodes(model).flatMap((node) => (SESS[node.sid]?.wait_edges ?? []).filter((w) => (!w.turn || w.turn === node.turn?.id) && w.start >= start && w.start <= end).map((w) => ({ ...w, sid: node.sid })));
    const critical = new Set(model.spawns.filter((h) => waits.some((w) => w.sid === h.from && w.targets?.includes(h.to))).map((h) => h.id));
    const pct = (m) => Math.max(0, Math.min(100, ((m - start) / span) * 100));
    const steps = [1, 2, 5, 10, 15, 30, 60, 120, 240].map((n) => n * 60000).map((step) => ({ step, count: Math.floor((span - 1) / step) })).filter((x) => x.count >= 3 && x.count <= 6)
      .sort((a, b) => Math.abs(a.count - 5) - Math.abs(b.count - 5) || a.step - b.step);
    const step = steps[0]?.step ?? [1, 2, 5, 10, 15, 30, 60, 120, 240].map((n) => n * 60000).sort((a, b) => Math.abs(Math.floor((span - 1) / a) - 5) - Math.abs(Math.floor((span - 1) / b) - 5))[0];
    const ticks = []; for (let offset = step; offset < span; offset += step) ticks.push(offset);
    const rowCount = (node) => 1 + node.children.reduce((n, child) => n + rowCount(child), 0);
    const allNodes = (node, into = []) => { into.push(node); for (const child of node.children) allNodes(child, into); return into; };
    const totalRows = rowCount(model.rootNode) + model.peers.reduce((n, peer) => n + rowCount(peer), 0), foldRows = totalRows > 12;
    const spawned = model.spawns.filter((h) => Number.isFinite(h.at) && Number.isFinite(h.done) && h.done > h.at).map((h) => ({ start: h.at, end: h.done }));
    const missingSpawnEnds = model.spawns.filter((h) => !Number.isFinite(h.done)).length;
    const points = spawned.flatMap((x) => [{ at: x.start, change: 1 }, { at: x.end, change: -1 }]).sort((a, b) => a.at - b.at || a.change - b.change);
    let active = 0, peak = 0; for (const p of points) { active += p.change; peak = Math.max(peak, active); }
    const totalCost = costForSessions(allRunNodes(model).map((n) => SESS[n.sid]).filter(Boolean));
    const panel = el("section", "agents-panel"); panel.setAttribute("aria-label", "Agents");
    panel.append(el("h2", "agents-title", "Agents"));
    const summary = el("div", "agents-summary"), participantCount = allRunNodes(model).length;
    for (const item of [participantCount + (participantCount === 1 ? " agent" : " agents"), dur(start, end), costText(totalCost) + " session totals", ...(spawned.length ? ["Recorded spawn overlap: " + peak] : []), ...(missingSpawnEnds ? [missingSpawnEnds + " spawn end" + (missingSpawnEnds === 1 ? "" : "s") + " not recorded"] : [])]) summary.append(el("span", null, item));
    panel.append(summary);
    while (RUN_EXPANDED.size > 16) RUN_EXPANDED.delete(RUN_EXPANDED.keys().next().value);
    const axis = el("div", "agents-axis");
    const axisTrack = el("div", "agent-axis-track");
    axisTrack.append(el("span", "agent-axis-time start", clock(start)), el("span", "agent-axis-time end", clock(end)));
    for (const offset of ticks) {
      const mark = el("span", "agent-axis-tick" + (offset / span > .86 ? " near-end" : "")); mark.style.left = pct(start + offset) + "%";
      mark.append(el("span", null, "+" + Math.round(offset / 60000) + "m")); axisTrack.append(mark);
    }
    axis.append(axisTrack); panel.append(axis);
    // The last tick can be right-aligned and run back into its neighbour. Keep the grid records, but show only labels that
    // fit the measured track, including narrow phones and a chart resized after opening.
    const layoutAxisLabels = () => {
      const bounds = axisTrack.getBoundingClientRect(); let previousRight = bounds.left - 8;
      for (const label of axisTrack.querySelectorAll(".agent-axis-tick > span")) {
        label.hidden = false; const rect = label.getBoundingClientRect();
        if (rect.left < previousRight + 8 || rect.right > bounds.right) label.hidden = true;
        else previousRight = rect.right;
      }
    };
    const legend = el("div", "agents-legend"); legend.append(el("span", "agents-legend-swatch"), el("span", null, waits.some((w) => w.targets?.length) ? "Recorded waits on agents" : waits.length ? "Recorded waits without known targets" : "Wait dependencies not recorded")); if (allRunNodes(model).some((node) => SESS[node.sid]?.wait_edges_truncated)) legend.append(el("span", null, "· Wait history incomplete")); panel.append(legend);
    const chart = el("div", "agents-chart"); panel.append(chart);
    const expandedMore = RUN_EXPANDED.get(root.id) ?? new Set(); RUN_EXPANDED.set(root.id, expandedMore);
    const drawRows = () => {
      chart.replaceChildren();
      const rows = new Map(), edgeDefs = [];
      const axisGridlines = (track) => { for (const offset of ticks) { const line = el("span", "agent-gridline"); line.style.left = pct(start + offset) + "%"; line.setAttribute("aria-hidden", "true"); track.append(line); } };
      const addRow = (node, depth, parentSid = null) => {
        const s = SESS[node.sid]; if (!s) return;
        const handoff = node.handoff, from = node.kind === "root" ? start : handoff.at;
        const to = node.kind === "root" ? end : s.state === "work" ? NOW : handoff.done ?? s.last ?? from;
        const duration = dur(from, to), ownCost = costForSession(node.sid), rollup = costForSessions(allNodes(node).map((x) => SESS[x.sid]).filter(Boolean));
        const isCritical = !!handoff && critical.has(handoff.id);
        const row = el("button", "agent-row"); row.type = "button"; row.dataset.agentId = node.sid; row.dataset.depth = String(Math.min(3, depth)); row.dataset.critical = String(isCritical);
        const kind = s.kind ?? HARNESS[s.harness], status = STATE[s.state] ?? s.state;
        row.setAttribute("aria-label", [s.name, kind, status, duration, costText(rollup)].join(", "));
        const label = el("span", "agent-label"), identity = el("span", "agent-identity"), indent = el("span", "agent-indent"); indent.style.width = Math.min(3, depth) * 12 + "px";
        const harnessDot = el("span", "agent-harness-dot " + hcls(node.sid)); harnessDot.setAttribute("aria-hidden", "true");
        identity.append(indent, harnessDot);
        if (node.kind === "relay") { const relayIcon = icon(I.relay, "agent-relay-icon"); relayIcon.setAttribute("aria-hidden", "true"); identity.append(relayIcon); }
        identity.append(el("span", "agent-name", s.name));
        const meta = el("span", "agent-meta"); meta.append(el("span", "agent-model", [kindText(s), shortModel(s.model)].filter(Boolean).join(" · ")));
        const statusMark = el("span", "agent-status stat " + s.state); statusMark.dataset.tip = status; statusMark.append(dot(s.state)); meta.append(statusMark);
        const usage = allNodes(node).reduce((sum, n) => sum + usageTotal(SESS[n.sid] ?? {}), 0);
        const tokens = el("span", "agent-tokens", tok(usage / 1000000) + " tokens"); tokens.dataset.tip = "Session totals, including this row's spawned agents"; meta.append(tokens);
        label.append(identity, meta);
        const durationCell = el("span", "agent-duration", duration), costCell = el("span", "agent-cost", rollup.usd == null ? "—" : shortMoney(rollup.usd));
        costCell.dataset.tip = "Own cost: " + costText(ownCost);
        const track = el("span", "agent-track"); track.setAttribute("aria-hidden", "true"); axisGridlines(track);
        const intervals = (s.busy ?? []).map(([a, b]) => [Math.max(from, a), Math.min(to, b)]).filter(([a, b]) => b > a).sort((a, b) => a[0] - b[0]);
        let cursor = from;
        const segment = (a, b, type) => { if (b <= a) return; const bar = el("span", "agent-segment " + type); bar.style.left = pct(a) + "%"; bar.style.width = Math.max(.2, pct(b) - pct(a)) + "%"; track.append(bar); };
        for (const [a, b] of intervals) { if (a > cursor) segment(cursor, a, "idle"); segment(a, b, "busy"); cursor = Math.max(cursor, b); }
        if (to > cursor) segment(cursor, to, "idle");
        for (const child of node.children) { const tick = el("span", "agent-spawn-tick"); tick.style.left = pct(child.handoff.at) + "%"; tick.setAttribute("aria-hidden", "true"); track.append(tick); }
        for (const wait of waits.filter((w) => w.sid === node.sid)) {
          const bar = el("span", "agent-wait"); bar.style.left = pct(wait.start) + "%"; bar.style.width = Math.max(.2, pct(wait.end ?? end) - pct(wait.start)) + "%";
          bar.dataset.tip = wait.targets?.length ? "Waiting on " + wait.targets.map(nameOf).join(", ") : "Wait without a recorded target"; track.append(bar);
        }
        row.append(label, durationCell, costCell, track); row.addEventListener("click", () => goSession(node.sid, node.turn?.id));
        const related = (on) => { for (const edge of chart.querySelectorAll(".agent-edge, .agent-edge-arrow")) if (edge.dataset.parent === node.sid || edge.dataset.child === node.sid) edge.classList.toggle("hot", on); };
        row.addEventListener("pointerenter", () => related(true)); row.addEventListener("pointerleave", () => related(false));
        row.addEventListener("focus", () => related(true)); row.addEventListener("blur", () => related(false));
        chart.append(row); rows.set(node.sid, row);
        if (parentSid) edgeDefs.push({ parentSid, childSid: node.sid, handoff, critical: critical.has(handoff.id) });
        const fold = foldRows && node.children.length > 8 && !expandedMore.has(node.sid), visible = fold ? node.children.slice(0, 8) : node.children;
        for (const child of visible) addRow(child, Math.min(3, depth + 1), node.sid);
        if (fold) {
          const hidden = node.children.slice(8), more = el("button", "agent-row agent-more"); more.type = "button"; more.dataset.depth = String(Math.min(3, depth + 1));
          const moreLabel = el("span", "agent-label"), moreIdentity = el("span", "agent-identity"), moreIndent = el("span", "agent-indent"); moreIndent.style.width = Math.min(3, depth + 1) * 12 + "px";
          moreIdentity.append(moreIndent, el("span", "agent-name", "+" + hidden.length + " more")); moreLabel.append(moreIdentity);
          const hiddenIds = new Set(); for (const n of hidden) for (const x of allNodes(n)) hiddenIds.add(x.sid);
          const hiddenCost = costForSessions([...hiddenIds].map((sid) => SESS[sid]).filter(Boolean));
          const moreCost = el("span", "agent-cost", hiddenCost.usd == null ? "—" : shortMoney(hiddenCost.usd)); moreCost.dataset.tip = "Hidden agents' rollup cost: " + costText(hiddenCost);
          more.setAttribute("aria-label", "Show " + hidden.length + " more agents under " + s.name);
          more.append(moreLabel, el("span", "agent-duration"), moreCost, el("span", "agent-track"));
          more.addEventListener("click", () => { expandedMore.add(node.sid); drawRows(); }); chart.append(more);
        }
      };
      addRow(model.rootNode, 0);
      if (model.peers.length) {
        const heading = el("div", "agent-relayed-heading", "Relayed to"); chart.append(heading);
        for (const peer of model.peers) addRow(peer, 0);
      }
      const edges = svgEl("svg", { class: "agent-edges", "aria-hidden": "true", preserveAspectRatio: "none" }); chart.append(edges);
      const drawEdges = () => {
        if (!chart.isConnected || !edges.isConnected) return;
        layoutAxisLabels();
        const rect = chart.getBoundingClientRect(), width = rect.width, height = rect.height;
        edges.setAttribute("viewBox", "0 0 " + width + " " + height); edges.replaceChildren();
        if (!window.matchMedia("(min-width: 761px)").matches || !width || !height) return;
        for (const edge of edgeDefs) {
          const parent = rows.get(edge.parentSid), child = rows.get(edge.childSid); if (!parent || !child) continue;
          const parentTrack = parent.querySelector(".agent-track"), childTrack = child.querySelector(".agent-track");
          const pr = parent.getBoundingClientRect(), cr = child.getBoundingClientRect(), ptr = parentTrack.getBoundingClientRect(), ctr = childTrack.getBoundingClientRect();
          const yParent = pr.top - rect.top + pr.height / 2, yChild = cr.top - rect.top + cr.height / 2;
          const xAt = ptr.left - rect.left + ptr.width * pct(edge.handoff.at) / 100;
          const endAt = edge.handoff.done ?? (SESS[edge.childSid].state === "work" ? NOW : SESS[edge.childSid].last);
          const xDone = ctr.left - rect.left + ctr.width * pct(endAt) / 100;
          const klass = "agent-edge" + (edge.critical ? " critical" : "");
          const related = { "data-parent": edge.parentSid, "data-child": edge.childSid };
          edges.append(svgEl("path", { ...related, class: klass, d: "M " + xAt + " " + yParent + " V " + yChild }));
          edges.append(svgEl("path", { ...related, class: klass, d: "M " + xAt + " " + (yParent - 4) + " V " + (yParent + 4) }));
          if (!Number.isFinite(edge.handoff.done)) continue; // A return arrow needs a recorded return.
          edges.append(svgEl("path", { ...related, class: klass, d: "M " + xDone + " " + yChild + " V " + (yParent + 4) }));
          edges.append(svgEl("path", { ...related, class: "agent-edge-arrow" + (edge.critical ? " critical" : ""), d: "M " + (xDone - 3) + " " + (yParent + 4) + " L " + xDone + " " + yParent + " L " + (xDone + 3) + " " + (yParent + 4) }));
        }
      };
      requestAnimationFrame(drawEdges);
      document.fonts.ready.then(() => requestAnimationFrame(drawEdges));
      if (typeof ResizeObserver !== "undefined") { runChartObserver?.disconnect(); runChartObserver = new ResizeObserver(drawEdges); runChartObserver.observe(chart); }
    };
    drawRows();
    return panel;
  }
  function renderTrace(page, id) {
    const root = TURN.get(id);
    const head = el("div", "ph sr"); const h1 = el("h1", null, "Trace"); head.append(h1); page.append(head); observeTitle(h1);
    if (!root) { page.append(el("p", "empty", "This turn isn't in the logs on this machine.")); return; }
    const readTrace = (turn, visited = new Set()) => {
      if (!turn || visited.has(turn.id)) return;
      visited.add(turn.id); markSeenResults(turn.out);
      for (const h of turn.out) if (h.kind === "spawn" || h.kind === "relay") readTrace(STARTS.get(h.id), visited);
    };
    readTrace(root);
    const flow = el("div", "flow"), seen = new Set([root.id]), sess = new Set([root.sid]); let n = 0;
    // Every session the trace names, so a machine shows only when the trace spans several.
    const scope = new Set([root.sid]), reached = new Set([root.id]);
    const reach = (t) => { for (const h of t.sent) { scope.add(h.kind === "toyou" ? h.from : h.to); const c = h.kind === "spawn" || h.kind === "relay" ? STARTS.get(h.id) : null; if (c && !reached.has(c.id)) { reached.add(c.id); reach(c); } } };
    reach(root);
    const meta = (body, st, text, sid, turn, note) => traceMeta(body, st, text, sid, turn, note, scope);
    const hop = (cls, ic, hc, parts, at) => { const x = el("div", "hop " + cls); const node = el("div", "node " + hc); node.append(icon(ic)); const body = el("div", "body"); const sent = el("div", "sent"); sent.append(...parts); if (at != null) sent.append(el("span", "tm", clock(at))); body.append(sent); x.append(node, body); flow.append(x); return [x, body]; };
    const s0 = root.start, text = s0 ? s0.brief : root.u?.text;
    const [ic0, parts0] = s0 ? sentence(s0, null) : root.u ? sentence({ kind: "ask", to: root.sid }, null) : [I.more, [el("span", "who", SESS[root.sid].name), el("span", "verb", " · a turn whose start isn't in these logs")]];
    const [r0, b0] = hop("k-root", ic0, hcls(s0 ? s0.from : root.u ? "you" : root.sid), parts0, s0?.at); r0.dataset.turn = root.id; if (s0) r0.dataset.h = s0.id;
    if (text) clampBrief(b0, text);
    const e0 = turnEnd(root); meta(b0, e0?.st ?? "idle", e0?.text ?? "Nothing recorded", root.sid, root);
    const walk = (t) => {
      for (const h of t.sent) {
        const result = h.kind === "toyou" && h.ask === "result";
        const c = h.kind === "spawn" || h.kind === "relay" ? STARTS.get(h.id) : null, tgt = h.kind === "toyou" ? h.from : h.to, [ic, parts] = result ? [I.result, [el("span", "verb", statWord(h))]] : sentence(h, null);
        const [x, b] = hop("child k-" + h.kind + " s-" + h.status + (c || h.kind === "toyou" || h.kind === "move" ? "" : " stub"), ic, hcls(tgt), parts, h.at); x.dataset.h = h.id; if (c) x.dataset.turn = c.id;
        if (result) { n++; continue; }
        clampBrief(b, h.brief); const an = answerEl(h, "result"); if (an) b.append(an);
        if (h.result) { const r = el("div", "result"); r.append(el("span", "rl", "Result:")); const s = el("span"); inline(s, h.result); r.append(s); b.append(r); }
        if (h.kind === "move") { meta(b, "done", "Moved", h.to, t); continue; }
        n++;
        if (h.kind === "toyou") { meta(b, isResult(h) ? SEEN_RESULTS.has(h.id) ? "read" : "new" : h.status === "done" ? "done" : h.status, statWord(h), h.from, t); continue; }
        sess.add(h.to); const e = c && turnEnd(c);
        meta(b, e ? e.st : h.status === "done" ? "done" : h.status, e ? e.text : statWord(h), h.to, c, c ? null : "Its turn isn't in these logs");
        if (c && !seen.has(c.id)) { seen.add(c.id); walk(c); } // a turn is drawn once, so a loop in the data can't recurse forever
      }
    };
    walk(root);
    page.append(renderAgents(root), flow);
    // The bar's summary line.
    return [sess.size + (sess.size === 1 ? " session" : " sessions"), n + (n === 1 ? " handoff" : " handoffs"), [...machineLabels(scope).values()].join(", ")].filter(Boolean).join(" · ");
  }

  // ---- Session page --------------------------------------------------------------------------------------------------
  function renderSession(page, sid) {
    markSeenResults(H.filter((h) => isResult(h) && h.from === sid));
    const s = SESS[sid], origin = originHandoff(sid), head = el("div", "ph sr"); const h1 = el("h1", null, s.name); head.append(h1); page.append(head); observeTitle(h1);
    // The jump button sits in the transcript's own grid, so its row adds no page gap when it appears.
    const t = transcript(sid); t.append(jumpWrap); page.append(t);
    if (showsFooter(s, origin)) page.append(sessionFooter(s, origin));
  }
  // Whether a session page ends in its status line: a child's when it is running or has returned, any other session's always.
  // renderSession and patchSession share it.
  const showsFooter = (s, origin) => origin ? s.state === "work" || s.state === "done" || s.state === "err" || origin.status === "done" || origin.status === "err" : !s.stub && !s.role && s.state in STATE;

  const thoughtText = (e) => String(e.text ?? "").trim();
  const isPendingThought = (e, entries, i, sid) => !!(e.pending || e.status === "thinking") ||
    Array.isArray(entries) && e.k === "think" && !thoughtText(e) && i === entries.length - 1 && SESS[sid]?.state === "work";
  const signalLabel = (e) => {
    const s = e.signal ?? {}, tag = s.tag ?? "", kind = s.kind;
    if (kind === "compact") return "Context compacted" + (s.value != null ? " · " + compactCount(s.value) + " tokens before" : "");
    if (kind === "interrupt") return "Interrupted by you";
    if (kind === "denial") return (s.tool ?? "Tool call") + " denied";
    if (kind === "hook") return "Hook " + (tag.includes("block") ? "blocked" : tag || "ran") + (s.tool ? " " + s.tool : "");
    const name = {model:"Model", effort:"Effort", approval:"Approval policy", sandbox:"Sandbox", permission:"Permission mode"}[kind] ?? "Signal";
    return name + (s.previous ? " " + s.previous + " → " : " → ") + tag;
  };
  const isMaskedThought = (e, entries, i, sid) => e.k === "think" && !isPendingThought(e, entries, i, sid) && !thoughtText(e);
  function thoughtSeconds(e) {
    if (Number.isFinite(e?.secs) && e.secs >= 0) return e.secs;
    if (typeof e?.secs === "string") { const m = /^(\d+(?:\.\d+)?)s?$/.exec(e.secs.trim()); if (m) return Number(m[1]); }
    return null;
  }
  // The label above a thought. The log holds no measured thinking time: the duration is the gap between the log timestamps of
  // this record and the one before it, which rounds to 0 s whenever the two were written together. So a duration is named only
  // once it reaches a second ("Thinking · 12s"); shorter, the label says just "Thinking", never "Thought for 0s".
  const thoughtLabel = (secs) => { const n = Number.isFinite(secs) ? Math.round(secs) : 0; return n < 1 ? "Thinking" : "Thinking · " + (n >= 60 ? Math.floor(n / 60) + "m " + (n % 60) + "s" : n + "s"); };
  // A masked thought (Claude redacts its thinking, Codex encrypts its reasoning) is kept in the list, so entry keys and turn
  // starts still line up with the index, and the page draws one quiet line for it.
  // Join adjacent Codex snapshots with the same resolved owner; unknown explicit turns block merging until ownership resumes.
  function transcriptEntries(entries, sid) {
    const out = [], codex = SESS[sid]?.harness === "codex";
    const turns = codex ? TURNS[sid] ?? [] : [], owners = codex ? new Map(turns.flatMap((t) => t.entries.map((e) => [e.key, t.id]))) : null;
    const turnIds = codex ? new Set(turns.map((t) => t.id)) : null;
    let chain = null, sawOwnedEntry = false, unresolvedOwner = false;
    for (let i = 0; i < entries.length; i++) {
      const e = entries[i], indexedOwner = codex ? owners.get(e.key) : null;
      const turn = codex ? indexedOwner ?? (turnIds.has(e.turn) ? e.turn : null) : null;
      if (codex && indexedOwner == null && e.turn != null && !turnIds.has(e.turn)) { unresolvedOwner = true; chain = null; }
      else if (turn !== null) { sawOwnedEntry = true; unresolvedOwner = false; }
      if (e.k !== "think") { out.push(e); chain = null; continue; }
      const pending = isPendingThought(e, entries, i, sid);
      const row = { ...e, ...(pending ? { pending: true } : {}), displaySecs: thoughtSeconds(e) };
      const text = thoughtText(row);
      if (!codex || pending || !text || unresolvedOwner || (turn === null && sawOwnedEntry)) { out.push(row); chain = null; continue; }
      if (chain && chain.turn === turn && (text === chain.text || text.startsWith(chain.text + "\n") || text.startsWith(chain.text + "\r\n"))) {
        const merged = { ...chain.row, text, displaySecs: row.displaySecs };
        out[chain.index] = merged; chain = { ...chain, row: merged, text };
      } else { out.push(row); chain = { index: out.length - 1, row, text, turn }; }
    }
    return out;
  }

  // What a tool call did: an icon and verb for its step row, and a phrase and nouns for a group summary
  // ("ran 2 commands, asked you 1 question"). An unknown tool keeps its own name ("TodoWrite 1 step").
  const RUN = ["run", "Ran", "ran", "command", "commands"], FIND = ["find", "Searched for", "searched", "time", "times"];
  const TOOLS = { Bash: RUN, shell: RUN, exec_command: RUN, local_shell: RUN, write_stdin: ["run", "Sent input to", "sent input to", "time", "times"], Grep: FIND, Glob: FIND,
    Read: ["read", "Read", "read", "file", "files"], Edit: ["edit", "Edited", "edited", "file", "files"], MultiEdit: ["edit", "Edited", "edited", "file", "files"],
    Write: ["edit", "Wrote", "wrote", "file", "files"], apply_patch: ["edit", "Patched", "patched", "file", "files"], NotebookEdit: ["edit", "Edited", "edited", "notebook", "notebooks"],
    AskUserQuestion: ["q", "Asked you", "asked you", "question", "questions"], ToolSearch: ["find", "Loaded", "loaded", "tool", "tools"],
    SendMessage: ["out", "Sent", "sent", "message", "messages"], SendUserFile: ["out", "Sent you", "sent you", "file", "files"], Agent: ["out", "Started", "started", "agent", "agents"], Task: ["out", "Started", "started", "agent", "agents"],
    Monitor: ["now", "Watched", "watched", "process", "processes"], ScheduleWakeup: ["now", "Scheduled", "scheduled", "wake-up", "wake-ups"], TaskStop: ["x", "Stopped", "stopped", "task", "tasks"],
    Artifact: ["ext", "Published", "published", "page", "pages"], WebFetch: ["ext", "Fetched", "fetched", "page", "pages"], WebSearch: ["search", "Searched the web for", "searched the web", "time", "times"],
    Skill: ["stack", "Used skill", "used", "skill", "skills"] };
  const toolInfo = (name) => { if (TOOLS[name]) return TOOLS[name]; const m = /^mcp__(.+?)__/.exec(name); if (m) { const srv = m[1].replace(/^claude_ai_/, "").replace(/_/g, " "); return ["ext", "Used " + srv, "used " + srv, "time", "times"]; } return ["run", name, name, "step", "steps"]; };
  const verb = (name) => toolInfo(name).slice(0, 2);
  const NOW_VERB = { Ran: "Running", Read: "Reading", Edited: "Editing", Patched: "Patching", Wrote: "Writing", "Searched for": "Searching for" };
  const verbNow = (name) => NOW_VERB[verb(name)[1]] ?? verb(name)[1];
  const isCmd = (name) => /^(Bash|shell|exec_command|local_shell)$/.test(name);
  // A background step's outcome ("running 8m 18s", "exit 0 · 1m 0s", "failed · 2m 0s"). Its `.sd` puts "background · " before it,
  // in a span a phone hides in favour of a marker before the verb, so the outcome is never what gets cut.
  const backgroundText = (bg) => {
    if (bg.state === "running") return "running " + bg.secs;
    if (bg.state === "unknown") return "no end recorded";
    const outcome = bg.state === "failed" ? "failed" : bg.state === "killed" ? "stopped" : bg.exit != null ? "exit " + bg.exit : "done";
    return outcome + (bg.secs ? " · " + bg.secs : "");
  };
  const elapsedMs = (text) => { const m = /^(?:(\d+)m )?(\d+(?:\.\d+)?)s$/.exec(text ?? ""); return m ? (Number(m[1] ?? 0) * 60 + Number(m[2])) * 1000 : null; };
  function transcript(sid, opts = {}) {
    const sec = el("section", "transcript"); sec.setAttribute("aria-label", "Transcript");
    const entries = transcriptEntries(TX[sid] ?? [], sid);
    const turnMode = true;
    // The transcript is a list of turns, each with its own entries.
    const box = el("div", "turns"); let tx = box; const hit = (s) => !find || s.toLowerCase().includes(find);
    const filtering = !!find || !show.messages || !show.tools || !show.thinking;
    // Slots identify the same entry even when an earlier page extends its turn and its turn-relative data-e key moves.
    const entryKey = (e) => e.slot != null ? sid + "#slot:" + e.slot : e.key;
    const keyed = (n, e) => { if (e.key) { n.dataset.e = e.key; n.dataset.entryKey = entryKey(e); } return n; };
    const range = turnMode ? TXM[sid] : null;
    if (range?.from > 0) box.append(pager(sid, "before", "Load earlier"));
    else if (!filtering) box.append(startedDivider(sid));
    // Adjacent tool calls collapse into one summary line ("Ran 2 commands, read 1 file · 1 failed"),
    // expandable to the individual steps. A lone call stays a single line; while finding, matches show directly.
    const endedCalls = new Set(entries.filter((e) => e.k === "bgend").map((e) => e.call));
    let run = []; const maskedIn = new WeakSet();
    const flush = () => {
      if (!run.length) return;
      const steps = el("div", "steps" + (run.length === 1 && !filtering ? " lone" : "")); run.forEach((r) => steps.append(r.node));
      if (run.length === 1 || filtering) { tx.append(steps); run = []; return; }
      const ends = run.filter((r) => r.end);
      const counts = new Map(); for (const r of run.filter((r) => !r.end)) { const [, , p, one, many] = toolInfo(r.k), c = counts.get(p) ?? { n: 0, one, many }; c.n++; counts.set(p, c); }
      // With finished background commands: "Finished 3 background commands (1 failed, 1 stopped), ran 4 commands". A phone hides
      // the long words (.long) and lets it take two lines, "3 background (1 failed, 1 stopped), 4 commands", so the counts show.
      const tt = el("span", "tt");
      if (ends.length) {
        const outcomes = [[ends.filter((r) => r.state === "failed").length, "failed"], [ends.filter((r) => r.state === "killed").length, "stopped"]].filter(([n]) => n).map(([n, word]) => n + " " + word);
        tt.classList.add("bgsum"); tt.append(el("span", "long", "Finished "), ends.length + " background", el("span", "long", " command" + (ends.length === 1 ? "" : "s")));
        if (outcomes.length) tt.append(el("span", "tt-counts", " (" + outcomes.join(", ") + ")"));
        for (const [p, c] of counts) tt.append(", ", p === "ran" ? el("span", "long", "ran ") : p + " ", c.n + " " + (c.n === 1 ? c.one : c.many));
      } else { const text = [...counts].map(([p, c]) => p + " " + c.n + " " + (c.n === 1 ? c.one : c.many)).join(", "); tt.textContent = text[0].toUpperCase() + text.slice(1); }
      const failed = run.filter((r) => !r.end && r.err && (!r.bg || !endedCalls.has(r.tid))).length;
      const live = run.some((r) => r.bg) ? run.filter((r) => r.live).sort((a, b) => (elapsedMs(b.secs) ?? 0) - (elapsedMs(a.secs) ?? 0))[0] : run.find((r) => r.live);
      const g = el("div", "tgroup"); if (run[0].key) { g.dataset.e = "g:" + run[0].key; g.dataset.entryKey = "g:" + run[0].node.dataset.entryKey; } const b = el("button", "tsum"); b.type = "button"; b.setAttribute("aria-expanded", "false");
      b.append(live ? el("span", "spin") : icon(I.stack), tt);
      if (failed) b.append(el("span", "tf", "· " + failed + " failed"));
      if (live) b.append(el("span", "tl tick", "· running " + live.secs));
      b.append(icon(I.chev, "chev"));
      steps.hidden = true;
      b.addEventListener("click", () => { steps.hidden = !steps.hidden; b.setAttribute("aria-expanded", String(!steps.hidden)); });
      g.append(b);
      g.append(steps); tx.append(g); run = [];
    };
    // A turn block: who started it, the work, and how it ended. While finding or filtering, a turn left with nothing drops out.
    const firsts = turnMode ? new Map((TURNS[sid] ?? []).filter((t) => t.entries[0]?.key).map((t) => [t.entries[0].key, t])) : new Map(); let cur = null;
    // A live update draws only the turns that changed (opts.only, by turn id).
    const owner = opts.only ? new Map((TURNS[sid] ?? []).flatMap((t) => t.entries.map((e) => [e.key, t.id]))) : null;
    const CONTENT = ".msg, .bubble, .step, .event, .child-card, .thought, .think-pending, .harness-note, .tgroup";
    const closeTurn = () => { flush(); if (!cur) return; const { t, blk, masked } = cur; cur = null; tx = box;
      // A turn whose only thinking was masked is left with nothing to draw (no header, no rows): it goes, unless its foot still says something.
      const bare = masked && !blk.querySelector(".tx > *, .turn-h");
      const end = turnEnd(t), returned = t.entries.some((e) => e.k === "end" && /^Returned to /.test(e.text ?? ""));
      const foot = !filtering && ((end && end.st === "err" && !returned) || t.out.length);
      if (bare && !foot) { blk.remove(); return; }
      if (bare) blk.querySelector(":scope > .tx")?.remove(); // its foot stays alone, with no empty space above it
      if (filtering && !blk.querySelector(CONTENT)) { blk.remove(); return; }
      // A turn's foot only says what the rows above don't: that it stopped on a failure, and where its trace is.
      if (foot) {
        const d = el("div", "turn-end");
        if (end?.st === "err" && !returned) d.append(stateLabel("err", end.text));
        if (t.out.length) { const tb = btn("link", null, "Trace what this turn set off"); tb.append(icon(I.trace), el("span", null, "Trace")); tb.addEventListener("click", () => goTrace(t.id)); d.append(tb); }
        blk.append(d);
      } };
    const openTurn = (t) => { closeTurn(); const blk = el("section", "turn"); blk.dataset.turn = t.id;
      // Your own message needs no header: the bubble is yours and its time sits under it. A relay or brief says who sent it.
      const h = t.start;
      if (t.u || h?.kind === "ask") blk.setAttribute("aria-label", "Your message" + (h ? " at " + clock(h.at) : ""));
      else if (h) { const hd = el("div", "turn-h"); const [, parts] = sentence(h, sid, true); const sender = SESS[h.from] && harnessIcon(SESS[h.from].harness, { size: 16 }); if (sender) hd.append(sender); hd.append(...parts, el("span", "tm", clock(h.at))); blk.append(hd); }
      tx = el("div", "tx"); blk.append(tx); box.append(blk); cur = { t, blk }; };
    const toolStep = (e, v, ic, live) => {
      // A background call's launch returns at once: its own lifecycle (e.bg) says whether it runs, failed or ended.
      const bg = e.bg, bgRunning = bg?.state === "running";
      const box = keyed(el("div", "step" + (bg ? (bg.state === "failed" ? " err" : "") + " background" + (bgRunning ? " background-running" : "") : live ? " live" : e.ok || e.ok === null ? "" : " err")), e);
      if (e.tid) box.dataset.tid = e.tid;
      if (live) { box.dataset.live = sid; if (e.since != null) box.dataset.since = e.since; }
      else if (bgRunning) { box.dataset.live = sid; if (bg.since != null) box.dataset.since = bg.since; }
      const b = el("button"); b.type = "button"; b.setAttribute("aria-expanded", "false");
      const bgSecs = bgRunning && bg.since != null ? running(NOW - bg.since) : bg?.secs;
      const waiting = live && SESS[sid]?.state === "wait", waitingText = SESS[sid]?.waiting_for?.includes("permission") ? "Waiting on permission" : "Waiting for your input";
      const status = bg ? null : waiting ? waitingText : live ? e.secs : e.unfinished ? "no result" : e.exit != null ? "exit " + e.exit + " · " + e.secs : e.ok ? e.secs : e.ok === null ? "exit unknown · " + e.secs : "failed · " + e.secs;
      const title = e.title ? String(e.title) : null;
      const command = e.in ?? e.arg;
      const firstNonemptyLine = typeof command === "string" ? command.split(/\r\n|\n|\r/).find((line) => line.trim()) : null;
      const label = title ? el("span", "sa st", title) : el("code", "sa", e.arg);
      if (title) {
        // Keep the title and changing status in the native accessible name.
        b.append(el("span", "sr-only", waiting ? waitingText + ": " : live ? "Running: " : e.ok === false ? "Failed: " : v + ": "));
        if (firstNonemptyLine != null) label.setAttribute("data-tip", firstNonemptyLine.slice(0, 200));
      }
      const sd = el("span", "sd" + (live || bgRunning ? " tick" : ""), status);
      b.append(live || bgRunning ? el("span", "spin") : icon(I[ic]));
      // A background step: "background · " then its outcome in `.sd`; a phone hides the word for the ring before the label.
      if (bg) { const mark = el("span", "bgmark"); mark.setAttribute("role", "img"); mark.setAttribute("aria-label", "background"); b.append(mark); sd.append(el("span", "bgw", "background · "), el("span", "bgo", backgroundText({ ...bg, secs: bgSecs }))); }
      if (!title) b.append(el("span", "sv", waiting ? "Waiting" : live ? verbNow(e.name) : v));
      b.append(label, sd, icon(I.chev, "chev"));
      // Build call details only on demand; collapsed transcripts remain inexpensive.
      let out = null;
      b.addEventListener("click", () => {
        if (!out) {
          out = stepDetail(e, v, ic); out.hidden = true;
          if (bg?.summary) out.append(el("div", "io", "Finished"), el("pre", "finished", bg.summary));
          box.append(out);
        }
        out.hidden = !out.hidden; b.setAttribute("aria-expanded", String(!out.hidden));
      });
      box.append(b); return box;
    };
    for (const e of entries) {
      if (owner && !opts.only.has(owner.get(e.key))) continue;
      if (turnMode && isGap(e)) { closeTurn(); if (!filtering) box.append(el("div", "divider", e.text)); continue; }
      if (turnMode && firsts.has(e.key)) openTurn(firsts.get(e.key));
      // Entries that render nothing (empty thinking, hidden kinds) must not split a run of tool calls.
      if (e.k === "think" && (!show.thinking || find)) continue;
      if (e.k === "signal") { const label = signalLabel(e); if (hit(label)) tx.append(keyed(el("div", "signal-marker", label), e)); continue; }
      // Masked thinking is one quiet line per turn (per list, when nested), at the first masked thought's place. Later ones draw
      // nothing, and the line never splits a run of steps: drawn before flush(), it lands ahead of a run still being gathered.
      if (e.k === "think" && isMaskedThought(e)) {
        const scope = cur ?? box;
        if (!maskedIn.has(scope)) { maskedIn.add(scope); const m = keyed(el("div", "thought masked"), e); m.append(el("div", "think-label", "Thinking hidden by the harness")); tx.append(m); }
        continue;
      }
      if (e.k === "bgend") {
        if (!show.tools || !hit(e.label ?? "")) continue;
        const word = e.state === "failed" ? "failed" : e.state === "killed" ? "stopped" : "completed";
        const row = keyed(el("div", "step bgend" + (e.state === "failed" ? " err" : "")), e);
        const loaded = entries.some((entry) => entry.k === "tool" && entry.tid === e.call);
        const line = el(loaded ? "button" : "div", "bg-line");
        line.append(icon(I.run), el("span", "bg-label", "Background command " + word + " · " + (e.label ?? "")));
        if (loaded) {
          line.type = "button";
          line.addEventListener("click", () => {
            const target = line.closest('section[aria-label="Transcript"], section.nested')?.querySelector('.step[data-tid="' + CSS.escape(e.call) + '"]'); if (!target) return;
            stopOpeningEndPin();
            for (let parent = target.parentElement; parent; parent = parent.parentElement) {
              const toggle = opener(parent) ?? (parent.classList.contains("turn") ? parent.querySelector(':scope > button[aria-expanded]') : null);
              if (toggle?.getAttribute("aria-expanded") === "false") toggle.click();
            }
            centre(target); target.classList.add("flash"); setTimeout(() => target.classList.remove("flash"), 1500);
          });
        }
        row.append(line); run.push({ node: row, end: true, state: e.state, key: e.key }); continue;
      }
      if (e.k === "tool") {
        if (!show.tools || !hit(e.name + " " + (e.title ?? "") + " " + e.arg + " " + (e.in ?? "") + " " + (e.out ?? ""))) continue;
        const [ic, v] = verb(e.name);
        const box = toolStep(e, v, ic, !!e.live && !e.bg);
        if (e.live && !e.bg) run.push({ node: box, v, k: e.name, live: true, secs: e.secs, key: e.key });
        else if (e.bg) { const bgRunning = e.bg.state === "running"; run.push({ node: box, v, k: e.name, err: e.bg.state === "failed", bg: true, tid: e.tid, live: bgRunning, secs: bgRunning && e.bg.since != null ? running(NOW - e.bg.since) : e.bg.secs ?? e.secs, key: e.key }); }
        else run.push({ node: box, v, k: e.name, err: e.ok === false, key: e.key });
        continue;
      }
      flush();
      if (e.k === "u") { if (!show.messages || !hit(e.text)) continue; const m = keyed(el("div", "msg user"), e); userBody(m, e, e.text); tx.append(m); }
      else if (e.k === "a") { if (!show.messages || !hit(e.text)) continue; const m = keyed(el("div", "msg assistant"), e); m.append(markdown(e.text)); tx.append(m); }
      else if (e.k === "think") {
        if (isPendingThought(e)) { const pending = keyed(el("div", "think-pending"), e); pending.append(el("span", "spin"), el("span", null, "Thinking…")); tx.append(pending); }
        else {
          // Readable thinking sits in the flow, in full and quietly styled, with nothing to open.
          const group = keyed(el("div", "thought"), e);
          group.append(el("div", "think-label", thoughtLabel(e.displaySecs)), markdown(thoughtText(e), "think-text"));
          tx.append(group);
        }
      }
      else if (e.k === "harness") { if (!show.messages || find) continue; tx.append(keyed(el("div", "harness-note", "Harness text added before the prompt (" + e.label + ")"), e)); }
      else if (e.k === "end") { if (filtering) continue; tx.append(keyed(el("div", "divider", e.text), e)); }
      else if (e.k === "h") {
        // A page before the model's window can name a handoff the model no longer holds: it draws nothing.
        const h = HID.get(e.id); if (!h || !hit(h.brief + " " + (h.result ?? ""))) continue;
        // Your own ask is simply your message.
        if (h.kind === "ask") { if (!show.messages) continue; const m = keyed(el("div", "msg user"), e); userBody(m, e, h.brief); tx.append(m); if (cur?.t.start === h) tx.append(el("div", "msg-tm", clock(h.at))); continue; }
        // A relay or brief that starts a turn is that turn's incoming message, under the header that names its sender.
        if (cur && cur.t.start === h) { if (!show.messages) continue; const m = keyed(el("div", "bubble in"), e); m.dataset.h = h.id; m.append(markdown(h.brief)); tx.append(m); continue; }
        // A session this one started is a card that opens it.
        if (h.kind === "spawn" && h.from === sid && SESS[h.to]) { if (!show.tools) continue; tx.append(keyed(childCard(h, SESS[h.to]), e)); continue; }
        if (find && h.kind === "move") continue;
        if (!show.messages && h.kind !== "move") continue;
        tx.append(keyed(eventRow(h, sid), e));
      }
    }
    closeTurn();
    if (range && range.to < range.total) box.append(pager(sid, "after", "Load later"));
    if (!box.querySelector(CONTENT)) box.append(el("p", "empty", find ? "Nothing matches “" + find + "”." : "Nothing to show with these filters."));
    sec.append(box); return sec;
  }
  // Codex cuts some outputs before the model sees them and says where and how much. An output cut that way is drawn as its
  // head, a divider with the count, then its tail; without a found gap it is one block with the note alone.
  const num = (n) => n.toLocaleString("en-US");
  const gapText = (g) => (g.unit === "tokens" ? "About " : "") + (g.unit === "lines" && g.of != null ? num(g.n) + " of " + num(g.of) + " lines" : num(g.n) + " " + (g.unit === "chars" ? "characters" : g.unit)) + " cut here by Codex";
  const cutNoteText = (cut) => "Codex cut this output before the model saw it" + (cut.original_tokens ? " (about " + num(cut.original_tokens) + " tokens in all)" : "") + ".";
  function outEl(e, cls) {
    const parts = e.cut?.parts;
    if (!parts?.length) return el("pre", cls, e.out);
    const box = el("div", "cutout" + (cls ? " " + cls : ""));
    for (const p of parts) { if (p.gap) box.append(el("div", "cutgap", gapText(p.gap))); else box.append(el("pre", null, p.text)); }
    return box;
  }
  const diffEl = (rows, cls) => { const d = el("div", "diff" + (cls ? " " + cls : "")); rows.forEach(([c, t]) => d.append(el("div", c, t))); return d; };
  // The whole tool call. A phone gets a full-screen sheet and a wider screen a dialog; either way it is a history entry,
  // so the back gesture closes it without leaving the page.
  let viewerEl = null, skipPop = false;
  // What a step shows opened: what was asked first (the command, the file, the input), then what came back. A failed command
  // shows the end of its output, where the failure is; anything else shows the start. "View all" opens the whole call.
  const PREVIEW_LINES = 12;
  const stateLabel = (st, text) => { const x = el("span", "state " + st); x.append(st === "work" && text !== STATE.work ? el("span", "spin") : dot(st), el("span", null, text ?? STATE[st] ?? st)); return x; };
  function copyBtn(text) { const c = btn("link copy", null, "Copy"); c.append(icon(I.copy), el("span", null, "Copy")); c.addEventListener("click", () => navigator.clipboard?.writeText(text).then(() => { c.lastChild.textContent = "Copied"; }, () => { c.lastChild.textContent = "Copy failed"; })); return c; }
  function stepDetail(e, v, ic) {
    const out = el("div", "out"), ioHead = (label, text) => { const h = el("div", "io"); h.append(el("span", null, label)); if (text) h.append(copyBtn(text)); return h; };
    const cmd = e.in ?? (isCmd(e.name) ? e.arg : null);
    // A long command stops at the preview height and fades (the text stays whole in the page); "View all" opens the whole call (#178).
    const cmdLines = typeof cmd === "string" ? cmd.split("\n") : [], inCut = cmdLines.length > PREVIEW_LINES, inLabel = cmd ? (isCmd(e.name) ? "Command" : "Input") : "Input";
    const viewAll = (label) => { const all = btn("viewall"); all.append(icon(I.expand), el("span", null, label)); all.addEventListener("click", () => openStepViewer(e, v, ic, inLabel)); out.append(all); };
    if (cmd) out.append(ioHead(isCmd(e.name) ? "Command" : "Input", cmd), el("pre", "in" + (inCut ? " clip clipped" : ""), cmd));
    else if (!e.diff && !e.changes) out.append(ioHead(/^(Read|Grep|Glob)$/.test(e.name) ? (e.name === "Read" ? "File" : "Pattern") : "Input"), el("pre", "in", e.arg));
    if (e.cwd && e.cwd !== ".") out.append(ioHead("Working directory · " + e.cwd));
    const actions = () => {
      if (e.script != null) { const sv = btn("viewall viewscript"); sv.append(icon(I.expand), el("span", null, "View script")); sv.addEventListener("click", () => openScript(e)); out.append(sv); }
    };
    if (e.changes) {
      for (const change of e.changes) { out.append(ioHead("Change · " + change.path + (change.move ? " → " + change.move : ""))); if (change.diff?.length) out.append(diffEl(change.diff)); else out.append(el("div", "noout", "No diff recorded")); }
      if (!e.changes.length) out.append(el("div", "noout", "No changes recorded"));
      if (inCut) viewAll("View all"); actions(); return out;
    }
    if (e.diff) { out.append(ioHead("Change · " + e.arg), diffEl(e.diff)); if (inCut) viewAll("View all"); actions(); return out; }
    if (!e.out) { out.append(ioHead("Output"), el("div", "noout", e.live ? (SESS[e.sid]?.state === "wait" ? "Waiting for your input or permission · no output yet" : "Running · no output yet") : e.unfinished ? "No result recorded: the machine stopped responding while this ran." : "No output")); if (inCut) viewAll("View all"); actions(); return out; }
    const lines = e.out.split("\n"), cut = lines.length > PREVIEW_LINES, tail = e.ok === false, parts = !!e.cut?.parts?.length;
    const shown = !cut || parts ? lines : tail ? lines.slice(-PREVIEW_LINES) : lines.slice(0, PREVIEW_LINES), more = !!e.more?.length;
    out.append(ioHead(parts || !cut ? "Output" : (tail ? "Output · last " : "Output · first ") + PREVIEW_LINES + (more ? "" : " of " + lines.length) + " lines"));
    if (parts) out.append(outEl(e)); else out.append(el("pre", null, shown.join("\n")));
    if (e.cut) out.append(el("div", "cutnote", cutNoteText(e.cut)));
    else if ([e.in, e.out].some((t) => /…(\(truncated\))?\s*$/.test(t ?? ""))) out.append(el("div", "cutnote", "Cut short in this copy of the logs"));
    if ((cut && !parts) || more || inCut) viewAll(more || parts || inCut ? "View all" : "View all " + lines.length + " lines");
    actions(); return out;
  }
  // The whole call, in a full sheet: what the preview cut, fetched from the server when it is longer than the preview.
  function openStepViewer(e, v, ic, inLabel) {
    if (e.more?.length && e.slot != null && !e.full) { const open = (f) => openStepViewer({ ...e, ...f, full: true }, v, ic, inLabel); fullOf(e).then(open, () => open({ fullFailed: true })); return; }
    const status = e.live ? "Running · " + e.secs : e.unfinished ? "No result" : e.ok ? "Done · " + e.secs : e.ok === null ? "Exit unknown · " + e.secs : "Failed · " + e.secs;
    const { body, show: open } = panel(v + " " + e.arg, { cls: "full", sub: e.name + " · " + status, label: v + " " + e.arg });
    body.classList.add("viewer-b");
    const section = (label, text) => { const x = el("div", "vs"); x.append(el("span", null, label)); if (text) x.append(copyBtn(text)); body.append(x); };
    const cutNote = (text) => { if (e.cut) { body.append(el("p", "vnote", cutNoteText(e.cut))); return; } if (!e.fullFailed && /…(\(truncated\))?\s*$/.test(text ?? "")) body.append(el("p", "vnote", "Cut short in this copy of the logs.")); };
    if (e.scriptText !== undefined) {
      section("Script", e.scriptText); body.append(el("pre", "script", e.scriptText));
      if (e.scriptTruncated) body.append(el("p", "vnote", "Cut at 8 MB: the rest isn't shown."));
    } else if (e.scriptFailed) body.append(el("p", "vnote", "Couldn't load the script from these logs."));
    else {
      if (e.in) { section(inLabel, e.in); body.append(el("pre", "in", e.in)); cutNote(e.in); }
      if (e.changes) {
        for (const change of e.changes) { section("Change · " + change.path + (change.move ? " → " + change.move : ""), null); body.append(diffEl(change.diff ?? [])); }
        if (!e.changes.length) body.append(el("p", "vnote", "No changes recorded."));
      } else if (e.diff) { section("Change", null); body.append(diffEl(e.diff)); }
      else { section("Output", e.out); if (e.out) { body.append(outEl(e)); cutNote(e.out); } else body.append(el("p", "vnote", e.live ? (SESS[e.sid]?.state === "wait" ? "Waiting for your input or permission · no output yet" : "Running · no output yet") : e.unfinished ? "No result recorded." : "No output.")); }
    }
    if (e.bg?.summary) { section("Finished", e.bg.summary); body.append(el("pre", null, e.bg.summary)); }
    if (e.fullFailed) body.append(el("p", "vnote", "Couldn't load the full text: this is the preview."));
    if (e.fullCut?.length) body.append(el("p", "vnote", "Cut at 8 MB: the rest isn't shown."));
    open();
  }

  // Images a prompt attached: thumbnails above its text, each opening the image whole in the viewer sheet. The page never holds
  // their bytes: /api/tx names each image by its line, block and content version, /api/attachment serves it, and the <img> is
  // made here with its src set to that URL. Its box is sized from the width and height /api/tx read from the image's header
  // (at most 200×160, its shape kept), so nothing moves when it loads; one whose size isn't known gets a fixed box. An image the
  // logs don't hold (a path, a redacted copy) is a quiet chip, and so is one that fails to load.
  const THUMB_W = 200, THUMB_H = 160;
  const IMAGE_KIND = { "image/png": "PNG", "image/jpeg": "JPEG", "image/gif": "GIF", "image/webp": "WebP" };
  const sizeText = (n) => n >= 1048576 ? (n / 1048576).toFixed(1) + " MB" : n >= 1024 ? Math.round(n / 1024) + " KB" : n + " bytes";
  const imageGone = () => el("span", "attach-na", "Image not available");
  function userBody(m, e, text) {
    if (e.img?.length) {
      const row = el("div", "attach-row");
      e.img.forEach((a, i) => {
        if (a.na) { row.append(imageGone()); return; }
        const label = "Attached image " + (i + 1) + " (" + (IMAGE_KIND[a.type] ?? "image") + ", " + sizeText(a.size) + ")";
        const url = "/api/attachment?sid=" + enc(e.sid) + "&o=" + enc(a.o) + "&b=" + enc(a.b) + "&v=" + enc(a.v);
        const b = el("button", "attach"), img = document.createElement("img");
        b.type = "button"; b.setAttribute("aria-haspopup", "dialog");
        img.className = "attach-img"; img.alt = label; img.loading = "lazy"; img.decoding = "async";
        if (a.w > 0 && a.h > 0) { const k = Math.min(1, THUMB_W / a.w, THUMB_H / a.h); img.width = Math.max(1, Math.round(a.w * k)); img.height = Math.max(1, Math.round(a.h * k)); }
        else img.classList.add("unsized");
        img.src = url;
        img.addEventListener("error", () => b.replaceWith(imageGone()), { once: true });
        b.append(img); b.addEventListener("click", () => openImage(url, label, b)); row.append(b);
      });
      m.append(row);
      if (!text) return;
    }
    m.append(markdown(text));
  }
  function openImage(url, label, from) {
    const d = el("dialog", "viewer image-viewer"); d.setAttribute("aria-label", label);
    const head = el("div", "vh"), t = el("div", "vt"), close = el("button", "vclose");
    t.append(el("span", null, label)); close.type = "button"; close.setAttribute("aria-label", "Close"); close.append(icon(I.x)); close.addEventListener("click", () => d.close());
    head.append(t, close);
    const body = el("div", "vb"), img = document.createElement("img");
    img.className = "attach-full"; img.alt = label; img.decoding = "async"; img.src = url;
    img.addEventListener("error", () => img.replaceWith(el("p", "vnote", "Image not available.")), { once: true });
    body.append(img); d.append(head, body); document.body.append(d);
    d.addEventListener("click", (ev) => { if (ev.target === d || ev.target === body) d.close(); }); // a tap beside the image
    // Focus goes back to the thumbnail that opened it, or to the same image's thumbnail when a live update redrew the page.
    const back = () => (from.isConnected ? from : [...document.querySelectorAll("button.attach")].find((x) => x.querySelector("img")?.getAttribute("src") === url))?.focus();
    d.addEventListener("close", () => { d.remove(); document.documentElement.classList.remove("viewer-open"); if (viewerEl === d) { viewerEl = null; if (history.state?.sheet) { skipPop = true; history.back(); } } if (LIVE.pending) refresh(); back(); });
    viewerEl = d; document.documentElement.classList.add("viewer-open"); d.showModal(); close.focus();
    try { history.pushState({ ...route, sheet: 1 }, ""); } catch {}
  }

  function openScript(e) {
    const done = (fields) => openStepViewer({ ...e, ...fields, full: true }, "View script", "run", "Script");
    api("/api/entry?sid=" + enc(e.sid) + "&slot=" + e.slot + "&as=script")
      .then((result) => done({ scriptText: result.text, scriptTruncated: result.truncated }))
      .catch(() => done({ scriptFailed: true }));
  }

  // A brief or message clamped to three lines; "Show more" opens it in place, and only appears when it is cut.
  function clampText(parent, text, cls) {
    const t = rich("div", "ev-text" + (cls ? " " + cls : ""), preview(text)); parent.append(t);
    const more = btn("link ev-more", "Show more"); more.hidden = true; more.setAttribute("aria-expanded", "false");
    more.addEventListener("click", (ev) => { ev.stopPropagation(); const o = t.classList.toggle("open"); more.textContent = o ? "Show less" : "Show more"; more.setAttribute("aria-expanded", String(o)); });
    observeClamp(t, more);
    parent.append(more); return t;
  }
  // A session this one started: a card that opens it. Its state, what it is, the brief, and what it is doing or returned.
  function childCard(h, c) {
    const b = el("div", "child-card"); b.dataset.h = h.id;
    const calls = countOf(c, "calls");
    const head = el("span", "cc-head"); const name = btn("cc-name who-link", c.name, "Open " + c.name); name.addEventListener("click", () => goSession(c.id, STARTS.get(h.id)?.id)); head.append(name, stateLabel(c.state), icon(I.chev, "chev"));
    // The harness mark leads the kind line (#146); the kind stays a word, with no glyph.
    b.append(head, withHarnessIcon(el("span", "cc-meta", [kindText(c), shortModel(c.model), dur(c.start, c.state === "work" ? null : c.last), (calls ?? "—") + (calls === 1 ? " step" : " steps"), costText(costForSession(c.id, true))].join(" · ")), c.harness, { size: 14 }));
    b.append(rich("span", "cc-brief", preview(h.brief)));
    if (h.result) { const r = el("span", "cc-result" + (h.status === "err" ? " err" : "")); r.append(el("span", "rl", h.status === "err" ? "Result: " : "Returned: ")); inline(r, h.result); b.append(r); }
    else if (c.state === "work" && c.activity) { const n = el("span", "cc-now"); n.append(el("span", "spin"), el("span", null, verbNow(c.activity[0])), el("code", null, c.activity[1])); b.append(n); }
    const holder = HOLDS.get(h.id);
    if (holder) { const run = btn("link cc-run", "Run view", "Open run view for " + nameOf(h.from)); run.addEventListener("click", () => goTrace(holder.id)); b.append(run); }
    return b;
  }
  // A relay, a message to you, or a machine move: who, when, the text, and what came back.
  function eventRow(h, sid) {
    const r = el("div", "event" + (h.kind === "toyou" && h.status === "wait" ? " waiting" : "") + (h.kind === "move" ? " move" : "")); r.dataset.h = h.id;
    const [ic, parts] = sentence(h, sid, true); const head = el("div", "ev-head"); const ln = el("span", "ln"); ln.append(...parts); head.append(icon(ic), ln, el("span", "tm", clock(h.at))); r.append(head);
    clampText(r, h.brief);
    if (h.result) { const x = el("div", "ev-result"); x.append(el("span", "rl", h.kind === "relay" ? "Reply: " : "Returned: ")); inline(x, h.result); r.append(x); }
    const an = answerEl(h, "ev-result"); if (an) r.append(an);
    if (h.kind === "toyou" && h.status === "wait") r.append(stateLabel("wait", "Waiting on you"));
    return r;
  }
  function openParentAtHandoff(h) { const turn = HOLDS.get(h.id); goSession(h.from, turn?.id); }
  // The tips on a session footer's items, as plain text (the tooltip sets it with textContent): calls by tool, times, cost by kind.
  const callsTip = (s) => { const parts = Object.entries(s.tool_calls ?? {}).sort((a, b) => b[1] - a[1] || (a[0] < b[0] ? -1 : 1)).map(([name, n]) => name + " " + n), failed = countOf(s, "errors"); if (failed) parts.push(failed + " failed"); return parts.join(" · "); };
  const timeTip = (s, h, finished) => "Started " + clock(s.start) + " · last activity " + clock(s.last) + (finished ? " · finished " + clock(h?.done ?? s.last) : "");
  function costTip(cost) {
    const groups = Object.entries(cost.by_model).map(([id, m]) => { const k = m.usd_by_kind ?? {}, kinds = [["Input", k.input], ["Output", k.output], ["Cache read", k.cache_read], ["Cache write", (Number(k.cache_write_5m) || 0) + (Number(k.cache_write_1h) || 0)]]; if (Number(k.web_search) >= 0.005) kinds.push(["Web search", k.web_search]); return { id, text: kinds.map(([label, usd]) => label + " " + asMoney(Number(usd) || 0)).join(" · ") }; });
    return (groups.length > 1 ? groups.map((g) => g.id + ": " + g.text).join("; ") : groups.map((g) => g.text).join("")) + (groups.length ? ". " : "") + COST_TIP;
  }
  // What a footer shows: its text, its tips, its state, and whether it has the button to the parent.
  const footerSig = (n) => [n.textContent, n.firstChild.className, !!n.querySelector("button"), ...[...n.querySelectorAll("[data-tip]")].map((x) => x.dataset.tip)].join("\n");
  // The line a session page ends in, for a child (h: its origin) and any other session alike: the state, the calls, the time and
  // the API-equivalent cost, each with its breakdown as a tip. A returned child keeps "Returned to <parent>" and its button.
  function sessionFooter(s, h) {
    const block = el("div", "session-foot"), done = s.state === "done" || s.state === "err", finished = h ? done || h.status === "done" || h.status === "err" : done;
    const status = h ? (finished ? (s.state === "err" || h.status === "err" ? "err" : "done") : "work") : s.state;
    // Each item names its kind (data-foot), so a redraw that adds or drops items can give the focus back to the same one.
    const item = (kind, text, tip) => { const n = el("span", "cr-item", text); n.dataset.foot = kind; if (tip) { n.dataset.tip = tip; n.tabIndex = 0; } return n; };
    const line = el("span"), cost = costForSession(s.id), priced = cost.usd != null && !costMissing(cost).length && cost.usd >= 0.005, running = status === "work";
    line.append(spaced(h && finished ? "Returned to " + nameOf(h.from) + " · " + STATE[status] : STATE[status]));
    if (!h || !finished) line.append(spaced(" · "), item("calls", callsText(countOf(s, "calls")), callsTip(s)));
    line.append(spaced(" · "), item("time", dur(s.start, running ? null : s.last), timeTip(s, h, finished)));
    if (priced) line.append(spaced(" · "), item("cost", asMoney(cost.usd), costTip(cost)));
    const state = el("span", "stat " + status); state.append(running ? el("span", "spin") : dot(status, false), line); block.append(state);
    if (h && finished) { const link = el("button", null, "Open in " + nameOf(h.from)); link.type = "button"; link.dataset.foot = "open"; link.addEventListener("click", () => openParentAtHandoff(h)); block.append(link); }
    return block;
  }

  // ---- Render --------------------------------------------------------------------------------------------------------
  // Persistent controls. A control the reader may be operating (a filter, the search field, a toggle group) is built once for
  // the page shown and kept by every redraw of that page: it is never taken out of the document, so its focus, an open
  // dropdown and the caret in a text field survive. `slot` finds or builds it in its box; the caller then hands it the
  // page's current data and handlers through `ctx` (a handler is read from `ctx` when the control fires, so it never calls
  // the closure of an earlier draw). A navigation to another route drops every slot.
  const SLOTS = new Map();
  function slot(key, box, build) {
    let s = SLOTS.get(key);
    if (!s || s.route !== route || s.box !== box || s.el.parentNode !== box) { s = { route, box, ctx: {}, el: null }; s.el = build(s.ctx); SLOTS.set(key, s); }
    return s;
  }
  // Empties `box` of everything but the slots the route being drawn already holds in it.
  function clearBox(box, r) {
    for (const [key, s] of SLOTS) if (s.route !== r) SLOTS.delete(key);
    const kept = new Set([...SLOTS.values()].filter((s) => s.box === box).map((s) => s.el));
    for (const n of [...box.childNodes]) if (!kept.has(n)) n.remove();
  }
  // Adds nodes to `box` in order, after `clearBox`: a node already at the next place stays where it is, and any other
  // goes in before it. `put.done()` drops what is left over.
  function placer(box) {
    let cur = box.firstChild;
    const put = (...nodes) => { for (const n of nodes) { if (n === cur) cur = cur.nextSibling; else box.insertBefore(n, cur); } };
    put.done = () => { while (cur) { const next = cur.nextSibling; cur.remove(); cur = next; } };
    return put;
  }
  function render() {
    runChartObserver?.disconnect(); pruneClamps();
    const focusSearch = focusSessionsSearchOnRender === route; focusSessionsSearchOnRender = null;
    if (SIDEBAR_ONLY) { CHILDREN = null; tick(); rendered = route; renderNav(); renderLanes(); return; } // the embedding page draws its own page and bar
    // The page first, then the bar: the bar's summary (a trace's counts, a search's matches) comes from the page.
    resetPagerInput(); holdProgrammaticScroll(); closeAccountMenu(); stopOpeningEndPin(); CHILDREN = null; // a redraw inside the open-at-end window ends the pin
    ordPageState = ordState("page"); tick(); const page = $("#page"), r = route; rendered = r; page.style.paddingBottom = ""; clearBox(page, r); page.classList.remove("child-page");
    if (r.v === "home") { renderHome(page); renderTopbar("Home"); }
    else if (r.v === "analytics") { renderAnalytics(page); renderTopbar("Analytics", null, { analytics: true }); }
    else if (r.v === "sessions") { renderSessions(page, focusSearch); renderTopbar("Sessions"); }
    else if (r.v === "machines") { renderMachines(page); renderTopbar("Machines"); }
    else if (r.v === "machine") { renderMachine(page, r.id); renderTopbar(MACHINE[r.id], { label: "Machines", go: () => go({ v: "machines" }) }, { line2: machineLine(r.id) }); }
    else if (r.v === "trace") { const sum = renderTrace(page, r.turn) ?? ""; if (sum) page.prepend(el("p", "trace-summary", sum)); renderTopbar("Trace", { label: SESS[r.sid].name, go: () => goSession(r.sid, r.turn) }, { traceSession: SESS[r.sid] }); }
    else if (r.v === "session") { const s = SESS[r.id], lineage = lineageOf(r.id).slice(0, -1); renderSession(page, r.id); renderTopbar(s.name, null, { session: s, lineage, line2: sessionLine(s) }); }
    if (r.v === "session" && errOn(r.id)) markError(false);
    document.documentElement.style.setProperty("--barh", $("#topbar").offsetHeight + "px");
    const lanesKept = lanesFor && lanesFor.r === r && lanesFor.version === LIVE.version; lanesFor = null;
    syncLayoutPrefs(); syncBarLine(); renderNav(); if (!lanesKept) renderLanes(); renderDrawerAccount(); syncJump(); ordPageState = null;
  }

  // ---- Analytics: the server computes each range (/api/analytics) -----------------------------------------------------------
  // The model holds only its own window (a day), so the page asks the server for the range it shows: 24 h, 7 d or 30 d, with
  // the filters. The answer has every figure, chart column and list the page draws, and the page does no range math. It is
  // asked for when the page opens, when the range or a filter changes, after every model update (the server answers 304
  // while nothing changed), and every 10 s while the page shows, as time moves the range. Answers are kept per range and
  // filters, so switching back draws at once while the page asks again.
  const MIN = 60000, HOUR = 60 * MIN, AN_EVERY = 10000, AN_KEEP = 8;
  const AN = { answers: new Map(), inflight: null, again: false, againAsked: false, timer: null, error: null, failedAt: 0 };
  function analyticsQuery() {
    const q = ["range=" + (analyticsRange === 1 ? "24h" : analyticsRange + "d")];
    // "No repo" is an empty repo; an unset filter isn't sent.
    if (sessionFilters.repo) q.push("repo=" + (sessionFilters.repo === "__none__" ? "" : enc(sessionFilters.repo)));
    for (const key of ["machine", "harness", "model"]) if (sessionFilters[key]) q.push(key + "=" + enc(sessionFilters[key]));
    return q.join("&");
  }
  const analyticsData = () => AN.answers.get(analyticsQuery())?.data ?? null;
  // One request at a time: a change while one is out asks again when it is back. Resolves true when the answer changed.
  function fetchAnalytics(askedByUser = false) {
    if (AN.inflight) { AN.again = true; AN.againAsked ||= askedByUser; return AN.inflight; }
    const key = analyticsQuery(), kept = AN.answers.get(key);
    const asked = fetch("/api/analytics?" + key, { credentials: "same-origin", headers: kept?.etag ? { "If-None-Match": kept.etag } : {} })
      .then((r) => {
        if (r.status === 304) { const cleared = AN.error != null; AN.error = null; return cleared; } // unchanged, and asking works again
        if (r.status === 403) { ended(403); return false; }
        if (!r.ok) throw Object.assign(new Error(r.status + " " + r.statusText), { status: r.status });
        const etag = r.headers.get("ETag");
        return r.json().then((data) => {
          AN.answers.delete(key); AN.answers.set(key, { etag, data });
          while (AN.answers.size > AN_KEEP) AN.answers.delete(AN.answers.keys().next().value);
          const changed = AN.error != null || kept?.etag !== etag; AN.error = null; return changed;
        });
      })
      .catch((e) => { const changed = AN.error !== e.message; AN.error = e.message; AN.failedAt = performance.now(); return changed; });
    AN.inflight = asked.then((changed) => {
      AN.inflight = null;
      if (!AN.again) return changed;
      const askedAgain = AN.againAsked; AN.again = AN.againAsked = false;
      if (!askedAgain && backingOff()) return changed;
      return fetchAnalytics(askedAgain).then((more) => changed || more);
    });
    return AN.inflight;
  }
  // Asks again in 10 s, or in a little over a second when the answer came from an older model than the page has. While
  // asking fails (a 409 or a 500), it asks 10 s after the last failure: the kept answer stays drawn, with the error above it.
  const backingOff = () => AN.error != null && performance.now() - AN.failedAt < AN_EVERY; // a monotonic clock: a wall-clock jump neither stalls nor rushes it
  function scheduleAnalytics() {
    clearTimeout(AN.timer); AN.timer = null;
    if (route.v !== "analytics" || LIVE.ended || !visible()) return;
    const data = analyticsData(), behind = !AN.error && data && LIVE.version && data.version !== LIVE.version;
    AN.timer = setTimeout(() => { AN.timer = null; refreshAnalytics(); }, AN.error ? Math.max(0, AN.failedAt + AN_EVERY - performance.now()) : behind ? 1200 : AN_EVERY);
  }
  // `asked`: the reader changed the range or a filter, which asks at once. Anything else (a model update, the tab showing
  // again) waits out the backoff while asking fails, so failed asks keep 10 s apart however fast the model moves.
  function refreshAnalytics(asked = false) {
    if (route.v !== "analytics") return Promise.resolve();
    if (asked !== true && backingOff()) { if (!AN.timer) scheduleAnalytics(); return Promise.resolve(); }
    clearTimeout(AN.timer); AN.timer = null;
    return fetchAnalytics(asked === true).then((changed) => {
      if (changed && route.v === "analytics" && rendered === route) {
        if (viewerEl || accountChrome.open) LIVE.pending = true; // drawn when the sheet or the account menu closes
        else { const st = capture(); render(); restore(st); }
      }
      scheduleAnalytics();
    });
  }
  if (!SIDEBAR_ONLY) document.addEventListener("visibilitychange", () => { if (visible() && route.v === "analytics") refreshAnalytics(); else if (!visible()) { clearTimeout(AN.timer); AN.timer = null; } });
  const nameOfSid = (A, sid) => SESS[sid]?.name ?? A.sessions[sid]?.name ?? sid;
  const harnessOfSid = (A, sid) => SESS[sid]?.harness ?? A.sessions[sid]?.harness ?? "";
  // A session older than the model's window has no page to open: its row is text.
  // `rank` ({ measure, max }) makes it a ranked row: a link to the session (a plain click still goes through the router) with a bar scaled to the list's largest value, in its harness's colour.
  function analyticsSessionRow(A, sid, cls, value, onOpen, rank) {
    const open = !!SESS[sid], harness = harnessOfSid(A, sid), b = el(open ? (rank ? "a" : "button") : "div", cls);
    if (open && rank) { b.href = urlOf({ v: "session", id: sid }); b.addEventListener("click", (e) => { if (e.defaultPrevented || e.button || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return; e.preventDefault(); onOpen(); }); }
    else if (open) { b.type = "button"; b.addEventListener("click", onOpen); }
    const label = el("span", "hlabel"), mark = harnessIcon(harness, { size: 14 }); if (mark) label.append(mark); label.append(harnessName(harness, true)); // the mark and the label share the cell the label had
    b.append(el("span", "session-name", nameOfSid(A, sid)), label, el("span", "session-value", value));
    if (rank) { b.classList.add("ranked"); if (/^[\w-]+$/.test(harness)) b.classList.add("h-" + harness); b.append(rankTrack(rank.measure, rank.max).track); }
    return b;
  }
  // The bar of a ranked row (a Breakdown row or a top session): its width is the value's share of the list's largest, never under 2 % when there is a value.
  function rankTrack(measure, max) {
    const track = el("span", "row-track"), bar = el("i", "row-bar"); bar.style.width = Math.max(measure ? 2 : 0, measure / max * 100) + "%"; track.append(bar); return { track, bar };
  }
  const sessionFacetValue = (s, key) => key === "repo" ? s.repo ?? "__none__" : key === "model" ? s.model ?? s.modelId ?? "Unknown model" : s[key] ?? "";
  function matchesSessionFacets(s) { return Object.keys(sessionFilters).every((key) => !sessionFilters[key] || sessionFacetValue(s, key) === sessionFilters[key]); }
  // The four filters (Repo, Machine, Harness, Model) of Analytics and Sessions: one persistent control per page. It is one
  // "Filter" button (with the count of active filters) and one chip per active filter, whose × clears it. The button opens a
  // sheet (a bottom sheet on a phone, a dialog on a wide screen) holding the four Selects, with "Clear all" and "Done". The
  // choices apply when the sheet closes, however it closes. A redraw (`sync`) brings the Selects' option lists, the chips and the
  // count up to date in place. A selected value that no session has now stays selected, marked "(no sessions)", until the reader
  // changes it. The sheet lives inside the control, so its Selects are in the page even while it is shut.
  // On Analytics the range's own values join the model's: a repo that worked last week is a choice there.
  const rangeFacet = (key) => route.v !== "analytics" ? [] : (analyticsData()?.facets?.[key] ?? []).map((v) => v ?? "__none__");
  const FACETS = [
    ["repo", "Repo", "All repos", () => [...new Set([...Object.values(SESS).map((s) => sessionFacetValue(s, "repo")), ...rangeFacet("repo")])].sort((a, b) => a === "__none__" ? 1 : b === "__none__" ? -1 : a.localeCompare(b)), (v) => v === "__none__" ? "No repo" : v],
    ["machine", "Machine", "All machines", () => [...new Set([...Object.values(SESS).map((s) => s.machine ?? ""), ...rangeFacet("machine")])].sort(), (v) => MACHINE[v] ?? v],
    ["harness", "Harness", "All harnesses", () => [...new Set([...Object.values(SESS).map((s) => s.harness ?? ""), ...rangeFacet("harness")])].sort(), (v) => HARNESS[v] ?? v],
    ["model", "Model", "All models", () => [...new Set([...Object.values(SESS).map((s) => sessionFacetValue(s, "model")), ...rangeFacet("model")])].sort(), shortModel],
  ];
  function renderFacetFilters(box, onChange) {
    const s = slot("facets", box, (ctx) => {
      const bar = el("div", "facet-filters"); bar.setAttribute("role", "group"); bar.setAttribute("aria-label", "Filter sessions"); const fields = [];
      const btn = el("button", "facet-btn"), count = el("span", "facet-n"); btn.type = "button"; btn.setAttribute("aria-haspopup", "dialog"); count.hidden = true; btn.append(icon(I.filter), el("span", null, "Filter"), count);
      const d = el("dialog", "viewer filters-sheet"), head = el("div", "vh"), title = el("div", "vt"), close = el("button", "vclose"), body = el("div", "vb"), foot = el("div", "vf"), clear = el("button", "fclear", "Clear all"), done = el("button", "fdone", "Done");
      d.setAttribute("aria-label", "Filter"); title.append(el("span", null, "Filter")); close.type = "button"; close.setAttribute("aria-label", "Close filters"); close.append(icon(I.x)); close.addEventListener("click", () => d.close()); head.append(title, close);
      clear.type = "button"; done.type = "button"; foot.append(clear, done); d.append(head, body, foot); bar.append(btn);
      for (const [key, label, allLabel, valuesOf, showValue] of FACETS) {
        const select = SemonShell.select({ label, options: [{ value: "", label: allLabel }], value: sessionFilters[key], onChange: (value) => { sessionFilters[key] = value; } }); body.append(select.el);
        const chip = el("button", "facet-chip"), text = el("span", "txt"); chip.type = "button"; chip.hidden = true; chip.dataset.facet = key; chip.append(text, icon(I.x));
        chip.addEventListener("click", () => { sessionFilters[key] = ""; ctx.sync(); btn.focus({ preventScroll: true }); ctx.onChange(); });
        bar.append(chip); fields.push({ key, label, select, allLabel, valuesOf, showValue, chip, text });
      }
      bar.append(d);
      ctx.sync = () => {
        let active = 0;
        for (const f of fields) {
          const current = sessionFilters[f.key], values = f.valuesOf().filter((v) => v !== ""), gone = current !== "" && !values.includes(current);
          if (gone) values.push(current);
          f.select.setOptions([{ value: "", label: f.allLabel }, ...values.map((v) => ({ value: v, label: f.showValue(v) + (gone && v === current ? " (no sessions)" : "") }))]);
          f.select.setValue(current);
          f.chip.hidden = current === ""; if (current !== "") { active++; f.text.textContent = f.label + ": " + f.showValue(current); f.chip.setAttribute("aria-label", "Remove " + f.label + ": " + f.showValue(current)); }
        }
        count.hidden = !active; count.textContent = active ? String(active) : ""; btn.setAttribute("aria-label", active ? "Filter, " + active + " active" : "Filter");
      };
      // Opening the sheet is a history entry, like the viewer's other sheets: Back closes it and the page stays. The page behind it
      // doesn't scroll: not by overflow: hidden on the page (that resets a phone's page to the top, as the Select's sheet found), but by
      // refusing the wheel and touch moves that don't start in a list that can move (the Select's own sheet does the same for its list).
      let before = "";
      const hold = (e) => { if (!d.isConnected || !d.open) { for (const type of ["wheel", "touchmove"]) document.removeEventListener(type, hold, { capture: true }); if (viewerEl === d) viewerEl = null; return; } const t = e.target; if (t.closest?.(".sh-select-list") || (body.contains(t) && body.scrollHeight > body.clientHeight + 1)) return; e.preventDefault(); };
      btn.addEventListener("click", () => {
        if (viewerEl || d.open) return; ctx.sync(); before = JSON.stringify(sessionFilters);
        viewerEl = d; d.showModal(); for (const type of ["wheel", "touchmove"]) document.addEventListener(type, hold, { capture: true, passive: false });
        fields[0].select.focus(); try { history.pushState({ ...route, sheet: 1, scrollTop: currentScroll() }, ""); } catch {}
      });
      d.addEventListener("keydown", (e) => {
        if (e.key !== "Tab" || e.defaultPrevented) return;
        const controls = [...d.querySelectorAll("button:not(:disabled), input:not(:disabled), [tabindex]:not([tabindex='-1'])")].filter((x) => x.getClientRects().length && !x.closest("[hidden]"));
        const first = controls[0], last = controls.at(-1);
        if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last?.focus(); }
        else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first?.focus(); }
      });
      clear.addEventListener("click", () => { for (const key of Object.keys(sessionFilters)) sessionFilters[key] = ""; d.close(); });
      done.addEventListener("click", () => d.close());
      d.addEventListener("click", (e) => { if (e.target === d) d.close(); });
      d.addEventListener("close", () => {
        for (const type of ["wheel", "touchmove"]) document.removeEventListener(type, hold, { capture: true });
        if (viewerEl === d) { viewerEl = null; if (history.state?.sheet) { skipPop = true; history.back(); } }
        btn.focus({ preventScroll: true });
        if (JSON.stringify(sessionFilters) !== before) { LIVE.pending = false; ctx.onChange(); } else if (LIVE.pending) refresh();
      });
      return bar;
    });
    s.ctx.onChange = onChange; s.ctx.sync(); return s.el;
  }
  const hoursText = (ms) => (ms / HOUR).toFixed(1) + " h", rangeName = () => analyticsRange === 1 ? "24 h" : analyticsRange + " d";
  function deltaNote(value, previous, format) {
    const delta = value - previous, note = el("div", "note");
    if (Math.abs(delta) < 1e-9) { note.textContent = "No change vs previous " + rangeName(); return note; }
    note.append(el("span", delta > 0 ? "up" : "down", (delta > 0 ? "+" : "−") + format(Math.abs(delta))), " vs previous " + rangeName()); return note;
  }
  function chartWidth() { const page = $("#page"), style = getComputedStyle(page); return Math.max(280, Math.round(page.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight))); }
  const niceStep = (max) => [.25, .5, 1, 2, 5, 10, 20, 50, 100, 200, 500].find((x) => x * 3 >= max) ?? 1000;
  const svgEl = (tag, attrs, text) => { const node = document.createElementNS(SVGNS, tag); for (const [key, value] of Object.entries(attrs)) node.setAttribute(key, String(value)); if (text != null) node.textContent = text; return node; };
  function timeText(ms) { const mins = Math.max(0, Math.round(ms / MIN)), days = Math.floor(mins / 1440), hours = Math.floor(mins % 1440 / 60), rem = mins % 60; return days ? days + "d " + hours + "h" : hours ? hours + "h " + rem + "m" : mins + "m"; }
  const countText = (n) => Math.round(n).toLocaleString(), hLabel = (n) => n ? +n.toFixed(2) + " h" : "0";
  const rangeAgo = (A) => A.days === 1 ? "24 h ago" : A.days + " d ago";
  // The graph's thin bars have equivalent full-width targets on touch screens.
  function chartSlices(A, bins, costMode = false) {
    const details = el("details", "chart-slices"), summary = el("summary", null, "Explore time slices"), list = el("div", "chart-slice-list");
    for (const bin of bins) {
      if (costMode ? !bin.sessions.length : bin.claude + bin.codex <= 0) continue;
      const b = btn("chart-slice"); b.append(el("span", null, clock(bin.a) + "–" + clock(bin.b)), el("span", null, costMode ? asMoney(bin.claude + bin.codex) : hLabel(bin.claude + bin.codex) + " agent-hours"));
      b.addEventListener("click", () => openAnalyticsSlice(A, bin.a, bin.b, bin.sessions, bin.more, costMode)); list.append(b);
    }
    if (!list.childElementCount) return null; details.append(summary, list); return details;
  }
  function renderAgentsChart(A) {
    const columns = A.agents.columns, count = columns.length, unit = A.agents.unit, panel = el("section", "analytics-panel");
    panel.append(el("h2", null, "Agents at work"), el("div", "panel-sub", "Agent-hours " + unit + " · stacked by harness"));
    const bins = columns.map((c) => ({ a: c.from, b: c.to, claude: c.claude_ms / HOUR, codex: c.codex_ms / HOUR, sessions: c.sessions, more: c.more }));
    const W = chartWidth(), height = 190, left = 40, right = W - 4, top = 12, bottom = 151, most = Math.max(0, ...bins.map((x) => x.claude + x.codex)), stepY = niceStep(most || 1), max = Math.max(stepY, Math.ceil(most / stepY) * stepY);
    const svg = svgEl("svg", { viewBox: "0 0 " + W + " " + height, role: "group", "aria-label": "Agent-hours " + unit + " over the selected range, stacked by harness" }), yOf = (n) => bottom - (bottom - top) * n / max;
    for (let n = 0; n <= max + 1e-9; n += stepY) svg.append(svgEl("line", { x1: left, x2: right, y1: yOf(n), y2: yOf(n), class: "gridline" }), svgEl("text", { x: 0, y: yOf(n) + 4, class: "axis-label" }, hLabel(n)));
    const step = (right - left) / Math.max(1, count), w = Math.max(2, step * .64);
    bins.forEach((bin, i) => { const x = left + i * step + (step - w) / 2, ch = (bottom - top) * bin.claude / max, xh = (bottom - top) * bin.codex / max, total = bin.claude + bin.codex;
      if (ch) svg.append(svgEl("rect", { x, y: bottom - ch, width: w, height: ch, class: "cost-claude" })); if (xh) svg.append(svgEl("rect", { x, y: bottom - ch - xh, width: w, height: xh, class: "cost-codex" }));
      const label = clock(bin.a) + "–" + clock(bin.b) + ": " + hLabel(total), hit = svgEl("rect", { x: left + i * step, y: top, width: step, height: bottom - top, class: "chart-hit" }); hit.dataset.tip = label;
      if (total > 0) { hit.setAttribute("role", "button"); hit.setAttribute("tabindex", "0"); hit.setAttribute("aria-label", label + ". Open the sessions busy then"); }
      const open = () => { if (total > 0) openAnalyticsSlice(A, bin.a, bin.b, bin.sessions, bin.more); };
      hit.addEventListener("click", open); hit.addEventListener("keydown", (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); open(); } }); svg.append(hit);
    });
    svg.append(svgEl("text", { x: left, y: 178, class: "axis-label" }, rangeAgo(A)), svgEl("text", { x: right, y: 178, "text-anchor": "end", class: "axis-label" }, "Now"));
    const chart = el("div", "analytics-chart"); chart.append(svg); panel.append(chart);
    const legend = el("div", "analytics-legend"); for (const [h, label] of [["claude", "Claude"], ["codex", "Codex"]]) { const item = el("span"), swatch = el("i"); swatch.style.setProperty("--h", "var(--" + h + ")"); item.append(...[swatch, harnessIcon(h, { size: 14 }), label].filter(Boolean)); legend.append(item); } panel.append(legend); const slices = chartSlices(A, bins); if (slices) panel.append(slices); return panel;
  }
  function renderCostChart(A) {
    const panel = el("section", "analytics-panel"), title = el("h2", null, "Cost over time"); title.append(costInfoTip());
    // Cost is recorded per UTC day: an hourly series would put a whole day into one hour.
    if (!A.cost) { panel.append(title, el("p", "empty", "Cost is recorded per UTC day, so there is no hourly series. Pick 7 d or 30 d for a daily chart.")); return panel; }
    const days = A.cost.days, count = days.length, unit = "per day"; panel.append(title, el("div", "panel-sub", "API-equivalent cost per UTC day · today so far · stacked by harness"));
    const bins = days.map((d) => ({ a: d.from, b: d.to, claude: d.claude_usd, codex: d.codex_usd, sessions: d.sessions, more: d.more }));
    const W = chartWidth(), svg = svgEl("svg", { viewBox: "0 0 " + W + " 190", role: "img", "aria-label": "API-equivalent cost " + unit + ", stacked by harness" });
    const left = 46, right = W - 4, top = 12, bottom = 151, max = Math.max(.01, ...bins.map((b) => b.claude + b.codex)), step = (right - left) / Math.max(1, count);
    for (let n = 0; n <= 2; n++) { const y = bottom - (bottom - top) * n / 2; svg.append(svgEl("line", { x1: left, x2: right, y1: y, y2: y, class: "gridline" }), svgEl("text", { x: 0, y: y + 4, class: "axis-label" }, "$" + (max * n / 2).toFixed(2))); }
    bins.forEach((bin, i) => { const w = Math.max(2, step * .64), x = left + i * step + (step - w) / 2, ch = bin.claude / max * (bottom - top), xh = bin.codex / max * (bottom - top);
      if (ch) svg.append(svgEl("rect", { x, y: bottom - ch, width: w, height: ch, class: "cost-claude" })); if (xh) svg.append(svgEl("rect", { x, y: bottom - ch - xh, width: w, height: xh, class: "cost-codex" }));
      const hit = svgEl("rect", { x: left + i * step, y: top, width: step, height: bottom - top, class: "chart-hit" }); if (bin.sessions.length) { hit.setAttribute("role", "button"); hit.setAttribute("tabindex", "0"); hit.setAttribute("aria-label", clock(bin.a) + " to " + clock(bin.b) + ": " + asMoney(bin.claude + bin.codex)); }
      const open = () => { if (bin.sessions.length) openAnalyticsSlice(A, bin.a, bin.b, bin.sessions, bin.more, true); }; hit.addEventListener("click", open); hit.addEventListener("keydown", (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); open(); } }); svg.append(hit);
    });
    svg.append(svgEl("text", { x: left, y: 178, class: "axis-label" }, rangeAgo(A)), svgEl("text", { x: right, y: 178, "text-anchor": "end", class: "axis-label" }, "Now"));
    const chart = el("div", "analytics-chart"); chart.append(svg); panel.append(chart); const legend = el("div", "analytics-legend");
    for (const [h, label] of [["claude", "Claude"], ["codex", "Codex"]]) { const item = el("span"), swatch = el("i"); swatch.style.setProperty("--h", "var(--" + h + ")"); item.append(...[swatch, harnessIcon(h, { size: 14 }), label].filter(Boolean)); legend.append(item); } panel.append(legend);
    const slices = chartSlices(A, bins, true); if (slices) panel.append(slices);
    if (A.cost.unpriced_models.length) panel.append(el("div", "no-price", "no price for " + A.cost.unpriced_models.join(", ") + "; unpriced usage is omitted from bars.")); return panel;
  }
  function openModelItems(group) {
    const { d, body, show: open } = panel(shortModel(group.model) + " · " + group.band, { label: "Work items for " + group.model + ", " + group.band });
    body.append(el("p", "panel-sub", group.n + " work items" + (group.small_sample ? " · small sample" : "")));
    for (const item of group.items ?? []) {
      const row = el("div", "model-item");
      if (SESS[item.sid]) { const b = btn("model-item-session", SESS[item.sid].name); b.addEventListener("click", () => { pendingSessionOpen = item.sid; d.close(); }); row.append(b); }
      else row.append(el("span", null, item.sid + " · outside the session window"));
      row.append(el("span", "panel-sub", (item.models ?? []).map(shortModel).join(" → ") + " · " + (item.cost_usd == null ? "cost unknown" : asMoney(item.cost_usd))));
      const trace = (TURNS[item.sid] ?? []).find((t) => t.out.length); if (trace) { const b = btn("model-item-trace", "Trace"); b.addEventListener("click", () => { afterPop = () => goTrace(trace.id); d.close(); }); row.append(b); }
      if (liveUrl(item.pr_url)) row.append(link("Open pull request", item.pr_url)); body.append(row);
    }
    if (group.items_more) body.append(el("p", "panel-sub", group.items_more + " more items in the selected range")); open();
  }
  function renderModelsComparison(A) {
    if (!A.models) return null;
    const section = el("section", "analytics-panel models-panel"); section.append(el("h2", null, "Models"), el("p", "panel-sub", "Your launched work, compared within each difficulty band. Every measure shows its own sample count; unknown values are not estimated."));
    const groups = A.models.groups ?? [], reasons = A.models.unknown_reasons ?? {};
    if (!groups.length) { section.append(el("p", "empty", "No launched work with recorded per-message models in this range.")); return section; }
    const measures = [["Acceptance", "first_pass_acceptance", "acceptance_n", "acceptance", (n) => (n * 100).toFixed(0) + "%"], ["Review rounds", "median_review_rounds", "review_rounds_n", "review_rounds", (n) => String(n)], ["Red CI heads", "median_red_ci_heads", "red_ci_n", "ci", (n) => String(n)], ["Model time", "median_model_ms", "model_time_n", "model_time", timeText], ["API cost", "median_cost_usd", "cost_n", "cost", asMoney], ["Allowance / M input", "allowance_per_million_input", "allowance_n", "allowance", (n) => n.toFixed(2) + "%"]];
    for (const band of ["easy", "medium", "hard", "unknown"]) {
      const rows = groups.filter((g) => g.band === band); if (!rows.length) continue;
      const heading = el("h3", null, band === "unknown" ? "Unknown difficulty" : band[0].toUpperCase() + band.slice(1)); section.append(heading);
      const wrap = el("div", "model-table-scroll"); wrap.tabIndex = 0; wrap.setAttribute("role", "region"); wrap.setAttribute("aria-label", band + " model comparison");
      const table = el("table", "model-table"), head = el("thead"), hr = el("tr"); for (const title of ["Model", "Work items", ...measures.map((m) => m[0]), "Tokens · input / output / cache"]) { const th = el("th", null, title); th.scope = "col"; hr.append(th); } head.append(hr); table.append(head);
      const body = el("tbody");
      for (const group of rows) {
        const tr = el("tr"), model = el("td"), b = btn("model-comparison-open", shortModel(group.model)); b.dataset.tip = group.model; b.addEventListener("click", () => openModelItems(group)); model.append(b); tr.append(model, el("td", null, group.n + (group.small_sample ? " · small sample" : "")));
        for (const [, key, nKey, reason, format] of measures) { const cell = el("td"), value = group[key]; cell.append(el("span", null, value == null ? "Unknown" : format(value)), el("span", "model-n", "n=" + (group[nKey] ?? 0))); if (value == null && reasons[reason]) cell.dataset.tip = reasons[reason]; tr.append(cell); }
        const tokens = group.tokens ?? {}, cell = el("td"); cell.append(el("span", null, group.tokens_n ? [tokens.input ?? 0, tokens.output ?? 0, (tokens.cache_read ?? 0) + (tokens.cache_write ?? 0)].map(countText).join(" / ") : "Unknown"), el("span", "model-n", "n=" + (group.tokens_n ?? 0))); tr.append(cell); body.append(tr);
      }
      table.append(body); wrap.append(table); section.append(wrap);
      const points = rows.filter((g) => g.first_pass_acceptance != null && g.median_cost_usd != null);
      if (points.length) {
        const W = Math.max(280, chartWidth()), maxCost = Math.max(.01, ...points.map((g) => g.median_cost_usd)), svg = svgEl("svg", { viewBox: "0 0 " + W + " 190", role: "img", "aria-label": "API cost against first-pass acceptance for " + band + " work" });
        svg.append(svgEl("line", { x1: 42, x2: W - 12, y1: 150, y2: 150, class: "gridline" }), svgEl("line", { x1: 42, x2: 42, y1: 12, y2: 150, class: "gridline" }));
        for (const group of points) { const x = 42 + group.median_cost_usd / maxCost * (W - 58), y = 150 - group.first_pass_acceptance * 130, dot = svgEl("circle", { cx: x, cy: y, r: 5, class: "model-point" }); dot.dataset.tip = group.model + ": " + asMoney(group.median_cost_usd) + ", " + (group.first_pass_acceptance * 100).toFixed(0) + "% accepted; acceptance n=" + group.acceptance_n + ", cost n=" + group.cost_n; dot.setAttribute("aria-label", dot.dataset.tip); svg.append(dot, svgEl("text", { x: Math.max(42, Math.min(W - 90, x + 8)), y: Math.max(20, y - 8), class: "axis-label" }, shortModel(group.model))); }
        svg.append(svgEl("text", { x: 42, y: 178, class: "axis-label" }, "Median API cost →"), svgEl("text", { x: 2, y: 16, class: "axis-label" }, "100%"), svgEl("text", { x: 8, y: 151, class: "axis-label" }, "0%")); const chart = el("div", "analytics-chart"); chart.append(svg); section.append(chart);
      } else section.append(el("p", "panel-sub", "Cost / acceptance plot needs both recorded measures."));
    }
    return section;
  }
  function renderCodexAllowance(limits) {
    if (!limits || limits.recorded_at == null) return null;
    const panel = el("section", "analytics-panel"), grid = el("div", "allowance-grid"); panel.append(el("h2", null, "Codex allowance"), el("div", "panel-sub", "Latest recorded rate limits · " + new Date(limits.recorded_at).toLocaleString([], { hour: "numeric", minute: "2-digit" })));
    for (const limit of limits.windows ?? []) { const label = limit.minutes === 300 ? "5-hour window" : limit.minutes === 10080 ? "Weekly window" : limit.minutes + "-minute window", box = el("div", "allowance-window");
      box.append(el("div", "window-name", label), el("div", "window-used", limit.used_percent + "% used"), el("div", "window-reset", "Resets " + new Date(limit.resets_at).toLocaleString([], { weekday: "short", hour: "numeric", minute: "2-digit" }))); grid.append(box); }
    if (!grid.childElementCount) return null; panel.append(grid); return panel;
  }
  // A chart column's sessions, as the server listed them (most first); `more` counts those it left out.
  function openAnalyticsSlice(A, a, b, items, more, costMode = false) {
    const d = el("dialog", "viewer analytics-slice"), head = el("div", "vh"), title = el("div", "vt"), close = el("button", "vclose"), when = new Date(a).toLocaleString([], { weekday: "short", hour: "numeric", minute: "2-digit" }) + "–" + new Date(b).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" }), heading = costMode ? "Sessions with cost" : "Sessions busy";
    d.setAttribute("aria-label", heading + " " + when); title.append(el("span", null, heading + " · " + when)); close.type = "button"; close.setAttribute("aria-label", "Close sessions list"); close.append(icon(I.x)); close.addEventListener("click", () => d.close()); head.append(title, close);
    const body = el("div", "vb"), list = el("div", "analytics-list"); if (!items.length) body.append(el("p", "empty", costMode ? "No sessions had a recorded cost then." : "No sessions were busy then."));
    for (const item of items) {
      const row = analyticsSessionRow(A, item.sid, "analytics-session analytics-slice", costMode ? asMoney(item.usd) : timeText(item.ms) + " busy", () => { pendingSessionOpen = item.sid; d.close(); });
      if (costMode && item.unpriced_models.length) row.append(el("span", "no-price", "no price for " + item.unpriced_models.join(", "))); list.append(row);
    }
    if (more) list.append(el("p", "empty", "and " + more + " more"));
    if (items.length) body.append(list); d.append(head, body); document.body.append(d); d.addEventListener("click", (e) => { if (e.target === d) d.close(); });
    d.addEventListener("close", () => { d.remove(); document.documentElement.classList.remove("viewer-open"); if (viewerEl === d) { viewerEl = null; if (history.state?.sheet) { skipPop = true; history.back(); } else if (pendingSessionOpen) { const id = pendingSessionOpen; pendingSessionOpen = null; goSession(id); } } });
    viewerEl = d; document.documentElement.classList.add("viewer-open"); d.showModal(); close.focus({ focusVisible: false }); try { history.pushState({ ...route, sheet: 1, scrollTop: currentScroll() }, ""); } catch {}
  }
  // The server's groups: repo (null for none), machine, or harness and model, each with its busy time, cost and sessions.
  function analyticsBreakdown(title, groups, groupKey) {
    const keyFor = (g) => groupKey === "repo" ? g.repo ?? "__none__" : groupKey === "machine" ? g.machine : g.harness + "\u0000" + g.model;
    const labelFor = (key) => groupKey === "repo" ? key === "__none__" ? "No repo (roles)" : key : groupKey === "machine" ? MACHINE[key] ?? key : (HARNESS[key.split("\u0000")[0]] ?? key.split("\u0000")[0]) + " · " + shortModel(key.split("\u0000")[1]);
    const selected = (x) => analyticsMeasure === "cost" ? x.cost : x.ms, items = groups.map((g) => ({ key: keyFor(g), ms: g.ms, cost: g.usd, unknown: g.unpriced_models, sessions: g.sessions })).sort((a, b) => selected(b) - selected(a) || labelFor(a.key).localeCompare(labelFor(b.key))), max = Math.max(1, ...items.map(selected));
    const panel = el("section", "analytics-panel"); panel.append(el("h3", null, title)); const list = el("div", "analytics-list");
    for (const item of items) { const b = el("button", "analytics-row"); b.type = "button"; b.append(el("span", "row-title", labelFor(item.key)), el("span", "row-count", item.sessions + (item.sessions === 1 ? " session" : " sessions")));
      const { track, bar } = rankTrack(selected(item), max); if (groupKey === "harness") bar.style.background = item.key.startsWith("claude") ? "var(--claude)" : "var(--codex)";
      b.append(track, el("span", "row-hours" + (analyticsMeasure === "hours" ? " on" : ""), hoursText(item.ms)), el("span", "row-cost" + (analyticsMeasure === "cost" ? " on" : ""), item.unknown.length ? "—" : asMoney(item.cost))); if (item.unknown.length) b.append(el("span", "no-price", "no price for " + item.unknown.join(", ")));
      b.dataset.breakdown = groupKey; b.dataset.key = item.key;
      b.addEventListener("click", () => { if (groupKey === "repo") sessionFilters.repo = item.key; else if (groupKey === "machine") sessionFilters.machine = item.key; else { const [harness, model] = item.key.split("\u0000"); sessionFilters.harness = harness; sessionFilters.model = model; } query = ""; groupBy = "recent"; go({ v: "sessions" }); }); list.append(b); }
    if (!items.length) list.append(el("p", "empty", "No activity in this range.")); panel.append(list); return panel;
  }
  function analyticsList(A, title, items, value, measure) {
    const panel = el("section", "analytics-panel"); panel.append(el("h2", null, title)); const list = el("div", "analytics-list"); if (!items.length) list.append(el("p", "empty", "No sessions in this range."));
    const max = Math.max(1, ...items.map(measure));
    for (const item of items) { const b = analyticsSessionRow(A, item.sid, "analytics-session", value(item), () => goSession(item.sid), { measure: measure(item), max }); const missing = item.unpriced_models ?? []; if (missing.length) b.append(el("span", "no-price", "no price for " + missing.join(", "))); list.append(b); } panel.append(list); return panel;
  }
  function renderAnalytics(page) {
    const A = analyticsData();
    const head = el("div", "ph"), h1 = el("h1", null, "Analytics"); head.append(h1, el("div", "sub", "Measured activity · Last " + (analyticsRange === 1 ? "24 hours" : analyticsRange + " days")));
    const put = placer(page); put(head); observeTitle(h1); put(renderFacetFilters(page, () => { render(); refreshAnalytics(true); }));
    // Until the range's answer is here (the first time a range or filter is asked for), the page says so.
    if (!A) { const wait = el("p", "empty", AN.error ? "Couldn't load Analytics: " + AN.error : "Loading…"); wait.setAttribute("role", "status"); put(wait); put.done(); return; }
    // An answer kept from before a request that failed is still drawn, under the error.
    if (AN.error) { const stale = el("p", "empty", "Couldn't update Analytics: " + AN.error + ". Showing the last answer."); stale.setAttribute("role", "status"); put(stale); }
    const now = A.current, previous = A.previous;
    // A card with an explanation carries it for a screen reader all the time (a hidden node it is described by, so it is read once, as the description); the tooltip shows it to a pointer,
    // and the card takes keyboard focus so the tooltip is reachable.
    const metrics = el("div", "analytics-metrics"), addMetric = (label, value, note, more, tip = false) => { const m = el("div", "analytics-metric"), l = el("div", "label"); l.append(el("span", null, label)); if (tip) l.append(costInfoTip()); if (more) { const why = document.createElement("span"); why.hidden = true; why.textContent = more; why.id = "metric-more-" + (++metricSeq); m.dataset.more = ""; m.dataset.tip = more; m.tabIndex = 0; m.setAttribute("aria-describedby", why.id); m.append(why); } m.append(l, el("div", "value", value), note); metrics.append(m); }, pct = (errors, tools) => tools ? Math.round(errors / tools * 100) + "%" : "0%";
    addMetric("Agent-hours", hoursText(now.agent_ms), deltaNote(now.agent_ms, previous.agent_ms, hoursText), "Busy time summed across sessions; two sessions busy for an hour count two hours.");
    const costNote = now.cost.usd == null || previous.cost.usd == null ? el("div", "note", "no price for " + [...new Set([...now.cost.unpriced_models, ...previous.cost.unpriced_models])].join(", ")) : deltaNote(now.cost.usd, previous.cost.usd, asMoney);
    addMetric(A.days === 1 ? "Cost today (UTC)" : "Cost, last " + A.days + " UTC days", now.cost.usd == null ? "—" : asMoney(now.cost.usd), costNote, A.days === 1 ? "API-equivalent cost. Cost is recorded per UTC day: this is the whole current UTC day so far, compared with the whole day before." : "API-equivalent cost. Cost is recorded per UTC day: the last " + A.days + " UTC days count, today so far, compared with the " + A.days + " whole UTC days before.", true);
    addMetric("Sessions started", countText(now.started), deltaNote(now.started, previous.started, countText)); addMetric("Turns", countText(now.turns), deltaNote(now.turns, previous.turns, countText));
    const toolNote = deltaNote(now.tools, previous.tools, countText), unavailable = A.calls_unknown;
    if (unavailable) toolNote.append(" · — for " + unavailable + (unavailable === 1 ? " session" : " sessions"));
    addMetric("Tool calls", countText(now.tools), toolNote, countText(now.errors) + " failed (" + pct(now.errors, now.tools) + ") · previous " + rangeName() + ": " + countText(previous.errors) + " failed (" + pct(previous.errors, previous.tools) + ")");
    addMetric("Peak concurrency", countText(now.peak), deltaNote(now.peak, previous.peak, countText), "The most sessions busy at the same moment.");
    addMetric("Waited on you", timeText(now.wait_ms), deltaNote(now.wait_ms, previous.wait_ms, timeText), "Median wait " + timeText(now.median_wait_ms) + " · previous " + rangeName() + ": " + timeText(previous.median_wait_ms));
    const currentWait = A.longest_current_wait; addMetric("Longest current wait", currentWait ? timeText(currentWait.ms) : "—", deltaNote(currentWait ? currentWait.ms : 0, previous.longest_wait_ms, timeText), currentWait ? nameOfSid(A, currentWait.sid) + " has waited on you for " + timeText(currentWait.ms) : "No session is waiting on you"); put(metrics);
    const breakdowns = el("div", "analytics-breakdowns"); breakdowns.append(analyticsBreakdown("By repo", A.breakdown.repo, "repo"), analyticsBreakdown("By machine", A.breakdown.machine, "machine"), analyticsBreakdown("By harness and model", A.breakdown.model, "harness"));
    // The Breakdown is one box: its title, the strip that stays pinned under the top bar (the measure toggle, a persistent control: the toggle's state is the page's), and the groups,
    // whose headings pin under the strip. The strip has to share a parent with the groups to stay while any of them is in view.
    const bdBox = slot("breakdown", page, () => {
      const box = el("div", "analytics-breakdown"), head = el("div", "analytics-bd-head"), strip = el("div", "analytics-bd-bar"), toggle = el("div", "analytics-measure"); head.append(el("h2", null, "Breakdown"), el("div", "panel-sub", "Agent-hours and API-equivalent cost; bars follow the toggle"));
      toggle.setAttribute("role", "group"); toggle.setAttribute("aria-label", "Breakdown bar measure");
      for (const [key, label] of [["hours", "Agent-hours"], ["cost", "API-equivalent cost"]]) { const b = el("button", null, label); b.type = "button"; b.dataset.measure = key; b.addEventListener("click", () => { if (analyticsMeasure === key) return; const top = currentScroll(); analyticsMeasure = key; render(); restoreScroll(top); }); toggle.append(b); }
      strip.append(toggle); box.append(head, strip); return box;
    }).el;
    for (const b of bdBox.querySelectorAll(".analytics-measure button")) b.setAttribute("aria-pressed", String(analyticsMeasure === b.dataset.measure));
    const oldGroups = bdBox.querySelector(":scope > .analytics-breakdowns"); if (oldGroups) oldGroups.replaceWith(breakdowns); else bdBox.append(breakdowns);
    const bottom = el("div", "analytics-split");
    bottom.append(analyticsList(A, "Top sessions · busy time", A.top.busy, (x) => timeText(x.ms), (x) => x.ms), analyticsList(A, "Top sessions · waited on", A.top.waited, (x) => timeText(x.ms), (x) => x.ms), analyticsList(A, "Most expensive sessions · API-equivalent cost", A.top.cost, (x) => x.usd == null ? "—" : asMoney(x.usd), (x) => x.usd ?? 0));
    put(renderAgentsChart(A), renderCostChart(A), bdBox, bottom); const models = renderModelsComparison(A); if (models) put(models); const allowance = renderCodexAllowance(A.allowance); if (allowance) put(allowance);
    // data-analytics-ready: the figures and charts are drawn (a stable hook for the budget check); data-query: for which range and filters.
    metrics.dataset.analyticsReady = ""; metrics.dataset.query = analyticsQuery();
    put.done();
  }

  // ---- Sessions: every top-level session and its child runs ---------------------------------------------------------------
  const laneOf = (sid) => { const seen = new Set(); while (parentOf(sid) && !seen.has(sid)) { seen.add(sid); sid = parentOf(sid); } return sid; };
  function childRuns(sid) {
    const kids = Object.values(SESS).filter((x) => x.id !== sid && laneOf(x.id) === sid && (showApprovalReviews || !isApprovalReview(x))), sub = kids.filter((x) => x.kind === "Subagent").length, cdx = kids.filter((x) => x.kind === "Codex run").length, other = kids.length - sub - cdx;
    return [sub ? sub + (sub === 1 ? " subagent" : " subagents") : null, cdx ? cdx + (cdx === 1 ? " Codex run" : " Codex runs") : null, other ? other + (other === 1 ? " other run" : " other runs") : null].filter(Boolean).join(" · ");
  }
  function renderSessions(page, focusSearch = false) {
    const all = Object.values(SESS).filter((s) => (showApprovalReviews || !isApprovalReview(s)) && matchesSessionFacets(s));
    const head = el("div", "ph"); const h1 = el("h1", null, "Sessions"); head.append(h1);
    const sub = el("div", "sub"); for (const [v, l] of [[all.length, all.length === 1 ? "session" : "sessions"], [all.filter((s) => s.state === "work").length, "working"], [all.filter((s) => s.state === "wait").length, "waiting on you"]]) { const x = el("span"); x.append(el("b", null, String(v)), l); sub.append(x); }
    head.append(sub); const put = placer(page); put(head); observeTitle(h1);
    // The search field and the group-by buttons are persistent controls: the page's redraws keep them, and the field's focus, text and caret.
    const found = slot("find", page, (ctx) => {
      const fr = el("label", "find"), fi = el("input"); fi.id = "sq"; fi.type = "search"; fi.placeholder = "Search sessions"; fi.setAttribute("aria-label", "Search sessions"); fi.value = query; fr.append(icon(I.search), fi);
      fi.addEventListener("input", () => { query = fi.value.trim(); const address = new URL(location.href); if (query) address.searchParams.set("q", query); else address.searchParams.delete("q"); history.replaceState(history.state, "", address); ctx.draw(); }); return fr;
    }), fr = found.el, fi = fr.querySelector("input");
    if (fi.value.trim() !== query) fi.value = query;
    const grouped = slot("groupby", page, (ctx) => {
      const gb = el("div", "groupby"); gb.setAttribute("role", "group"); gb.setAttribute("aria-label", "Group by");
      for (const [g, label] of [["recent", "Recent"], ["project", "Project"], ["machine", "Machine"], ["harness", "Harness"]]) { const b = el("button", null, label); b.type = "button"; b.dataset.g = g; b.addEventListener("click", () => { groupBy = g; ctx.draw(); }); gb.append(b); }
      return gb;
    }), gb = grouped.el;
    const reviewControl = slot("review-toggle", page, () => {
      const group = el("div", "groupby"); group.setAttribute("role", "group"); group.setAttribute("aria-label", "Session visibility");
      const toggle = el("button", null, "Show approval reviews"); toggle.type = "button"; toggle.dataset.showApprovalReviews = ""; toggle.setAttribute("aria-pressed", String(showApprovalReviews));
      toggle.addEventListener("click", () => { showApprovalReviews = !showApprovalReviews; ORD.delete("page"); ORD.delete("side"); render(); });
      group.append(toggle); return group;
    }), reviewToggle = reviewControl.el.querySelector("button[data-show-approval-reviews]");
    const hasApprovalReviews = Object.values(SESS).some(isApprovalReview), showReviewControl = hasApprovalReviews || showApprovalReviews;
    reviewToggle.setAttribute("aria-pressed", String(showApprovalReviews));
    const out = el("div", "sess"); out.style.display = "grid"; out.style.gap = "16px";
    const draw = () => {
      out.replaceChildren(); const lanes = all.filter((s) => sessMatch(s, query));
      let groups; const order = orderScope("page", pageSig(), route, pageState()); // each group's rows keep their order; its count is the whole group's
      if (groupBy === "recent") groups = [["", orderList(order, "recent", lanes, byLast), lanes.length]];
      else { const key = { machine: (s) => MACHINE[s.machine], project: (s) => s.repo ?? "No repo (roles)", harness: (s) => HARNESS[s.harness] }[groupBy]; const keys = [...new Set(lanes.map(key))].sort((a, b) => a.startsWith("No repo") - b.startsWith("No repo") || a.localeCompare(b)); groups = keys.map((k) => { const xs = lanes.filter((s) => key(s) === k); return [k, orderList(order, "g:" + k, xs, byLast), xs.length]; }); }
      for (const [title, items, total] of groups) {
        if (!items.length && total) continue; // a group new to a list that is held: its sessions are counted in the pill
        const box = el("div"); box.style.display = "grid"; if (title) { const head = secHead(title, total); if (groupBy === "harness") { const id = Object.keys(HARNESS).find((k) => HARNESS[k] === title); if (id) withHarnessIcon(head, id, { size: 14, lead: false }); } box.append(head); }
        const list = el("div", "list");
        for (const s of items) list.append(sessionRow(s));
        box.append(list); out.append(box);
      }
      if (!lanes.length) out.append(el("p", "empty", "No sessions match “" + query + "”."));
      for (const b of gb.querySelectorAll("button[data-g]")) b.setAttribute("aria-pressed", String(b.dataset.g === groupBy));
      renderLanes();
    };
    found.ctx.draw = grouped.ctx.draw = draw;
    const nodes = [renderFacetFilters(page, () => render()), fr, gb]; if (showReviewControl) nodes.push(reviewControl.el); nodes.push(out);
    put(...nodes); put.done(); draw();
    if (focusSearch) fi.focus({ preventScroll: true });
  }

  // ---- Drawer (phone) ---------------------------------------------------------------------------------------------------------
  const sidebar = $("#sidebar");
  function openDrawer() { if (!phone.matches) return; orderApply("side"); document.body.classList.add("drawer-open"); $("#lead-btn")?.setAttribute("aria-expanded", "true"); }
  function closeDrawer(quiet) { if (!document.body.classList.contains("drawer-open")) return; document.body.classList.remove("drawer-open"); closeAccountMenu(); const b = $("#lead-btn"); b?.setAttribute("aria-expanded", "false"); if (!quiet) b?.focus(); setTimeout(() => { if (phone.matches && !document.body.classList.contains("drawer-open")) orderApply("side"); }, ORD_DRAWER_MS); }
  // On an embedding page shell.js opens and closes the drawer, and names its opening (semon:drawer-open); the viewer binds none of it, "/" included.
  // On a phone there is no drawer-close event from the embedding page, so held order waits for the next open.
  if (!SIDEBAR_ONLY) { $("#drawer-close").addEventListener("click", () => closeDrawer()); $("#scrim").addEventListener("click", () => closeDrawer()); }
  else window.addEventListener("semon:drawer-open", () => orderApply("side")); // opening the drawer re-sorts what the list held, as openDrawer does
    if (!SIDEBAR_ONLY) document.addEventListener("keydown", (e) => { if (e.key === "Escape" && accountSheet) accountChrome.escape(); else if (e.key === "Escape" && !viewerEl) { closeDrawer(); closeAccountMenu(); $(".session-menu")?.remove(); $("#more-btn")?.setAttribute("aria-expanded", "false"); } if (e.key === "/" && !/INPUT|TEXTAREA/.test(document.activeElement?.tagName ?? "") && !document.activeElement?.isContentEditable && !viewerEl) { e.preventDefault(); if (route.v === "session") { findOpen = true; render(); $("#find")?.focus(); } else if (route.v === "sessions") { $("#sq")?.focus(); } else { const r = { v: "sessions", q: query }; focusSessionsSearchOnRender = r; go(r); } } });
  let sx = null;
  if (!SIDEBAR_ONLY) sidebar.addEventListener("touchstart", (e) => { sx = e.touches[0].clientX; }, { passive: true });
  if (!SIDEBAR_ONLY) sidebar.addEventListener("touchmove", (e) => { if (sx !== null && e.touches[0].clientX - sx < -50) { sx = null; closeDrawer(); } }, { passive: true });
  phone.addEventListener("change", () => {
    closeDrawer(true); syncLayoutPrefs(); expandedAll = null; renderLanes();
    if (!phone.matches && viewerEl?.classList.contains("kids-sheet")) viewerEl.close(); // a sheet is a phone's: a wide screen opens the list in the tree
    if (route.v === "session" || route.v === "analytics") { const top = currentScroll(); render(); restoreScroll(top); }
    else renderLanes();
    const l2 = $("#topbar .meta-line"); if (l2 && route.v === "session") fitMeta(l2); syncJump();
  });

  // ---- Live updates (deliberate difference 3) --------------------------------------------------------------------------------
  // Every screen polls /api/model?since= every 2 s while the tab is visible, one request at a time: it backs off up to 30 s
  // on errors and stops on 403 (the server restarted with a new token). A new model swaps the globals and draws the screen
  // again with its view state kept: the scroll position, anchored to the first visible block; what is open, by stable keys;
  // focus, find and filters; the drawer. A session page follows its transcript's tail
  // with /api/tx?after= and replaces only the turns that changed. An open View all sheet holds the redraw until it closes.
  const LIVE = { late: null, lateTries: 0, retry: false, version: null, timer: null, due: 0, busy: false, started: -Infinity, delay: 2000, ended: false, again: false, pending: false, fresh: 0, turns: new Map() };
  // The turn index as last drawn, to tell which turns an update changed.
  const remember = (m) => { LIVE.turns = new Map(m.turns.map((x) => [x.id, turnKey(x)])); };
  // What an update can change in a turn record or a handoff, cheaply (not the text, which a record never rewrites).
  const turnKey = (x) => [x.start, x.end?.st, x.end?.why, x.end?.h, x.sent.join(","), x.last ? 1 : 0].join("|");
  const handKey = (h) => [h.status, h.to, h.done, h.result?.length, h.answer?.length, h.answers?.length, h.declined ? 1 : 0].join("|");
  let rendered = null; // the route the page shows
  const visible = () => document.visibilityState === "visible";
  function schedule(ms) { clearTimeout(LIVE.timer); LIVE.timer = null; if (!LIVE.ended && visible()) { LIVE.due = performance.now() + ms; LIVE.timer = setTimeout(poll, ms); } }
  document.addEventListener("visibilitychange", () => {
    if (!visible()) { clearTimeout(LIVE.timer); LIVE.timer = null; } else if (LIVE.version && !LIVE.busy && !LIVE.timer) schedule(LIVE.delay > 2000 ? LIVE.delay : 0); }); // a backoff in progress holds
  // An embedding page can ask for a poll soon (it knows something changed) with `semon:refresh`. The rule, one floor:
  // a refresh starts a poll no sooner than REFRESH_FLOOR after the previous poll started, and never touches the backoff.
  // - Refreshes inside that second merge into the one poll at its end; a pending timer that is due sooner is kept.
  // - During a poll, a refresh asks for one follow-up (its request may have left before the change), at the same floor.
  // - The backoff is only what polls set: a refresh-driven poll that succeeds resets it to 2 s like any other, and one that
  //   fails doubles it. So a listener that refreshes on every `semon:polled` polls at most once a second, even against a
  //   server answering 500, and when it stops, the timer resumes at the backoff the failures set.
  // Ignored before the first model has loaded and after the session ended.
  const REFRESH_FLOOR = 1000;
  const floorWait = () => Math.max(0, LIVE.started + REFRESH_FLOOR - performance.now());
  window.addEventListener("semon:refresh", () => {
    if (!LIVE.version || LIVE.ended) return;
    if (LIVE.busy) { LIVE.again = true; return; }
    const wait = floorWait();
    if (!LIVE.timer || performance.now() + wait < LIVE.due) schedule(wait);
  });
  function poll() {
    LIVE.timer = null; if (LIVE.busy || LIVE.ended || !visible()) return; LIVE.busy = true; LIVE.started = performance.now(); LIVE.retry = false;
    let ok = false;
    // While a late origin's transcript has yet to load again, the poll asks for the whole model (an empty since), so the update
    // that retries it runs even when nothing else moved.
    fetch("/api/model?delta=1&since=" + enc(LIVE.late ? "" : LIVE.version ?? ""), { credentials: "same-origin" })
      .then((r) => r.status === 304 ? null : r.ok ? r.json() : Promise.reject(Object.assign(new Error(r.status + " " + r.statusText), { status: r.status })), (e) => Promise.reject(Object.assign(e, { status: 0 })))
      .then((m) => { if (!m) return null; try { m = applyModelDelta(m); } catch { return api("/api/model?delta=1").then(update); } return update(m); })
      // A failed retry of that transcript backs off like a failed poll (4 s, 8 s, 16 s, capped at 30 s); anything else resets to 2 s.
      .then(() => { LIVE.delay = LIVE.retry ? Math.min(30000, LIVE.delay * 2) : 2000; ok = true; }, (err) => {
        if (err?.status === 403) return ended(403);
        LIVE.delay = Math.min(30000, LIVE.delay * 2);
        if (err?.status == null) setTimeout(() => { throw err; }); // not the network: a fault on the page, reported as one
      })
      .finally(() => {
        LIVE.busy = false; schedule(LIVE.again ? floorWait() : LIVE.delay); LIVE.again = false;
        // Once per poll, after the next one is scheduled. ok: the server answered 200 or 304 and the update drew.
        window.dispatchEvent(new CustomEvent("semon:polled", { detail: { ok } }));
      });
  }
  // An embedding page can cancel `semon:ended` to draw its own note in place of this one.
  function ended(status) {
    LIVE.ended = true; clearTimeout(LIVE.timer); LIVE.timer = null; if ($(".livenote, .livenote-side")) return;
    if (!window.dispatchEvent(new CustomEvent("semon:ended", { cancelable: true, detail: { status } }))) return;
    const n = el("p", SIDEBAR_ONLY ? "ghead livenote-side" : "livenote", SIDEBAR_ONLY ? "Sessions stopped updating: reload the page" : "Session ended: reload with the printed URL"); n.setAttribute("role", "status");
    if (SIDEBAR_ONLY) $("#lanes").after(n); else document.body.append(n); // on an embedding page, under the list that stopped
  }
  // A 403 or a dropped connection fails the update (and backs off); anything else skips that one transcript.
  const soft = (p) => p.catch((e) => { if (e?.status === 403 || e?.status === 0) throw e; });
  // The transcript on screen: a session page's own. Any other loaded transcript is dropped from TX (the last few opened are kept in
  // TXCACHE, and brought up to date when opened again). A child run's card is drawn from the model, so its transcript is not loaded.
  const viewed = () => { const v = new Set(); if (route.v === "session") v.add(route.id); return v; };
  // What a child card shows, so an update knows which cards changed: the run's name, state, kind, model, steps and current call.
  const cardKeys = () => new Map(H.filter((h) => h.kind === "spawn" && SESS[h.to]).map((h) => { const c = SESS[h.to]; return [h.id, [c.name, c.state, c.kind, c.model, countOf(c, "calls"), c.activity?.join("|"), h.status, h.result].join("\u0001")]; }));
  function applyModelDelta(m) {
    if (m.delta !== 1) return m;
    if (!wireModel || m.from !== wireModel.version) throw new Error("Model delta base expired");
    const next = { ...wireModel, ...m.set };
    for (const field of m.remove ?? []) delete next[field];
    for (const [field, patch] of Object.entries(m.collections ?? {})) {
      const array = field === "handoffs" || field === "turns";
      const rows = array ? new Map((wireModel[field] ?? []).map((row) => [row.id, row])) : new Map(Object.entries(wireModel[field] ?? {}));
      for (const id of patch.remove ?? []) rows.delete(id);
      for (const [id, row] of Object.entries(patch.set ?? {})) rows.set(id, row);
      next[field] = array ? (patch.order ?? [...rows.keys()]).map((id) => rows.get(id)) : Object.fromEntries(rows);
      if (array && next[field].some((row) => !row)) throw new Error("Incomplete model delta");
    }
    next.version = m.version;
    return next;
  }
  function update(m) {
    m = applyModelDelta(m);
    const oldH = new Map(H.map((h) => [h.id, handKey(h)])), oldT = LIVE.turns, oldCards = cardKeys(), names = new Map(Object.values(SESS).map((x) => [x.id, x.name]));
    const hadOrigins = new Set([...TXCACHE.keys()].filter((sid) => !!originHandoff(sid)));
    const hadOrigin = route.v === "session" && !!SESS[route.id] && !!originHandoff(route.id);
    adopt(m); remember(m);
    for (const sid of STALE_BRIEFS) if (!SESS[sid]) STALE_BRIEFS.delete(sid);
    for (const sid of [...TXCACHE.keys()]) if (!hadOrigins.has(sid) && originHandoff(sid)) TXCACHE.delete(sid);
    // A page that had no origin and now has one (its parent's spawn arrived) loads its transcript again: the first prompt it drew as
    // a message is the brief, which the intro now shows. A failed request is retried on the next poll, which backs off, up to
    // LATE_TRIES requests in all; after that the page stays as drawn (the brief shows twice until a reload) and polls as usual.
    if (LIVE.late !== route.id) { if (LIVE.late) TXCACHE.delete(LIVE.late); LIVE.late = null; }
    if (route.v === "session" && !hadOrigin && !!SESS[route.id] && !!originHandoff(route.id)) { LIVE.late = route.id; LIVE.lateTries = 0; STALE_BRIEFS.add(route.id); TXCACHE.delete(route.id); }
    if (LIVE.late && !TX[LIVE.late]) LIVE.late = null; // nothing loaded to load again: the page loads it with its origin
    const changedH = new Set(H.filter((h) => oldH.get(h.id) !== handKey(h)).map((h) => h.id));
    const newCards = cardKeys(), changedCards = new Set([...newCards].filter(([id, k]) => oldCards.has(id) && oldCards.get(id) !== k).map(([id]) => id));
    const view = viewed(), grown = new Set(), cuts = new Map(), patched = new Map();
    let full = Object.values(SESS).some((x) => names.has(x.id) && names.get(x.id) !== x.name); // a new name shows in every turn
    for (const sid of Object.keys(TX)) { if (view.has(sid) && SESS[sid]) spread(sid); else dropTx(sid); }
    // Only transcripts whose mark in the model moved are asked for, one at a time.
    let chain = Promise.resolve();
    for (const sid of view) if (TX[sid] && LIVE.late === sid) chain = chain.then(() => soft(reloadLate(sid).then(() => { grown.add(sid); full = true; })));
    else if (TX[sid] && TXM[sid].tok != null && TOK[sid] != null && shrank(TXM[sid].tok, TOK[sid])) chain = chain.then(() => soft(reload(sid).then(() => { grown.add(sid); full = true; })));
    else if (TX[sid] && TXM[sid].to >= TXM[sid].total && TXM[sid].tok !== TOK[sid]) chain = chain.then(() => soft(tail(sid).then((r) => { grown.add(sid); if (r.cut != null) cuts.set(sid, r.cut); if (r.patched?.length) patched.set(sid, r.patched); if (r.reload) full = true; })));
    for (const sid of view) if (TX[sid] && TXM[sid].to < TXM[sid].total && TXM[sid].watchTok !== TOK[sid]) chain = chain.then(() => soft(watchLater(sid).then((r) => { if (r?.reload) full = true; })));
    return chain.then(() => { LIVE.version = m.version; refresh(full ? null : dirtyTurns(cuts, grown, changedH, oldT, changedCards, patched)); if (route.v === "analytics") refreshAnalytics(); const e = errorsLive(); return e && soft(e); });
  }
  // The turns of the session page an update changed: those holding entries its tail brought (from the cut on), or a background
  // call it updated, those whose record or handoffs changed, and those holding the spawn of a child run that grew. Null: draw
  // them all.
  function dirtyTurns(cuts, grown, changedH, oldT, changedCards, patched) {
    if (route.v !== "session" || !TX[route.id]) return null;
    const sid = route.id, dirty = new Set(), owner = new Map((TURNS[sid] ?? []).flatMap((t) => t.entries.map((e) => [e, t.id])));
    if (cuts.has(sid)) for (const e of TX[sid].slice(cuts.get(sid))) { if (isGap(e)) return null; if (owner.has(e)) dirty.add(owner.get(e)); }
    for (const e of patched?.get(sid) ?? []) { if (!owner.has(e)) return null; dirty.add(owner.get(e)); }
    for (const t of TURNS[sid] ?? []) {
      if (oldT.get(t.id) !== LIVE.turns.get(t.id) || [t.start, ...t.sent].some((h) => h && changedH.has(h.id)) || t.entries.some((e) => e.k === "h" && changedH.has(e.id))) dirty.add(t.id);
    }
    for (const h of H) if (h.kind === "spawn" && h.from === sid && changedCards.has(h.id) && HOLDS.get(h.id)) dirty.add(HOLDS.get(h.id).id);
    return dirty;
  }
  // A deep link keeps its contiguous window. Learn that its unseen tail grew without replacing the reader's page.
  function watchLater(sid) {
    const m = TXM[sid], tok = TOK[sid];
    return api("/api/tx?sid=" + enc(sid) + "&after=" + m.total).then((p) => {
      if (TXM[sid] !== m) return;
      if (p.total < m.total) return reload(sid);
      m.newer = (m.newer ?? 0) + Math.max(0, p.total - m.total);
      Object.assign(m, { total: p.total, calls: p.calls, errors: p.errors, watchTok: tok });
    });
  }
  // The tail of a transcript loaded to its end: from its first foreground call still running, or the last turn's first call
  // with no result yet, since its result may have landed; else from the end. A file that shrank or was rewritten, or more
  // than five pages of new entries, loads the last page again instead.
  function tail(sid) {
    const es = TX[sid], m = TXM[sid], tok = TOK[sid];
    let cut = es.findIndex((e) => e.live && e.slot != null); if (cut < 0) cut = es.length;
    for (let i = es.length - 1; i >= 0; i--) { if (es[i].unfinished && es[i].slot != null) cut = Math.min(cut, i); if (es[i].turn) break; }
    let got = [], last = null, n = 0;
    const page = (after) => api("/api/tx?sid=" + enc(sid) + "&after=" + after).then((p) => {
      got = got.concat(p.entries.map((e) => txEntry({ ...e, sid }))); last = p;
      if (p.to < p.total && p.entries.length && ++n < 5) return page(p.to);
    });
    return page(cut < es.length ? es[cut].slot : m.to).then(() => {
      if (TX[sid] !== es) return { cut: null }; // "Load earlier" ran meanwhile: the next update catches up
      if (last.total < m.total || last.to < last.total || (cut < es.length && got[0]?.slot !== es[cut].slot)) return reload(sid);
      // A background call loaded before the cut is not fetched again (it may be pages back): its finish row in the tail, or the
      // page's list of calls still running (bg_running), says how it stands. Its turn is drawn again (patched).
      const ends = new Map(got.filter((e) => e.k === "bgend" && e.bg).map((e) => [e.call, e.bg])), still = new Set(last.bg_running ?? []), patched = [];
      for (const e of es.slice(0, cut)) if (e.bg) {
        const next = ends.get(e.tid) ?? (still.has(e.tid) ? { ...e.bg, state: "running", secs: e.bg.secs ?? "—" } : e.bg.state === "running" ? { state: "unknown" } : e.bg);
        if (JSON.stringify(next) !== JSON.stringify(e.bg)) { e.bg = next; patched.push(e); }
      }
      TX[sid] = es.slice(0, cut).concat(got); Object.assign(m, { to: last.to, total: last.total, calls: last.calls, errors: last.errors, tok }); spread(sid);
      return { cut, patched };
    });
  }
  // The page's own transcript loads its last page again; a child run's is dropped, and loads again as new child work.
  function reload(sid) {
    if (sid !== route.id) { dropTx(sid); return Promise.resolve({ cut: null, reload: true }); }
    return fetchTx(sid, "").then(() => ({ cut: null, reload: true }));
  }
  // The reload of a page whose origin arrived late (update): done once it loads; a failure is counted, makes the poll back off, and
  // after LATE_TRIES requests gives up.
  const LATE_TRIES = 4;
  function reloadLate(sid) {
    return reload(sid).then((r) => { if (LIVE.late === sid) LIVE.late = null; return r; }, (err) => {
      if (LIVE.late === sid) {
        if (++LIVE.lateTries >= LATE_TRIES) { LIVE.late = null; console.warn("semon: gave up reloading the transcript of " + sid + " after its origin arrived"); }
        else LIVE.retry = true;
      }
      throw err;
    });
  }
  // Draws the new model on the screen shown, unless a navigation is still loading (it draws when done), the sheet is open
  // (it draws when the sheet closes), or what the screen shows left the model (it stays as it was).
  function refresh(dirty) {
    if (accountChrome.open && !$(".account-popover")?.isConnected) closeAccountMenu(); // a menu some redraw took away is closed
    if (viewerEl || accountChrome.open) { LIVE.pending = true; return; } // drawn whole when the sheet or the account menu closes
    if (SIDEBAR_ONLY) { LIVE.pending = false; render(); return; } // the page is the embedding page's: only the sidebar is redrawn
    LIVE.pending = false; const r = route;
    if (rendered !== r || (r.v === "session" && !SESS[r.id]) || (r.v === "trace" && !SESS[r.sid]) || (r.v === "machine" && !MACHINE[r.id])) return;
    const st = capture(); $("#page").style.paddingBottom = "";
    if (r.v !== "session") { render(); restore(st); return; }
    const n = patchSession(dirty); restore(st, st.bottom);
    if (!st.bottom && n) LIVE.fresh += n;
    syncJump();
  }

  // View state. A block's identity: its class and keys, or its own text when it has no key (a section heading).
  const scroller = () => (phone.matches ? document.scrollingElement : $("#main"));
  const edge = () => $("#topbar").getBoundingClientRect().bottom;
  // The opened transcript can still grow as fonts and clamped cards settle; hold the tail briefly, then yield on reader input.
  let openingEndUntil = 0, openingEndTimer = null, openingEndObserver = null;
  function stopOpeningEndPin() {
    const wasPinned = !!openingEndUntil;
    openingEndUntil = 0;
    clearTimeout(openingEndTimer); openingEndTimer = null;
    openingEndObserver?.disconnect(); openingEndObserver = null;
    if (wasPinned) queuePagerObservers();
  }
  function pinOpeningEnd() {
    if (route.v !== "session" || performance.now() >= openingEndUntil) { stopOpeningEndPin(); return; }
    const sc = scroller(); scrollProgrammatically(() => { sc.scrollTop = sc.scrollHeight; }); LIVE.anchor = null; syncJump(); saveHistoryScroll();
  }
  function startOpeningEndPin() {
    resetPagerInput();
    stopOpeningEndPin(); if (route.v !== "session" || location.hash) return;
    openingEndUntil = performance.now() + 2000;
    disconnectPagerObservers();
    const turns = $("#page section[aria-label='Transcript'] .turns");
    if (turns) { openingEndObserver = new ResizeObserver(pinOpeningEnd); openingEndObserver.observe(turns); }
    pinOpeningEnd(); openingEndTimer = setTimeout(stopOpeningEndPin, 2000);
  }
  const ANCHORS = "[data-e], .turn, .hop, .ib, .nrow, .sec-h, .ph, .divider, .analytics-metric, .analytics-panel, .facet-filters, .groupby, .find, .empty";
  const HOSTS = "[data-e], [data-h], [data-id], [data-sid], [data-go], [data-turn], [data-g], [data-m]";
  const FOCUSABLE = "button, input, [tabindex], a[href]";
  const stateKey = (n) => n.dataset.entryKey ?? n.dataset.e;
  const identOf = (n) => { const d = n.dataset, keys = [stateKey(n), d.turn, d.h, d.id, d.m, d.sid, d.go, d.g];
    return [n.classList[0], ...keys, keys.some((x) => x != null) ? "" : n.firstChild?.nodeType === 3 ? n.firstChild.data : ""].map((x) => x ?? "").join("|"); };
  // Each anchor candidate on the page, in order, with its identity made unique by how many came before it.
  function anchors(fn) { const seen = new Map(); for (const n of $("#page").querySelectorAll(ANCHORS)) { const id = identOf(n), k = seen.get(id) ?? 0; seen.set(id, k + 1); if (fn(n, id + "#" + k)) return; } }
  const opener = (n) => n.classList.contains("step") ? n.querySelector(":scope > button") : n.classList.contains("tgroup") ? n.querySelector(":scope > .tsum") : null;
  function capture() {
    const sc = scroller(), line = edge();
    const st = { top: sc.scrollTop, bottom: sc.scrollHeight - sc.scrollTop - sc.clientHeight <= 80, anchor: null, open: new Set(), groups: new Set(), groupMembers: new Set(), focus: null, drawer: document.body.classList.contains("drawer-open") };
    // The first block, innermost, still visible under the bar, and how far its top is from the bar. While the reader hasn't
    // scrolled since the last redraw placed it, that placement's block and offset are kept as they were, so the fraction
    // of a pixel each placement rounds off can't add up over many updates.
    const kept = LIVE.anchor; let still = null;
    if (kept && kept.route === rendered && Math.abs(sc.scrollTop - kept.top) < 1) anchors((n, id) => (id === kept.id ? (still = n) : false));
    // ... and only while that block is still where it was placed (a filter, find or zoom since has moved it).
    if (still && Math.abs(still.getBoundingClientRect().top - line - kept.off) <= 1) st.anchor = { id: kept.id, off: kept.off };
    else {
      const turnBounds = new Map();
      anchors((n, id) => {
        // Hidden turn contents need no measurements: only the enclosing turn's box can be near the reader.
        const turn = n.closest(".turn");
        if (turn) {
          let bounds = turnBounds.get(turn); if (!bounds) { bounds = turn.getBoundingClientRect(); turnBounds.set(turn, bounds); }
          if (bounds.bottom <= line || bounds.top >= (phone.matches ? innerHeight : sc.getBoundingClientRect().bottom)) return false;
        }
        if (n.querySelector(ANCHORS)) return false;
        const b = n.getBoundingClientRect(); if (!b.height || b.bottom <= line) return false;
        st.anchor = { id, off: b.top - line }; return true;
      });
    }
    for (const n of $("#page").querySelectorAll("[data-e]")) {
      if (n.classList.contains("tgroup")) st.groups.add(stateKey(n));
        if (opener(n)?.getAttribute("aria-expanded") === "true") for (const step of n.querySelectorAll(".step[data-e]")) st.groupMembers.add(stateKey(step));
      if (opener(n)?.getAttribute("aria-expanded") === "true" || (n.classList.contains("event") && n.querySelector(":scope > .ev-text.open"))) st.open.add(stateKey(n));
    }
    for (const n of $("#page").querySelectorAll(".hop")) if (n.querySelector(".brief.open")) st.open.add("hop:" + identOf(n));
    const a = document.activeElement;
    if (a && a !== document.body && !a.closest("dialog")) {
      const host = a.id ? null : a.closest(HOSTS), sel = host && host !== a ? a.tagName.toLowerCase() + [...a.classList].map((c) => "." + CSS.escape(c)).join("") : null;
      // By id, else by its keyed block and place in it, else by its label, else by its place among the page's controls.
      st.focus = { id: a.id || null, host: host ? identOf(host) : null, sel, i: sel ? [...host.querySelectorAll(sel)].indexOf(a) : 0, range: null,
        label: a.getAttribute("aria-label"), at: [...$("#page").querySelectorAll(FOCUSABLE)].indexOf(a), of: $("#page").querySelectorAll(FOCUSABLE).length,
        foot: a.closest("#page > .session-foot") ? a.closest("[data-foot]")?.dataset.foot ?? null : null };
      try { if (typeof a.selectionStart === "number") st.focus.range = [a.selectionStart, a.selectionEnd]; } catch {}
    }
    return st;
  }
  function restore(st, pin) {
    const all = (sel) => [...$("#page").querySelectorAll(sel)], r0 = rendered, revision = scrollRevision;
    // A new card or brief measures its "Show more" now, as its ResizeObserver would a frame later, so nothing moves after the
    // scroll position is set.
    const clamp = (br, more) => { if (!br || !more || !br.clientHeight) return; const x = br.scrollHeight > br.clientHeight + 1; more.hidden = !x; br.classList.toggle("clipped", x); };
    for (const c of all(".event")) clamp(c.querySelector(":scope > .ev-text"), c.querySelector(":scope > .ev-more"));
    for (const br of all(".hop .body > .brief:not(.open)")) clamp(br, br.parentElement.querySelector(":scope > .more"));
    // Groups first (a new one opens if it holds an open step), then steps and events.
    for (const n of all(".tgroup[data-e]")) {
      const want = st.groups.has(stateKey(n)) ? st.open.has(stateKey(n)) : [...n.querySelectorAll(".step[data-e]")].some((x) => st.open.has(stateKey(x)) || st.groupMembers.has(stateKey(x)));
      if (want && opener(n).getAttribute("aria-expanded") === "false") opener(n).click();
    }
    for (const n of all(".step[data-e]")) if (st.open.has(stateKey(n)) && opener(n)?.getAttribute("aria-expanded") === "false") opener(n).click();
    // An event opened before its size was measured: its "Show less" is shown by hand.
    for (const n of all(".event[data-e]")) if (st.open.has(stateKey(n)) && !n.querySelector(":scope > .ev-text.open")) { const m = n.querySelector(":scope > .ev-more"); m.hidden = false; m.click(); }
    for (const n of all(".hop")) if (st.open.has("hop:" + identOf(n)) && !n.querySelector(".brief.open")) { const m = n.querySelector(".body > .more"); m.hidden = false; m.click(); }
    if (st.drawer) { document.body.classList.add("drawer-open"); $("#lead-btn")?.setAttribute("aria-expanded", "true"); }
    if (st.focus) {
      let n = st.focus.id ? document.getElementById(st.focus.id) : null;
      if (!n && st.focus.host) { const host = [...document.querySelectorAll(HOSTS)].find((x) => identOf(x) === st.focus.host); n = host && st.focus.sel ? host.querySelectorAll(st.focus.sel)[st.focus.i] : host; }
      if (!n && st.focus.label) n = [...document.querySelectorAll("#page [aria-label], #topbar [aria-label]")].find((x) => x.getAttribute("aria-label") === st.focus.label);
      // A session footer's item or button, by its kind: the footer's items change as the session runs and finishes, so its place
      // among the page's controls can name another one (patchSession's footer rule: else the time item, never the button).
      if (!n && st.focus.foot) { const f = $("#page > .session-foot"); n = f?.querySelector('[data-foot="' + st.focus.foot + '"]') ?? f?.querySelector('[data-foot="time"]') ?? null; }
      // By place only when it had no keyed block and the page has as many controls as before: never onto another row.
      if (!n && !st.focus.host && st.focus.at >= 0 && $("#page").querySelectorAll(FOCUSABLE).length === st.focus.of) n = $("#page").querySelectorAll(FOCUSABLE)[st.focus.at];
      if (n && n !== document.activeElement) { n.focus({ preventScroll: true }); if (st.focus.range) try { n.setSelectionRange(...st.focus.range); } catch {} }
    }
    const pagingTurns = st.paging ? all(".turn") : [];
    if (st.paging) {
      // Newly prepended offscreen turns must have measured heights before placing the visible
      // entry; otherwise their lazy intrinsic estimates change after the anchor has been restored.
      for (const turn of pagingTurns) turn.style.contentVisibility = "visible";
      const heights = pagingTurns.map((turn) => turn.getBoundingClientRect().height);
      pagingTurns.forEach((turn, i) => { turn.style.containIntrinsicBlockSize = "auto " + Math.ceil(heights[i]) + "px"; });
    }
    const place = (first) => {
      const sc = scroller();
      scrollProgrammatically(() => {
        if (pin) sc.scrollTop = sc.scrollHeight;
        else {
          const paging = st.paging;
          let found = null;
          if (paging?.anchor) found = all(".turns [data-e][data-entry-key]:not(.tgroup)").find((n) => n.dataset.entryKey === paging.anchor.key);
          else if (!paging && st.anchor) anchors((n, id) => (id === st.anchor.id ? (found = n) : false));
          if (found) {
            // Rows that moved from below the anchor to above it (a re-sorted list) need more room below than the page may
            // have: the page's bottom padding grows by what is missing, rather than the view sliding. The next redraw or
            // navigation drops it.
            const line = paging ? (phone.matches ? 0 : sc.getBoundingClientRect().top) : edge();
            const d = found.getBoundingClientRect().top - line - (paging ? paging.anchor.off : st.anchor.off), want = sc.scrollTop + d, room = sc.scrollHeight - sc.clientHeight;
            if (want > room + 0.5) { const page = $("#page"); page.style.paddingBottom = parseFloat(getComputedStyle(page).paddingBottom) + Math.ceil(want - room) + "px"; }
            if (d) sc.scrollTop = want;
            LIVE.anchor = paging ? null : { ...st.anchor, route: r0, top: sc.scrollTop };
          } else if (first) sc.scrollTop = st.top + (paging?.before ? sc.scrollHeight - paging.height : 0);
        }
      }, false);
      if (pin) LIVE.anchor = null;
      syncBarLine();
    };
    // And once more two frames later, in case something above changed size after all (as revealTurn does).
    place(true); const placedTop = scroller().scrollTop;
    requestAnimationFrame(() => requestAnimationFrame(() => { for (const turn of pagingTurns) turn.style.contentVisibility = ""; if (rendered === r0 && !viewerEl && scrollRevision === revision && Math.abs(scroller().scrollTop - placedTop) < 1) place(false); })); // yield to a new scroll or jump
  }

  // A session page in place: its turns are drawn again and only those that changed (or are new) replace the ones shown, so
  // the rest keep their nodes and state. The bar's summary line, the title and the sidebar follow. Returns how many entries
  // are new.
  function patchSession(dirty) {
    resetPagerInput(); holdProgrammaticScroll();
    tick(); const s = SESS[route.id], box = $("#page .turns");
    const keys = () => new Set([...$("#page").querySelectorAll(".turns :is(.msg, .bubble, .step, .event, .child-card, .thought, .think-pending)[data-e]")].map((n) => n.dataset.e));
    const before = keys();
    // Only the changed turns are drawn again, unless the turns shown no longer match the index or nothing was shown.
    const whole = !dirty || box.querySelector(":scope > p.empty") || [...box.querySelectorAll(":scope > .turn")].some((b) => !TURN.has(b.dataset.turn));
    if (whole) morph(box, transcript(route.id).querySelector(".turns"));
    else if (dirty.size) morphTurns(box, transcript(route.id, { only: dirty }).querySelector(".turns"), dirty);
    // A page follows its status: the footer is redrawn when its text,
    // tips or state changed (in place, so a focused "Open in" button stays while nothing changes).
    const page = $("#page"), origin = originHandoff(s.id), foot = page.querySelector(":scope > .session-foot");
    if (!showsFooter(s, origin)) foot?.remove();
    else {
      const next = sessionFooter(s, origin);
      if (!foot) page.append(next);
      else if (footerSig(foot) !== footerSig(next)) {
        // Focus stays on the same kind of control (the button, or the calls, time or cost item) of the footer that replaces this
        // one, matched by kind since the items change as the session runs and finishes ([calls, time, cost] while a child works;
        // [time, cost, button] once it has returned). An item the new footer lacks (the calls, once a child returns) hands the
        // focus to the time item, which every footer has, and never to the button, where Enter would leave the page.
        const held = foot.contains(document.activeElement) ? document.activeElement.closest("[data-foot]")?.dataset.foot : null;
        foot.replaceWith(next);
        if (held) (next.querySelector('[data-foot="' + held + '"]') ?? next.querySelector('[data-foot="time"]'))?.focus({ preventScroll: true });
      }
    }
    const h1 = $("#page .ph h1"); if (h1) h1.textContent = s.name;
    const t = $("#topbar .t"); if (t) { t.textContent = s.name; t.dataset.tip = s.name; }
    const lead = $("#topbar .l1-state"); if (lead) lead.replaceWith(stateLead(s));
    const l2 = $("#topbar .meta-line"); if (l2) { l2.replaceChildren(); sessionLine(s)(l2); requestAnimationFrame(() => { if (l2.isConnected) fitMeta(l2); }); }
    // Find's count and the Failed steps chip's count follow the page; a chip that appears or goes redraws the bar.
    const fc = $("#topbar .fcount"); if (fc) fc.textContent = matchText(matchCount());
    if (findOpen && !errOn(s.id)) { const chip = $('#topbar .chip[data-filter="failures"]'), failed = countOf(s, "errors") ?? 0; if (!!chip !== !!failed) keepFocus(drawSessionBar); else if (chip) { const nn = chip.querySelector(".n"); if (nn) nn.textContent = String(failed); } }
    for (const pager of box.querySelectorAll("[data-pager-where]")) paintPager(pager);
    renderNav(); renderLanes(); ticker();
    let n = 0; for (const k of keys()) if (!before.has(k)) n++;
    queuePagerObservers();
    return n;
  }
  // What a block shows, without what the reader toggled (open, hidden, measured clipping) or what opening fills in.
  const VIEW = new Set(["open", "clipped", "flash", "err-current", "err-ring"]);
  function sig(root) {
    let s = [...root.classList].filter((c) => !VIEW.has(c)).join(" ");
    const walk = (x) => { for (const c of x.childNodes) {
      if (c.nodeType === 3) { s += c.data; continue; }
      // A running time (the clock rewrites it every second) is left out: it isn't a change.
      if (c.nodeType !== 1 || c.classList.contains("cutnote") || c.classList.contains("more") || c.classList.contains("ev-more") || c.classList.contains("tick")) continue;
      s += "<" + c.tagName + " " + [...c.classList].filter((k) => !VIEW.has(k)).join(" ") + " " + (c.dataset?.e ?? "") + " " + (c.dataset?.h ?? "") + " " + (c.dataset?.sum ?? "") + " " + (c.getAttribute("d") ?? "") + ">"; walk(c); s += "</>"; } };
    walk(root); return s;
  }
  // A block's signature is taken once: what the reader toggles and the clock's times are outside it.
  const SIGS = new WeakMap();
  const sigOf = (n) => { let x = SIGS.get(n); if (x == null) SIGS.set(n, (x = sig(n))); return x; };
  function morph(box, fresh) {
    const keyOf = (n) => (n.classList.contains("turn") ? "t:" + n.dataset.turn : "x:" + n.className + ":" + n.textContent);
    const count = new Map(), tag = (n) => { const k = keyOf(n), i = count.get(k) ?? 0; count.set(k, i + 1); return k + "#" + i; };
    const old = new Map(); for (const n of box.children) old.set(tag(n), n); count.clear();
    const next = [...fresh.children].map((n) => {
      const o = old.get(tag(n)); if (!o || sigOf(o) === sigOf(n)) return o ?? n;
      for (const f of o.querySelectorAll(".hcard.flash")) {
        const replacement = [...n.querySelectorAll(".hcard")].find((x) => x.dataset.h === f.dataset.h); replacement?.classList.add("flash");
      }
      return n;
    });
    const cur = [...box.children]; let i = 0; while (i < next.length && next[i] === cur[i]) i++;
    for (const n of cur.slice(i)) n.remove();
    box.append(...next.slice(i));
  }
  // The changed turns only, in the index's order: each replaces the one shown if what it shows differs, a new one goes
  // before the next turn shown (else at the end), and one with nothing left to show goes.
  function morphTurns(box, fresh, dirty) {
    const order = (TURNS[route.id] ?? []).map((t) => t.id), q = (root, id) => root.querySelector(':scope > .turn[data-turn="' + CSS.escape(id) + '"]');
    order.forEach((id, i) => {
      if (!dirty.has(id)) return;
      const nb = q(fresh, id), ob = q(box, id);
      if (!nb) { ob?.remove(); return; }
      if (ob) { if (sigOf(ob) !== sigOf(nb)) ob.replaceWith(nb); return; }
      const next = order.slice(i + 1).map((x) => q(box, x)).find(Boolean);
      // With no later turn shown it is the newest: it goes at the end, after any divider there, but before "Load later".
      if (next) next.before(nb); else { const later = box.querySelector(":scope > .list:last-child, :scope > p.empty:last-child"); if (later) later.before(nb); else box.append(nb); }
    });
  }

  // Jump to the latest: centred at the transcript column's foot, sticky, with the count of what arrived while the reader was away.
  const jumpWrap = el("div", "jump-wrap"), jumpButton = el("button", "jump"); jumpButton.type = "button"; jumpButton.id = "jump-bottom"; jumpButton.setAttribute("aria-label", "Jump to bottom of transcript"); jumpWrap.hidden = true; jumpWrap.append(jumpButton);
  function scrollMetrics() {
    if (phone.matches) return { top: window.scrollY, height: document.documentElement.scrollHeight, viewport: window.innerHeight, gap: Math.max(0, document.documentElement.scrollHeight - window.innerHeight - window.scrollY) };
    const m = $("#main"); return { top: m.scrollTop, height: m.scrollHeight, viewport: m.clientHeight, gap: Math.max(0, m.scrollHeight - m.clientHeight - m.scrollTop) };
  }
  function scrollToEnd(behavior = "smooth") { scrollProgrammatically(() => { if (phone.matches) window.scrollTo({ top: document.documentElement.scrollHeight, behavior }); else { const m = $("#main"); m.scrollTo({ top: m.scrollHeight, behavior }); } }); }
  // The button is rebuilt only when what it shows changes (hidden or not, and the new-entry count), not on every scroll.
  let jumpKey = "";
  function syncJump() {
    if (route.v !== "session") { LIVE.fresh = 0; jumpWrap.hidden = true; jumpKey = ""; return; }
    const { gap } = scrollMetrics(), newer = TXM[route.id]?.newer ?? 0; if (gap <= 80) LIVE.fresh = 0;
    const fresh = LIVE.fresh + newer;
    const key = (gap <= 80) + "|" + fresh; if (key === jumpKey && jumpWrap.isConnected) return;
    jumpKey = key; jumpWrap.hidden = gap <= 80 && !newer; jumpButton.replaceChildren();
    if (fresh) jumpButton.append(el("span", "new-count", fresh + " new"));
    jumpButton.append(icon(I.down));
    jumpButton.setAttribute("aria-label", fresh ? "Jump to bottom; " + fresh + " new entries" : "Jump to bottom of transcript");
  }
  function clearNewEntries() { LIVE.fresh = 0; jumpWrap.hidden = true; jumpKey = ""; }
  jumpButton.addEventListener("click", () => {
    const sid = route.id, r = route, m = TXM[sid];
    if (m && m.to < m.total) {
      jumpButton.disabled = true;
      fetchTx(sid, "").then(() => { if (route === r) goSession(sid); }).catch(() => {}).finally(() => { jumpButton.disabled = false; });
    } else scrollToEnd("smooth");
  });
  if (!SIDEBAR_ONLY) { window.addEventListener("scroll", syncJump, { passive: true }); $("#main").addEventListener("scroll", syncJump, { passive: true }); }
  const cancelOpeningEndPin = () => { if (openingEndUntil) stopOpeningEndPin(); };
  if (!SIDEBAR_ONLY) { window.addEventListener("wheel", cancelOpeningEndPin, { passive: true }); window.addEventListener("touchmove", cancelOpeningEndPin, { passive: true }); window.addEventListener("pointerdown", cancelOpeningEndPin, { passive: true }); } // a press anywhere, a scrollbar drag included
  const transcriptInput = (e) => !e.target.closest?.("#sidebar, dialog, input, textarea, select, [contenteditable='true']") && (phone.matches || $("#main").contains(e.target));
  if (!SIDEBAR_ONLY) {
    const input = (e) => { if (transcriptInput(e)) readerScrollInput(); };
    window.addEventListener("wheel", input, { passive: true }); window.addEventListener("touchmove", input, { passive: true });
    const scroll = () => { if (programmaticScrollPending) holdProgrammaticScroll(); else if (!openingEndUntil) readerScrollInput(); };
    window.addEventListener("scroll", () => { if (phone.matches) scroll(); }, { passive: true });
    $("#main").addEventListener("scroll", () => { if (!phone.matches) scroll(); }, { passive: true });
  }
  if (!SIDEBAR_ONLY) document.addEventListener("keydown", (e) => {
    if (!e.defaultPrevented && (transcriptInput(e) || e.target === document.body || e.target === document.documentElement) && ["ArrowUp", "ArrowDown", "PageUp", "PageDown", "Home", "End", " "].includes(e.key)) readerScrollInput();
  });

  // Every second: each running step uses its own start; a group shows its oldest running call.
  // A clock that stands still (the checks pin it) changes nothing.
  const running = (ms) => { const x = Math.max(0, Math.floor(ms / 1000)); return x < 60 ? x + "s" : Math.floor(x / 60) + "m " + (x % 60) + "s"; };
  function ticker() {
    if (!visible() || Date.now() === fetchedAt) return;
    tick();
    const groups = new Map();
    for (const n of document.querySelectorAll(".step.live[data-live], .step.background-running[data-live]")) {
      const a = SESS[n.dataset.live]?.activity, since = n.dataset.since != null ? Number(n.dataset.since) : a?.[3];
      if (since == null || !Number.isFinite(since)) continue;
      const text = running(NOW - since), bg = n.classList.contains("background-running");
      const sd = n.querySelector(bg ? ".sd > .bgo" : ".sd"), label = bg ? "running " + text : text;
      if (sd && sd.textContent !== label) sd.textContent = label;
      const group = n.closest(".tgroup"), earliest = group && groups.get(group);
      if (group && (!earliest || since < earliest.since)) groups.set(group, { since, text });
    }
    for (const [group, live] of groups) {
      const tl = group.querySelector(":scope > .tsum > .tl"), text = "· running " + live.text;
      if (tl && tl.textContent !== text) tl.textContent = text;
    }
    for (const n of document.querySelectorAll(".nrow[data-id] .act .el")) { const a = SESS[n.closest(".nrow").dataset.id]?.activity; if (a) n.textContent = Math.max(0, a[2]) + "s"; }
  }

  // An embedding page's sidebar: the row its data-viewer-nav names (home, sessions or machines) is current.
  if (SIDEBAR_ONLY) { const nav = app.dataset.viewerNav; route = { v: ["home", "sessions", "machines"].includes(nav) ? nav : "" }; }
  boot();
})();
