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
    machine: "M3 5h18v11H3zM8 20h8M12 16v4", repo: "M6 3v12M6 15a3 3 0 1 0 0 6 3 3 0 0 0 0-6zM18 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM18 9c0 6-12 3-12 6", delegate: "M4 3h7v5H4zM7.5 8v9H13M13 14h7v6h-7z",
    out: "M7 17L17 7M9 7h8v8", in: "M17 7L7 17M15 17H7V9", move: "M4 8h13l-3-3M20 16H7l3 3", ask: "M5 18l-1 3 3-1 11-11-2-2zM14 6l4 4", you: "M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM4 21c1-4 4-6 8-6s7 2 8 6", q: "M9 9a3 3 0 1 1 4 2.8c-.7.3-1 .9-1 1.7V14M12 18h.01", check: "M5 12l4 4 10-10", qc: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM9.6 9.4a2.5 2.5 0 1 1 3.4 2.4c-.6.3-1 .8-1 1.5v.4M12 17h.01", decide: "M12 21v-6M12 15L6 9M12 15l6-6M6 9V4M18 9V4M4 6l2-2 2 2M16 6l2-2 2 2", result: "M14 3H6v18h12V7zM14 3v4h4M9 12h6M9 16h6", done: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM8 12.5l2.7 2.7L16 9.8", x: "M6 6l12 12M18 6L6 18", expand: "M15 3h6v6M9 21H3v-6M21 3l-7 7M3 21l7-7", copy: "M9 9h11v11H9zM5 15H4V4h11v1", ext: "M14 4h6v6M20 4l-9 9M18 14v6H4V6h6",
    down: "M12 4v15M5 12l7 7 7-7", up: "M6 15l6-6 6 6", dn: "M6 9l6 6 6-6", branch: "M6 3v12M6 15a3 3 0 1 0 0 6 3 3 0 0 0 0-6zM18 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM18 9c0 6-12 3-12 6",
    wrench: "M14.5 6.5a5 5 0 0 0-6.9 6.9l-4.8 4.8a2 2 0 0 0 2.8 2.8l4.8-4.8a5 5 0 0 0 6.9-6.9l-3 3-2.8-2.8z", wide: "M4 9V4h5M15 4h5v5M20 15v5h-5M9 20H4v-5", sidebar: "M4 5h16v14H4zM9 5v14", tokens: "M5 5h14M12 5v14M9 19h6", chart: "M4 19V5M4 19h17M8 15l3-4 3 2 5-7", coin: "M12 3v18M17 7.5C17 6.1 14.8 5 12 5S7 6.1 7 7.5 9.2 10 12 10s5 1.1 5 2.5-2.2 2.5-5 2.5-5-1.1-5-2.5", relay: "M4 7h13l-3-3M20 17H7l3 3M17 4l-3 3 3 3M7 14l3 3-3 3",
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
  const shortModel = (model) => String(model ?? "Unknown model").replace(/^gpt-\d+-/i, "").replace(/^claude-/i, "").replace(/^(opus|sonnet|haiku)-(\d+)-(\d+)$/i, "$1 $2.$3").replace(/^(opus|sonnet|haiku)-(\d+)\.(\d+)$/i, "$1 $2.$3").replace(/^(opus|sonnet|haiku)-(\d+)$/i, "$1 $2");
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
  const inbox = () => H.filter((h) => h.kind === "toyou" && (h.status === "wait" || (isResult(h) && !SEEN_RESULTS.has(h.id)))).sort((a, b) => b.at - a.at);
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
  function sentence(h, viewer) {
    const W = (id) => { const s = el("span", "who", nameOf(id)); return s; };
    const kindOf = (id) => SESS[id]?.kind === "Subagent" ? "subagent" : SESS[id]?.kind ?? "";
    if (h.kind === "ask") return [I.ask, [W("you"), el("span", "verb", " asked "), W(h.to)]];
    if (h.kind === "spawn") return viewer === h.to ? [I.in, [el("span", "verb", "Brief from "), W(h.from)]] : [I.out, [W(h.from), el("span", "verb", " handed off to " + kindOf(h.to) + " "), W(h.to)]];
    if (h.kind === "relay") return viewer === h.to ? [I.in, [el("span", "verb", "Relay from "), W(h.from)]] : [I.out, [W(h.from), el("span", "verb", " relayed to "), W(h.to)]];
    if (h.kind === "move") {
      const ends = [h.fromMachine, h.toMachine], short = machineShorts(ends.map((id) => [id, MACHINE[id] ?? id]));
      const M = (id) => { const n = el("span", "verb mach", short.get(id)); n.dataset.tip = "Machine: " + (MACHINE[id] ?? id); return n; };
      return [I.move, [el("span", "verb", "Semon moved "), W(h.to), el("span", "verb", " from "), M(h.fromMachine), el("span", "verb", " to "), M(h.toMachine)]];
    }
    const what = { question: " asked you", result: " sent you a result", decision: " needs your decision" }[h.ask];
    const answered = h.status === "done" && (h.ask === "question" || h.ask === "decision");
    return [answered ? I.done : h.ask === "question" ? I.qc : h.ask === "decision" ? I.decide : I.result, [W(h.from), el("span", "verb", what)]];
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
  const textField = (value, min, max) => typeof value === "string" && [...value].length >= min && [...value].length <= max && !/[\u0000-\u001f\u007f-\u009f]/.test(value);
  // A validated copy of an account menu, or null. Each field is read once, inside `try` (an embedding page's object may have
  // getters that throw or answer differently the second time), into plain data, and the copy is what gets checked and kept.
  function accountOf(source) {
    let value;
    try {
      if (!source || typeof source !== "object") return null;
      const list = (xs, max, pick) => {
        if (!Array.isArray(xs)) return null;
        const n = xs.length; if (!(n <= max)) return null;
        const out = []; for (let i = 0; i < n; i++) { const x = xs[i]; out.push(x && typeof x === "object" ? pick(x) : null); }
        return out;
      };
      value = {
        name: source.name, login: source.login, initials: source.initials, avatar_href: source.avatar_href ?? null,
        workspaces: list(source.workspaces, 50, (w) => ({ name: w.name, role: w.role, current: w.current, switch_href: w.switch_href })),
        links: list(source.links, 12, (a) => ({ label: a.label, href: a.href, method: a.method, danger: a.danger })),
      };
    } catch { return null; }
    if (!textField(value.name, 1, 80) || !value.name.trim() || !textField(value.login, 0, 80) || !textField(value.initials, 1, 3) || !value.initials.trim()) return null;
    if (value.avatar_href !== null && !safePath(value.avatar_href)) return null;
    if (!value.workspaces || !value.links) return null;
    if (value.workspaces.some((w) => !w || !textField(w.name, 1, 80) || !w.name.trim() || !textField(w.role, 0, 80) || typeof w.current !== "boolean" || !safePath(w.switch_href))) return null;
    if (value.links.some((a) => !a || !textField(a.label, 1, 80) || !a.label.trim() || !safePath(a.href) || (a.method !== "get" && a.method !== "post") || typeof a.danger !== "boolean")) return null;
    return value;
  }
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
  function adopt(m) {
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
  function fetchTx(sid, q, where, signal) {
    const tok = TOK[sid];
    return api("/api/tx?sid=" + enc(sid) + (q ? "&" + q : ""), signal).then((p) => {
      const es = p.entries.map((e) => txEntry({ ...e, sid })), m = TXM[sid];
      if (where === "before" && m) { TX[sid] = es.concat(TX[sid]); m.from = p.from; }
      else if (where === "after" && m) { TX[sid] = TX[sid].concat(es); m.to = p.to; }
      else { TX[sid] = es; TXM[sid] = { from: p.from, to: p.to }; }
      Object.assign(TXM[sid], { total: p.total, calls: p.calls, errors: p.errors }); if (where !== "before" && p.to >= p.total) TXM[sid].tok = tok; spread(sid);
    });
  }
  // A spawn's child work opens inline under its card: load the turn each brief started, when its session isn't loaded.
  function kids(sid, signal) {
    const jobs = [];
    for (const e of TX[sid] ?? []) {
      const h = e.k === "h" ? HID.get(e.id) : null, c = h && h.kind === "spawn" && h.from === sid ? STARTS.get(h.id) : null;
      if (c && !TX[c.sid]) jobs.push(fetchTx(c.sid, "turn=" + enc(c.id), undefined, signal));
    }
    return jobs.length ? Promise.all(jobs) : null;
  }
  // What a route needs before it can draw: a session's page (the one holding a deep-linked turn), and its child work.
  // `signal` cancels what a navigation asked for when the reader goes elsewhere first.
  function load(r, signal) {
    if (r.v === "analytics") return fetchAnalytics().then(() => { scheduleAnalytics(); }); // the range's answer, from the server
    if (r.v !== "session" || !SESS[r.id]) return null;
    const t = r.turn ? TURN.get(r.turn) : null, deep = t && t.sid === r.id && !t.entries.length;
    if (TX[r.id] && !deep) return lenient(kids(r.id, signal));
    return fetchTx(r.id, deep ? "turn=" + enc(t.id) : "", undefined, signal).then(() => lenient(kids(r.id, signal)));
  }
  // Child work that fails to load leaves its cards as they were: only the session's own transcript failing fails the route.
  const lenient = (p) => (p ? p.catch(() => {}) : p);
  // The last few transcripts opened, kept when the reader leaves them, so opening one again draws it at once. (A transcript
  // still in TX, which only a model update prunes, draws from there just the same.) A transcript is kept only when it was
  // loaded to its end, and the cache is bounded by entries and by estimated memory: two bytes for each character of an entry's
  // text, since JavaScript strings are UTF-16. Opening one takes it out of the cache; leaving it puts it back at the newest end.
  const TXCACHE = new Map(), TXCACHE_MAX = 5, TXCACHE_BYTES = 2 * 1024 * 1024;
  const weigh = (entries) => { let n = 0; for (const e of entries) for (const v of Object.values(e)) n += typeof v === "string" ? v.length : v && typeof v === "object" ? JSON.stringify(v).length : 4; return n * 2; };
  function cacheTx(sid, entries, meta) {
    if (!entries || !meta || meta.to < meta.total || meta.tok == null) return; // without its mark there is no telling later whether it grew
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
    if (t && t.sid === r.id && !t.entries.length) { delete TX[r.id]; delete TXM[r.id]; return false; }
    return true;
  }
  // A mark with fewer entries or bytes than the one loaded means the file was cut or rewritten: load it again.
  function shrank(a, b) { const [s0, b0] = String(a).split(".").map(Number), [s1, b1] = String(b).split(".").map(Number); return s1 < s0 || b1 < b0; }
  // A transcript drawn from the cache is brought up to date the way a live update does it: when the model's mark for it moved
  // since it was kept, its tail is fetched (or the whole page, if the file shrank), and its child work loads. Nothing is asked
  // for when the mark is the same. The page is drawn again, keeping the reader's place, once something arrived.
  function revalidate(r) {
    const sid = r.id, m = TXM[sid], moved = m && m.to >= m.total && m.tok != null && TOK[sid] != null && m.tok !== TOK[sid];
    const job = moved ? (shrank(m.tok, TOK[sid]) ? reload(sid) : tail(sid)) : null, work = job ? job.then(() => kids(sid)) : kids(sid);
    if (work) work.then(() => { if (route === r && rendered === r) refresh(null); }, () => {});
  }
  // "Load earlier" at the top of a transcript, and "Load later" at its end when a deep link loaded a middle page.
  function pager(sid, where, label) {
    const w = el("div", "list"), b = el("button", "more", label); b.type = "button"; if (where === "before") b.dataset.loadEarlier = ""; w.append(b); // data-load-earlier: a stable hook for the budget check
    b.addEventListener("click", () => {
      stopOpeningEndPin();
      const m = TXM[sid], box = phone.matches ? document.documentElement : $("#main"), h0 = box.scrollHeight; b.disabled = true;
      fetchTx(sid, where === "before" ? "before=" + m.from : "after=" + m.to, where).then(() => kids(sid)).then(() => {
        render(); if (where === "before") { const d = box.scrollHeight - h0; if (phone.matches) window.scrollBy(0, d); else box.scrollTop += d; } }, () => { b.disabled = false; });
    });
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
    api("/api/model").then((m) => {
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
      const p = load(route); if (p) p.then(done, done); else done();
    }, (err) => { $(SIDEBAR_ONLY ? "#lanes" : "#page").replaceChildren(el("p", SIDEBAR_ONLY ? "ghead" : "empty", "Couldn't load the sessions: " + err.message)); });
  }

  // ---- State & navigation ---------------------------------------------------------------
  const phone = window.matchMedia("(max-width: 760px)");
  let wideMode = false, railMode = false, treePrefs = {};
  try { wideMode = localStorage.getItem("semon.wide") === "1"; } catch {}
  try { railMode = !SIDEBAR_ONLY && localStorage.getItem("semon.rail") === "1"; } catch {} // the rail is the viewer's own layout: an embedding page keeps its sidebar whole
  try { const saved = JSON.parse(localStorage.getItem("semon.tree") ?? "{}"); if (saved && typeof saved === "object" && !Array.isArray(saved)) treePrefs = pruneTreePrefs(saved); } catch {}
  const app = $(".app");
  const syncLayoutPrefs = () => { if (SIDEBAR_ONLY) return; app.classList.toggle("rail", railMode && !phone.matches); $("#page").classList.toggle("wide-mode", wideMode && !phone.matches); };
  function setWideMode(on) { wideMode = on; try { localStorage.setItem("semon.wide", on ? "1" : "0"); } catch {} syncLayoutPrefs(); $(".wide-toggle")?.setAttribute("aria-pressed", String(on)); $(".account-popover [data-pref=\"wide\"]")?.setAttribute("aria-checked", String(on)); }
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
  let route = { v: "home" }; let groupBy = "recent"; let query = ""; let focusSessionsSearchOnRender = null; let analyticsRange = 7, analyticsMeasure = "hours";
  const sessionFilters = { repo: "", machine: "", harness: "", model: "" };
  let pendingSessionOpen = null, pendingFlashHandoff = null;
  let accountOpen = false;
  // The phone's account menu adds a history entry, so the back gesture closes it.
  let accountSheet = false;
  // What to do once the account menu's history entry has been stepped back over (leaving the page from one of its items).
  let afterPop = null;
  if (!SIDEBAR_ONLY) try { history.scrollRestoration = "manual"; } catch {}
  let show = { messages: true, tools: true, thinking: true }; let find = ""; let findOpen = false; let filterOpen = false;
  const currentScroll = () => phone.matches ? window.scrollY : $("#main").scrollTop;
  const restoreScroll = (top) => { if (phone.matches) window.scrollTo(0, top); else $("#main").scrollTop = top; };
  const saveHistoryScroll = () => { try { if (history.state?.v) history.replaceState({ ...history.state, scrollTop: currentScroll() }, ""); } catch {} };
  let scrollSaveFrame = false;
  const queueScrollSave = () => { if (scrollSaveFrame) return; scrollSaveFrame = true; requestAnimationFrame(() => { scrollSaveFrame = false; saveHistoryScroll(); }); };
  if (!SIDEBAR_ONLY) { window.addEventListener("scroll", queueScrollSave, { passive: true }); $("#main").addEventListener("scroll", queueScrollSave, { passive: true }); }
  const quietTop = () => { if (phone.matches) window.scrollTo(0, 0); else $("#main").scrollTop = 0; };
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
    stopOpeningEndPin(); navAbort?.abort(); navAbort = null;
    if (r.v === "timeline") { r = { ...r, v: "analytics" }; try { history.replaceState({ ...r, scrollTop: r.scrollTop ?? currentScroll() }, "", urlOf(r)); } catch {} }
    if (r.v === "machines" && NAV_MACHINES) { location.assign(NAV_MACHINES); return; }
    if (!fromHistory) saveHistoryScroll();
    closeAccountMenu(true, true);
    dropErrors(true); // (first: it drops a range the error stepper moved, and that is not kept)
    // The session left is kept for opening it again; weighing it waits until the frame the click drew has been painted.
    if (route.v === "session" && (r.v !== "session" || r.id !== route.id) && TX[route.id] && TXM[route.id]) { const sid = route.id, entries = TX[sid], meta = { ...TXM[sid] }; requestAnimationFrame(() => setTimeout(() => cacheTx(sid, entries, meta), 0)); }
    route = r; find = ""; findOpen = false; filterOpen = false; closeDrawer(true); $(".session-menu")?.remove(); clearNewEntries();
    if (!fromHistory) { const state = { ...r }; delete state.scrollTop; try { history.pushState(state, "", urlOf(r)); } catch {} }
    const done = () => {
      if (route !== r) return;
      endLoading(); render(); if (r.v === "session") focusTitle();
      if (fromHistory && Number.isFinite(r.scrollTop)) restoreScroll(r.scrollTop);
      else if (r.v === "session" && r.turn) { revealTurn(r.turn, !fromHistory); if (location.hash) requestAnimationFrame(() => requestAnimationFrame(revealEntryHash)); }
      else if (r.v === "session" && location.hash) revealEntryHash();
      else if (r.v === "session") openSessionAtEnd();
      else quietTop();
      syncJump();
      if (pendingFlashHandoff && r.v === "session" && HID.get(pendingFlashHandoff)?.from === r.id) {
        const id = pendingFlashHandoff; pendingFlashHandoff = null;
        requestAnimationFrame(() => { const card = [...document.querySelectorAll(".hcard")].find((x) => x.dataset.h === id); if (!card) return; card.scrollIntoView({ behavior: matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth", block: "center" }); card.classList.add("flash"); setTimeout(() => card.classList.remove("flash"), 1500); });
      }
    };
    // A transcript already in memory, still in TX or kept in the cache, needs no network: the top bar and the sidebar are drawn
    // from the model at once, the page itself on the next frame (so the first two are on screen before a long transcript is
    // built), and it is brought up to date afterwards. Otherwise the route waits for its data, with the top bar and the
    // sidebar already drawn.
    if (r.v === "session" && SESS[r.id]) {
      const kept = !TX[r.id];
      if (kept ? adoptCached(r) : !isDeep(r)) {
        TXCACHE.delete(r.id);
        paintPending(r);
        requestAnimationFrame(() => setTimeout(() => { if (route !== r) return; if (kept && !r.turn) spread(r.id); done(); revalidate(r); }, 0));
        return;
      }
    }
    const signal = r.v === "session" ? (navAbort = new AbortController()).signal : undefined, p = load(r, signal);
    if (p) { if (r.v === "session") paintPending(r); p.then(done, (err) => failLoad(r, err)); } else done();
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
    const b = [...document.querySelectorAll(".turn")].find((x) => x.dataset.turn === id); if (!b) { quietTop(); return; }
    const place = () => { if (!b.isConnected) return; const gap = $("#topbar").offsetHeight + 8;
      if (phone.matches) window.scrollTo(0, Math.max(0, window.scrollY + b.getBoundingClientRect().top - gap)); else { const m = $("#main"); m.scrollTop += b.getBoundingClientRect().top - m.getBoundingClientRect().top - gap; }
      syncJump(); saveHistoryScroll(); };
    place(); requestAnimationFrame(() => requestAnimationFrame(place));
    if (flash) { b.classList.add("flash"); setTimeout(() => b.classList.remove("flash"), 1500); }
  }
  function revealEntryHash() {
    if (!location.hash) return;
    let key = ""; try { key = decodeURIComponent(location.hash.slice(1)); } catch { key = location.hash.slice(1); }
    const target = document.getElementById(key) ?? [...document.querySelectorAll("[data-e]")].find((n) => n.dataset.e === key) ?? [...document.querySelectorAll(".turn[data-turn]")].find((n) => n.dataset.turn === key);
    if (!target) return;
    const place = () => { if (!target.isConnected) return; const gap = $("#topbar").offsetHeight + 8;
      if (phone.matches) window.scrollTo(0, Math.max(0, window.scrollY + target.getBoundingClientRect().top - gap)); else { const m = $("#main"); m.scrollTop += target.getBoundingClientRect().top - m.getBoundingClientRect().top - gap; }
      syncJump(); saveHistoryScroll(); };
    place(); requestAnimationFrame(() => requestAnimationFrame(place));
  }

  // ---- Sidebar ----------------------------------------------------------------------------------
  function accountAvatar(account) {
    const avatar = el("span", "account-avatar", account.initials);
    if (safePath(account.avatar_href)) {
      const image = el("img"); image.alt = ""; image.setAttribute("src", account.avatar_href);
      image.addEventListener("error", () => image.remove()); avatar.append(image);
    }
    return avatar;
  }
  function accountPopover(compact) {
    const menu = el("div", "menu account-popover"); menu.setAttribute("role", "menu"); menu.setAttribute("aria-label", "Account");
    const identity = el("div", "account-identity"); identity.append(accountAvatar(ACCOUNT));
    const details = el("span", "account-identity-text"); details.append(el("span", "account-name", ACCOUNT.name), el("span", "account-login-value", ACCOUNT.login)); identity.append(details); menu.append(identity);
    const workspaces = el("section", "account-section"); workspaces.append(el("div", "account-heading", "Workspaces"));
    for (const workspace of ACCOUNT.workspaces) {
      if (!safePath(workspace.switch_href)) continue;
      const form = el("form", "account-menu-form account-workspace-form"); form.setAttribute("method", "post"); form.setAttribute("action", workspace.switch_href);
      const row = el("button", "account-menu-row"); row.type = "submit"; row.setAttribute("role", "menuitem");
      if (workspace.current) row.setAttribute("aria-current", "page");
      const name = el("span", "account-row-main"); name.append(el("span", "account-workspace-name", workspace.name), el("span", "account-role", workspace.role)); row.append(name);
      if (workspace.current) row.append(el("span", "account-check", "✓"));
      form.append(row); workspaces.append(form);
    }
    menu.append(workspaces);
    if (!compact) {
      const display = el("section", "account-section account-display"); display.append(el("div", "account-heading", "Display"));
      const row = el("button", "account-menu-row account-switch-row"); row.type = "button"; row.setAttribute("role", "menuitemcheckbox"); row.setAttribute("aria-checked", String(wideMode)); row.dataset.pref = "wide";
      row.append(el("span", "account-row-main", "Wide reading mode"));
      const toggle = el("span", "switch"); toggle.setAttribute("aria-hidden", "true"); toggle.append(el("span", "switch-knob")); row.append(toggle);
      row.addEventListener("click", (event) => { event.stopPropagation(); setWideMode(!wideMode); });
      display.append(row); menu.append(display);
    }
    if (ACCOUNT.links.length) {
      // A destructive link (Sign out) gets a section of its own, set apart from the rest.
      const links = el("section", "account-section account-links"), apart = el("section", "account-section account-links account-danger");
      for (const link of ACCOUNT.links) {
        if (!safePath(link.href)) continue;
        const into = link.danger ? apart : links;
        const row = el(link.method === "post" ? "button" : "a", "account-menu-row" + (link.danger ? " danger" : ""), link.label);
        row.setAttribute("role", "menuitem");
        if (link.method === "post") {
          const form = el("form", "account-menu-form account-link-form"); form.setAttribute("method", "post"); form.setAttribute("action", link.href);
          row.type = "submit"; form.append(row); into.append(form);
        } else {
          row.setAttribute("href", link.href); into.append(row);
        }
      }
      for (const section of [links, apart]) if (section.childElementCount) menu.append(section);
    }
    return menu;
  }
  // Every close takes the phone menu's history entry with it, so no Back press is spent on a menu that is gone. Only a
  // navigation (`go`) and the back gesture itself (`keepEntry`) leave it: stepping back then would undo the navigation, or the
  // entry is already gone. Focus that was in the menu (or fell to the page when it closed) returns to the menu's button, and an
  // update that waited for the menu to close is drawn, unless a navigation (`navigating`) is about to draw the page anyway.
  function closeAccountMenu(keepEntry, navigating) {
    const menu = $(".account-popover"), trigger = $('.account-trigger[aria-expanded="true"]'), active = document.activeElement;
    const refocus = !!menu && (menu.contains(active) || !active || active === document.body);
    document.querySelectorAll(".account-popover, .account-backdrop").forEach((node) => node.remove());
    document.querySelectorAll(".account-trigger").forEach((button) => button.setAttribute("aria-expanded", "false"));
    accountOpen = false;
    if (accountSheet) { accountSheet = false; if (!keepEntry && history.state?.sheet) { skipPop = true; history.back(); } }
    if (refocus && trigger?.isConnected) trigger.focus({ focusVisible: false, preventScroll: true });
    if (LIVE.pending && !navigating) setTimeout(() => { if (LIVE.pending && !viewerEl && !accountOpen) refresh(); }, 0);
  }
  // Leaving the page from a phone menu item steps back over the menu's entry first, so Back from the next page lands on this
  // one, not on a menu that is no longer there.
  function leaveAccountSheet(go) {
    accountSheet = false;
    if (history.state?.sheet) { skipPop = true; afterPop = go; history.back(); } else go();
  }
  // A page brought back from the back-forward cache comes back as it was left, menu and all: close it (its entry is gone).
  window.addEventListener("pageshow", (e) => { if (e.persisted && accountOpen) { accountSheet = false; closeAccountMenu(true); } });
  function toggleAccountMenu(widget, trigger, compact) {
    if (accountOpen) { closeAccountMenu(); return; }
    closeAccountMenu(); closeFilter(); $(".session-menu")?.remove(); $("#more-btn")?.setAttribute("aria-expanded", "false");
    const menu = accountPopover(compact);
    if (compact) {
      // On a phone it floats just above its row, as wide as the row, over a clear backdrop that takes the tap outside it, so it
      // never pushes the drawer and never runs past the screen (the stylesheet caps its height and it scrolls inside).
      const at = trigger.getBoundingClientRect();
      widget.style.setProperty("--account-left", at.left + "px");
      widget.style.setProperty("--account-width", at.width + "px");
      widget.style.setProperty("--account-bottom", Math.max(0, innerHeight - at.top + 6) + "px");
      const backdrop = el("div", "account-backdrop"); backdrop.addEventListener("click", (e) => { e.stopPropagation(); closeAccountMenu(); });
      widget.insertBefore(backdrop, trigger); trigger.after(menu);
      menu.addEventListener("click", (e) => { const a = e.target.closest?.("a[href]"); if (!a || !accountSheet) return; e.preventDefault(); leaveAccountSheet(() => location.assign(a.href)); });
      menu.addEventListener("submit", (e) => { if (!accountSheet) return; e.preventDefault(); const form = e.target; leaveAccountSheet(() => form.submit()); });
      try { history.pushState({ ...route, sheet: 1, scrollTop: currentScroll() }, ""); accountSheet = true; } catch {}
    } else widget.append(menu);
    accountOpen = true; trigger.setAttribute("aria-expanded", "true");
    menu.querySelector(".account-menu-row")?.focus({ focusVisible: false });
  }
  function accountWidget(compact) {
    if (!ACCOUNT) return null;
    const widget = el("div", "account-widget " + (compact ? "account-widget-phone" : "account-widget-desktop"));
    const trigger = el("button", "account-trigger"); trigger.type = "button";
    trigger.setAttribute("aria-haspopup", "menu"); trigger.setAttribute("aria-expanded", "false");
    const current = ACCOUNT.workspaces.find((workspace) => workspace.current);
    if (compact) {
      trigger.setAttribute("aria-label", ACCOUNT.name + ", " + (current?.name ?? ACCOUNT.login));
      const summary = el("span", "account-summary"); summary.append(el("span", "account-summary-name", ACCOUNT.name), el("span", "account-summary-workspace", current?.name ?? ACCOUNT.login));
      trigger.append(accountAvatar(ACCOUNT), summary);
    } else {
      trigger.classList.add("account-avatar-button"); trigger.setAttribute("aria-label", ACCOUNT.name + " account menu"); trigger.append(accountAvatar(ACCOUNT));
    }
    trigger.addEventListener("click", (event) => { event.stopPropagation(); toggleAccountMenu(widget, trigger, compact); });
    widget.append(trigger); return widget;
  }
  function renderDrawerAccount() {
    $("#account-drawer")?.remove();
    if (!ACCOUNT) return;
    const widget = accountWidget(true); widget.id = "account-drawer"; $("#sidebar").append(widget);
  }
  // ---- Stable order: a list of sessions keeps its rows where they are while the model updates ------------------------------------
  // Each list sorted by recency keeps its order as a snapshot of session ids. A live update refreshes every row in place and never
  // reorders one, wherever the list is scrolled: a reorder is held and counted. A session new since the snapshot is inserted at the top
  // (a pure insertion, like a message arriving) only while the list's top is in view and it is untouched (no pointer down, no mouse
  // over one of its rows, no keyboard focus on one); otherwise it is held and counted too, and so is a list new since the snapshot.
  // What is held shows as "N updated": a small chip in the list's own header row while its top is in view (it takes no room and covers
  // no control), a floating pill once the top is scrolled out of view. Tapping either re-sorts the list (the pill also scrolls to its
  // top). The list re-sorts, unasked, only when the reader changes what it is (the route, the search, the filters, the grouping) or
  // opens the phone's drawer. A session that leaves the list or the filter goes in place. Nothing re-sorts on a timer.
  const ORD = new Map(), ordPills = new Map(), ordChips = new Map(), ordTotal = { page: 0, side: 0 }, ordTouch = { down: false }, byLast = (a, b) => b.last - a.last;
  const pageSig = () => JSON.stringify([query, groupBy, sessionFilters]);
  document.addEventListener("pointerdown", () => { ordTouch.down = true; }, true);
  for (const t of ["pointerup", "pointercancel"]) document.addEventListener(t, () => { ordTouch.down = false; }, true);
  window.addEventListener("blur", () => { ordTouch.down = false; });
  document.addEventListener("visibilitychange", () => { ordTouch.down = false; });
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
    const host = side ? null : chipHost("page"); // the header row the chip sits in: the list's top is in view while that row's bottom is below the bar
    const inView = side ? sideRegion().scrollTop <= 1 : !host || host.getBoundingClientRect().bottom >= $("#topbar").getBoundingClientRect().bottom - 1;
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
  // The pill: in the sidebar a sticky zero-height slot above the rows, on a page a fixed button under the top bar, so that showing it
  // moves no row. It shows the count the last draw of that screen found.
  function makeOrderButton(name, cls) {
    const b = el("button", cls); b.type = "button"; b.hidden = true; b.dataset.order = name;
    b.addEventListener("click", (e) => orderResort(name, e.detail === 0, cls === "order-chip"));
    (cls === "order-chip" ? ordChips : ordPills).set(name, b); return b;
  }
  const sideRegion = () => $("#side-list") ?? $("#sidebar");
  const ordLive = el("div", "sr-only"); ordLive.id = "order-status"; ordLive.setAttribute("role", "status"); ordLive.setAttribute("aria-live", "polite");
  { const content = el("div", "side-content"), slot = el("div", "order-slot"); slot.append(makeOrderButton("side", "order-pill"));
    // Bound the sticky slot to the full list content, so it stays pinned through the entire scroll range.
    content.append(slot, $("#lanes")); sideRegion().prepend(content);
    const pagePill = makeOrderButton("page", "order-pill"); if (SIDEBAR_ONLY) sideRegion().append(ordLive); else { $("#page").before(pagePill); document.body.append(ordLive); }
    makeOrderButton("side", "order-chip"); makeOrderButton("page", "order-chip"); } // the page's pill sits just before the page, so the tab order reaches it first
  function placeOrderPill() {
    const b = ordPills.get("page"); if (!b || b.hidden) return;
    const r = $("#page").getBoundingClientRect(); if (r.width) b.style.setProperty("--jump-x", r.left + r.width / 2 + "px");
    b.style.setProperty("--order-top", Math.max(0, $("#topbar").getBoundingClientRect().bottom) + 8 + "px");
  }
  // The header row a screen's chip sits in: the sidebar's "Recent" label; Sessions' group-by row; the heading above a page's first list.
  function chipHost(name) {
    if (name === "side") return $(".side-h");
    if (route.v === "sessions") return $("#page .groupby");
    return $("#page .nrow")?.closest(".list")?.previousElementSibling ?? null;
  }
  function labelOrder(b, n, compact) {
    b.replaceChildren(); if (!n) return;
    const count = el("span", "new-count"), word = el("span", "word"); word.textContent = " updated"; count.append(String(n), word);
    b.append(count, icon(I.up)); b.setAttribute("aria-label", n + " updated, show in recency order"); b.classList.toggle("compact", !!compact);
  }
  // What the last draw of that screen held or found stale shows as the chip while the list's top is in view, as the floating pill once
  // it is scrolled out of view.
  function syncOrderPill(name) {
    const pill = ordPills.get(name), chip = ordChips.get(name), sc = ORD.get(name); if (!pill) return;
    const view = ordState(name), n = !sc || (name === "page" && sc.tie !== route) || (name === "side" && railMode && !phone.matches) ? 0 : sc.n, was = ordTotal[name];
    const pn = view.inView ? 0 : n, cn = view.inView ? n : 0;
    if (pill.dataset.n !== String(pn)) { pill.dataset.n = String(pn); pill.hidden = !pn; labelOrder(pill, pn); }
    const host = cn ? chipHost(name) : null;
    if (!host || chip.parentElement !== host) chip.remove();
    if (host) {
      if (chip.parentElement !== host) host.append(chip);
      if (chip.dataset.n !== String(cn)) { chip.dataset.n = String(cn); labelOrder(chip, cn); }
      chip.hidden = false; chip.classList.remove("compact", "tiny");
      // On a narrow row the chip would cover the last control: it shrinks to its count and arrow, then to its count.
      const last = [...host.children].filter((x) => x !== chip && !x.hidden).at(-1), over = () => last && chip.getBoundingClientRect().left < last.getBoundingClientRect().right + 6;
      if (over()) { chip.classList.add("compact"); if (over()) chip.classList.add("tiny"); }
    } else { chip.hidden = true; chip.dataset.n = "0"; }
    ordTotal[name] = n; // one status region serves both screens: it is cleared only when neither holds anything
    if (n && !was) ordLive.textContent = n + (n === 1 ? " session updated" : " sessions updated"); else if (!ordTotal.page && !ordTotal.side) ordLive.textContent = "";
    if (pn) placeOrderPill();
  }
  // Scrolling moves what is held between the chip and the pill.
  function orderScroll() {
    for (const name of ["page", "side"]) if (ORD.get(name)?.n || !ordPills.get(name).hidden || !ordChips.get(name).hidden) syncOrderPill(name); // nothing held or shown: nothing to move
  }
  if (!SIDEBAR_ONLY) { window.addEventListener("resize", placeOrderPill, { passive: true }); window.addEventListener("scroll", orderScroll, { passive: true }); $("#main").addEventListener("scroll", orderScroll, { passive: true }); }
  sideRegion().addEventListener("scroll", orderScroll, { passive: true });
  // Tapping the pill re-sorts the list it belongs to and scrolls it to its top; a keyboard user lands on its first row. The chip
  // re-sorts where the list stands (its top is already in view).
  function orderResort(name, keyboard, quiet) {
    ORD.delete(name);
    if (name === "side") { renderLanes(); if (!quiet) sideRegion().scrollTop = 0; if (keyboard) $("#lanes .srow")?.focus({ preventScroll: true }); }
    else if (quiet) { const st = capture(); render(); restore(st); syncOrderPill("page"); syncOrderPill("side"); if (keyboard) $("#page .nrow")?.focus({ preventScroll: true }); }
    else { render(); quietTop(); if (keyboard) $("#page .nrow")?.focus({ preventScroll: true }); }
  }
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
      const row = $('#lanes .srow[data-id="' + CSS.escape(s.id) + '"]'); row?.scrollIntoView({ block: "nearest" }); row?.focus({ preventScroll: true });
    });
    line.append(fewer);
  }
  // A phone's "All N": every session below the parent in one sheet, waiting first, then running, then finished, newest first in each.
  function openKidsSheet(parent, trigger) {
    const all = descendantsOf(parent.id, sessionChildren()), bucket = (s) => s.state === "wait" ? 0 : s.state === "work" ? 1 : 2;
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
    const row = el("button", "srow"); row.type = "button"; row.dataset.id = s.id; if (rail) row.dataset.tip = s.name; // the collapsed rail shows only a dot; otherwise the name has a tip while it is cut off
    row.setAttribute("aria-label", s.name + ", " + (STATE[s.state] ?? s.state) + ", " + (HARNESS[s.harness] ?? s.harness) + ", " + shortHost(s));
    const parts = allKids.length ? childParts(allKids) : []; if (parts.length) row.setAttribute("aria-label", row.getAttribute("aria-label") + ", " + parts.join(", "));
    if (current === s.id) row.setAttribute("aria-current", "page");
    if (rail && ancestors.has(s.id)) { row.classList.add("on-path"); row.setAttribute("aria-current", "true"); }
    const main = el("span", "srow-main"), ag = el("span", "ag", ago(s.last)), nm = el("span", "nm", s.name); nm.dataset.tip = s.name; nm.dataset.tipClipped = ""; main.append(dot(s.state, !rail), nm, ag); const mark = rail ? null : harnessIcon(s.harness, { size: 14, label: true }); if (mark) nm.after(mark); // the harness's mark stands alone after the name, its own control apart from the dot (the row's aria-label names the harness too); in the rail the row has the tip (the name), and a dot inside it would answer first
    if (rail && allKids.some((x) => x.state === "work" || x.state === "wait")) { const childDot = dot(urgentDescendant(s.id, children) ?? "work", false); childDot.classList.add("child-dot"); childDot.setAttribute("aria-hidden", "true"); main.append(childDot); }
    // A parent's row (open or collapsed) shows a small dot beside its time only when a session under it needs you (amber) or failed (red); the label says which, so the dot is decorative.
    const flag = kids.length && !rail ? (allKids.some((x) => x.state === "wait") ? "wait" : allKids.some((x) => x.state === "err") ? "err" : null) : null;
    if (flag) { const f = el("span", "kid-flag " + flag); f.setAttribute("aria-hidden", "true"); ag.before(f); }
    const meta = el("span", "srow-meta"); meta.append(icon(I.machine), el("span", "host", shortHost(s)), el("span", "repo-short", s.repo ?? "no repo")); meta.querySelector(".host").dataset.tip = "Machine: " + hostOf(s); meta.querySelector(".repo-short").dataset.tip = spaced("Repo: " + (s.repo ?? "none") + " · " + (s.worktree ? "Worktree: " : "Branch: ") + branchOf(s));
    row.append(main, meta); row.addEventListener("click", () => goSession(s.id)); line.append(row); if (lineToggle) line.append(lineToggle); item.append(line);
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
    return kind && id ? { kind, id } : null;
  }
  function restoreLaneFocus(f) {
    if (!f || document.activeElement !== document.body) return;
    const q = (sel) => $("#lanes " + sel), id = CSS.escape(f.id);
    const target = f.kind === "srow" ? q('.srow[data-id="' + id + '"]') : f.kind === "tree-all" ? q('.tree-all[data-id="' + id + '"]') : f.kind === "tree-fewer" ? q('.treeitem[data-id="' + id + '"] > .tree-row .tree-fewer')
      : f.kind === "tree-toggle" ? q('.tree-toggle[data-tree-toggle="' + id + '"]') : q('.treeitem[data-id="' + id + '"]');
    (target ?? q('.srow[data-id="' + id + '"]'))?.focus({ preventScroll: true });
  }
  function renderLanes() {
    const rail = railMode && !phone.matches, prevSide = ORD.get("side");
    if (rail || prevSide?.rail !== rail) ORD.delete("side"); // the rail draws sorted, and so does a change into or out of it
    const sideState = ordState("side"), focus = laneFocus(), everyone = sessionChildren(), { current, ancestors } = routedPath();
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
    const lanes = Object.values(SESS).filter((s) => s.lane && !parentOf(s.id));
    if (expandedAll && (phone.matches || railMode || !SESS[expandedAll])) expandedAll = null;
    // Opening or folding a parent's whole list, or crossing into or out of the rail, changes which lists are drawn: those the reader
    // just brought back are drawn sorted, not held as new.
    sideOrder.exp = JSON.stringify([expandedAll, railMode && !phone.matches]); sideOrder.reseed = sideOrder.keep && sideOrder.prevExp !== sideOrder.exp;
    expandedPath = expandedAll ? ancestorsOf(expandedAll) : new Set(); expandedUnder = expandedAll ? new Set(descendantsOf(expandedAll, children).map((x) => x.id)) : new Set();
    box.replaceChildren();
    for (const s of orderList(sideOrder, "lanes", lanes, byLast, { limit: 8, must: new Set([current, ...ancestors]) }).slice(0, 8)) box.append(buildLaneItem(s, 0, children, railMode && !phone.matches));
    syncOrderPill("side");
    if (!lanes.length) { const empty = el("p", "ghead", "No sessions"); empty.setAttribute("role", "none"); box.append(empty); }
    restoreLaneFocus(focus);
    // A stuck row covers the top of the sidebar: what is scrolled into view (the open session, after a navigation) stays clear of it.
    const stuck = box.querySelector(".tree-row.stuck"), navigated = revealedFor !== route; revealedFor = route;
    ($("#side-list") ?? $("#sidebar")).style.scrollPaddingTop = stuck ? stuck.offsetHeight + 8 + "px" : "";
    if (stuck && navigated) box.querySelector('.srow[aria-current="page"]')?.scrollIntoView({ block: "nearest" });
  }

  // ---- Top bar ---------------------------------------------------------------------------------------
  // Always visible, the same on every page: the menu toggle (phones), the title, and the page's actions. A list page
  // (Home, Sessions, Machines) shows its name. A detail page (a session, a trace, a machine) shows two lines: its name
  // after a crumb up a level, then a one-line summary that ellipsizes. On a session, search takes over the bar and the
  // filter drops down from it; the ⋯ menu holds the session's details.
  function renderTopbar(title, crumb, opts = {}) {
    closeAccountMenu(); // the bar is redrawn from scratch, the desktop menu with it: close it properly, not by detaching it
    const bar = $("#topbar"), s = opts.session; clearBox(bar, route); bar.classList.remove("scrolled");
    // What the bar holds is added through `put`, so the range control on Analytics (a persistent control) stays where it is.
    const put = placer(bar), sink = { append: put };
    bar.classList.toggle("detail", !!opts.line2); bar.classList.toggle("session-bar", !!s); bar.classList.toggle("searching", !!(s && (findOpen || errOn(s.id))));
    if (s && errOn(s.id)) { errorsBar(sink); const account = accountWidget(false); if (!account) appendWideToggle(sink); if (account) put(account); put.done(); return; }
    if (s && findOpen) { searchBar(sink); const account = accountWidget(false); if (!account) appendWideToggle(sink); if (account) put(account); put.done(); return; }
    const m = el("button", "ibtn lead"); m.id = "lead-btn"; m.type = "button"; m.setAttribute("aria-label", "Open navigation"); m.setAttribute("aria-controls", "sidebar"); m.setAttribute("aria-expanded", "false"); m.append(icon(I.menu)); m.addEventListener("click", openDrawer); put(m);
    const t = el("div", "ttl"), l1 = el("div", "l1");
    if (opts.lineage?.length) {
      if (!phone.matches) opts.lineage.forEach((item) => { const c = el("button", "crumb", item.name); c.type = "button"; c.setAttribute("aria-label", "Open " + item.name); c.addEventListener("click", () => goSession(item.id)); l1.append(c, el("span", "sep", "›")); });
    } else if (crumb) { const c = el("button", "crumb", crumb.label); c.type = "button"; c.setAttribute("aria-label", "Back to " + crumb.label); c.addEventListener("click", crumb.go); l1.append(c, el("span", "sep", "›")); }
    const tt = el("span", "t", title); tt.dataset.tip = title; tt.dataset.tipClipped = ""; if (s && !(phone.matches && opts.lineage?.length)) l1.append(stateLead(s)); l1.append(tt); t.append(l1);
    if (opts.line2) {
      const l2 = el("div", "l2" + (s ? " session-meta" : ""));
      opts.line2(l2); t.append(l2);
      if (s) requestAnimationFrame(() => { if (l2.isConnected) fitSessionLine(l2); });
    }
    put(t);
    if (opts.analytics) { put(rangeControl(bar)); const account = accountWidget(false); if (!account) appendWideToggle(sink); if (account) put(account); put.done(); return; }
    if (!s) { const account = accountWidget(false); if (!account) appendWideToggle(sink); if (account) put(account); put.done(); return; }
    const fb = el("button", "ibtn"); fb.type = "button"; fb.setAttribute("aria-label", "Find in transcript"); fb.append(icon(I.search));
    fb.addEventListener("click", () => { findOpen = true; filterOpen = false; render(); $("#find")?.focus(); });
    const pop = el("div", "filters pop"); pop.hidden = !filterOpen;
    for (const [key, label] of [["messages", "Messages"], ["tools", "Tool steps"], ["thinking", "Thinking"]]) { const l = el("label"); const cb = el("input"); cb.type = "checkbox"; cb.checked = show[key]; cb.id = "f-" + key; cb.addEventListener("change", () => { show[key] = cb.checked; render(); }); l.append(cb, label); pop.append(l); }
    const filtered = !(show.messages && show.tools && show.thinking);
    const tb = el("button", "ibtn" + (filtered ? " on" : "")); tb.id = "filter-btn"; tb.type = "button"; tb.setAttribute("aria-label", "Filter transcript"); tb.setAttribute("aria-expanded", String(filterOpen)); tb.append(icon(I.filter));
    tb.addEventListener("click", () => { $(".session-menu")?.remove(); closeAccountMenu(); $("#more-btn")?.setAttribute("aria-expanded", "false"); filterOpen = pop.hidden; pop.hidden = !filterOpen; tb.setAttribute("aria-expanded", String(filterOpen)); });
    const more = el("button", "ibtn" + (phone.matches && filtered ? " on" : "")); more.id = "more-btn"; more.type = "button"; more.setAttribute("aria-label", "Session details and actions"); more.setAttribute("aria-expanded", "false"); more.append(icon(I.more)); more.addEventListener("click", (e) => { e.stopPropagation(); toggleMenu(SESS[s.id] ?? s, more); });
    // On phones search and filter live in the ⋯ menu, and the filter hangs from that button.
    const place = () => { const anchor = phone.matches ? more : tb; pop.style.right = Math.max(0, bar.getBoundingClientRect().right - anchor.getBoundingClientRect().right) + "px"; };
    tb.addEventListener("click", place);
    if (phone.matches) put(more, pop); else put(fb, tb, more, pop);
    const account = accountWidget(false); if (!account) appendWideToggle(sink); if (filterOpen) place();
    if (account) put(account);
    put.done();
  }
  function appendWideToggle(bar) {
    const b = el("button", "ibtn wide-toggle"); b.type = "button"; b.setAttribute("aria-label", "Wide reading mode"); b.setAttribute("aria-pressed", String(wideMode)); b.dataset.tip = "Wide reading mode"; b.append(icon(I.wide));
    b.addEventListener("click", () => setWideMode(!wideMode)); bar.append(b);
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
  // Kind and state are the two items line 2 always keeps. Everything else drops from the right in append order
  // (model, machine, branch, tools, runs, tokens, cost); errors, the kind word, turn count, then state word give way last.
  function fitSessionLine(l2) {
    const keep = new Set(["meta-kind", "meta-state"]), droppable = [...l2.children].filter((n) => (n.classList.contains("meta-item") || n.classList.contains("meta-runs")) && ![...n.classList].some((c) => keep.has(c)));
    droppable.forEach((n) => { n.hidden = false; });
    const errs = l2.querySelector(".errs"), errsSep = l2.querySelector(".errs-sep"); if (errs) errs.hidden = false; if (errsSep) errsSep.hidden = false;
    const kindValue = l2.querySelector(".meta-kind .meta-value"); if (kindValue) kindValue.hidden = false;
    const stateValues = [...l2.querySelectorAll(".meta-state .meta-value")], sep = l2.querySelector(".meta-state .state-sep"); stateValues.forEach((n) => { n.hidden = false; }); if (sep) sep.hidden = false;
    const fits = () => l2.scrollWidth <= l2.clientWidth + 1;
    for (let i = droppable.length - 1; i >= 0 && !fits(); i--) droppable[i].hidden = true;
    if (!fits() && errs) { errs.hidden = true; if (errsSep) errsSep.hidden = true; }
    if (!fits() && kindValue) kindValue.hidden = true;
    if (!fits() && stateValues[1]) { stateValues[1].hidden = true; if (sep) sep.hidden = true; }
    if (!fits() && stateValues[0]) stateValues[0].hidden = true;
  }
  function closeFilter() { if (!filterOpen) return; filterOpen = false; const p = $(".filters.pop"); if (p) p.hidden = true; $("#filter-btn")?.setAttribute("aria-expanded", "false"); }
  // Search takes over the bar: back, the field, and how many entries match. Back (or Escape) restores the bar.
  function searchBar(bar) {
    const back = el("button", "ibtn"); back.type = "button"; back.setAttribute("aria-label", "Close search"); back.append(icon(I.back)); back.addEventListener("click", () => { findOpen = false; find = ""; render(); });
    const fr = el("label", "find"); const fi = el("input"); fi.id = "find"; fi.type = "search"; fi.placeholder = "Find in transcript"; fi.setAttribute("aria-label", "Find in transcript"); fi.value = find; fr.append(fi);
    fi.addEventListener("input", () => { find = fi.value.toLowerCase(); const pos = fi.selectionStart; render(); const a = $("#find"); a?.focus(); a?.setSelectionRange(pos, pos); });
    fi.addEventListener("keydown", (e) => { if (e.key === "Escape") { e.stopPropagation(); back.click(); } });
    const n = find ? $("#page").querySelectorAll(".turns .msg, .turns .step, .turns .hcard").length : 0;
    const c = el("span", "fcount", find ? (n ? n + (n === 1 ? " match" : " matches") : "No matches") : ""); c.setAttribute("aria-live", "polite");
    bar.append(back, fr, c);
  }
  // ---- Errors mode: "N errors" steps through the session's failed steps ---------------------------------------------------------
  // The bar reads "Error k of N" with previous and next, and a close button (Escape). /api/tx?errors=1 says where every failed
  // step is (its slot), so a step on a page not loaded yet is reachable: a page next to the loaded range is added to it, one
  // further away replaces it with the page around the step. Each step is scrolled to the middle and marked, never opened;
  // its tool group opens so it shows. Closing puts back the pages, what was open and the scroll position from before. The
  // mode is its own controller, apart from find, so the two can become one mode later.
  const ERR = { on: false, sid: null, slots: [], listed: false, count: 0, version: null, k: -1, slot: null, saved: null, range: null, tools: true, chain: Promise.resolve(), gen: 0 };
  const ERR_NEAR = 400, ERR_AROUND = 40; // slots: a page (at most 200 entries) or two away is added; 40 entries of context above
  const errLive = el("div", "sr-only"); errLive.setAttribute("role", "status"); errLive.setAttribute("aria-live", "polite"); if (!SIDEBAR_ONLY) document.body.append(errLive);
  const errText = () => ERR.k < 0 ? (ERR.count ? "Finding errors…" : "No errors") : "Error " + (ERR.k + 1) + " of " + ERR.count;
  const errOn = (sid) => ERR.on && ERR.sid === sid;
  function errorsBar(bar) {
    const close = el("button", "ibtn"); close.type = "button"; close.id = "err-close"; close.setAttribute("aria-label", "Close errors"); close.append(icon(I.x)); close.addEventListener("click", () => closeErrors());
    const pill = el("div", "find errnav"), mark = el("span", "errs-dot"); mark.setAttribute("aria-hidden", "true"); pill.append(mark, el("span", "errnav-count", errText()));
    const prev = el("button", "ibtn errnav-btn"); prev.type = "button"; prev.id = "err-prev"; prev.setAttribute("aria-label", "Previous error"); prev.append(icon(I.up)); prev.addEventListener("click", () => stepErrors(-1));
    const next = el("button", "ibtn errnav-btn"); next.type = "button"; next.id = "err-next"; next.setAttribute("aria-label", "Next error"); next.append(icon(I.dn)); next.addEventListener("click", () => stepErrors(1));
    prev.disabled = next.disabled = ERR.listed && !ERR.slots.length;
    const group = el("div", "errnav-bar"); group.setAttribute("role", "group"); group.setAttribute("aria-label", "Failed steps"); group.append(close, pill, prev, next);
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
  function openErrors(sid) {
    if (ERR.on || route.v !== "session" || route.id !== sid || !TXM[sid]) return;
    stopOpeningEndPin(); closeFilter(); $(".session-menu")?.remove();
    Object.assign(ERR, { on: true, sid, slots: [], listed: false, count: countOf(SESS[sid], "errors") ?? 0, version: null, k: -1, slot: null, saved: capture(), range: { tx: TX[sid], m: { ...TXM[sid] } }, tools: show.tools, gen: ERR.gen + 1 });
    if (!show.tools) { show.tools = true; render(); } else drawSessionBar();
    document.getElementById("err-next")?.focus({ preventScroll: true }); errLabel(true);
    const gen = ERR.gen;
    fetchErrors(sid).then(() => { if (ERR.gen === gen && ERR.on && ERR.slots.length) { ERR.k = 0; showError(true); } else errLabel(true); }, () => { if (ERR.gen === gen && ERR.on) { errLive.textContent = "Couldn't list the errors"; const t = $("#topbar .errnav-count"); if (t) t.textContent = "Couldn't list the errors"; } });
  }
  // The list, or nothing new (304) when the model hasn't moved since it was fetched.
  function fetchErrors(sid) {
    return fetch("/api/tx?sid=" + enc(sid) + "&errors=1" + (ERR.version ? "&since=" + enc(ERR.version) : ""), { credentials: "same-origin" })
      .then((r) => r.status === 304 ? null : r.ok ? r.json() : Promise.reject(Object.assign(new Error(r.status + " " + r.statusText), { status: r.status })), (e) => Promise.reject(Object.assign(e, { status: 0 })))
      .then((x) => {
        if (!x || !errOn(sid)) return;
        ERR.listed = true; ERR.slots = Array.isArray(x.slots) ? x.slots.filter(Number.isInteger) : []; ERR.count = Number.isInteger(x.errors) ? Math.max(x.errors, ERR.slots.length) : ERR.slots.length; ERR.version = typeof x.version === "string" ? x.version : null;
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
    return Promise.resolve(near ? extend() : null).then(around).then(() => kids(sid)).then(() => true);
  }
  // The session's own step for a slot: not one in a child run's work drawn inside it.
  function errNode(sid, slot) {
    const e = (TX[sid] ?? []).find((x) => x.k === "tool" && x.slot === slot); if (!e?.key) return null;
    return [...$("#page").querySelectorAll(".turns .step[data-e]")].find((n) => n.dataset.e === e.key && !n.closest(".cw-body")) ?? null;
  }
  // Marks the current step (its group opened so it shows); with `ring`, rings it for a moment and centres it under the bar.
  function markError(ring) {
    for (const n of $("#page").querySelectorAll(".step.err-current")) n.classList.remove("err-current", "err-ring");
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
    const d = (r.top + r.bottom) / 2 - (edge() + bottom) / 2; if (Math.abs(d) >= 1) sc.scrollTop += d;
    syncBarLine(); syncJump(); saveHistoryScroll();
  }
  function showError(announce) {
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
    if (away && range && m && (m.from !== range.m.from || m.to < range.m.to)) { delete TX[sid]; delete TXM[sid]; }
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
      const b = $("#topbar .errs"), shown = b && !b.getClientRects().length ? $("#more-btn") : b; /* on a phone line 2 is not drawn: focus goes to ⋯ */ if (b && shown && !b.hidden && document.activeElement !== shown && (!document.activeElement || document.activeElement === document.body || !document.activeElement.isConnected)) shown.focus({ preventScroll: true });
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
  function childKindChip(s, meta = false) {
    const c = el("span", meta ? "meta-item meta-kind" : "child-kind"); c.style.setProperty("--h", "var(--" + s.harness + ")");
    // The top bar's badge (meta) keeps the delegation glyph for a Subagent. The card's badge names the harness by its mark and the kind by its word, so it has no glyph for one.
    const mark = s.kind === "Subagent" ? (meta ? icon(I.delegate, "kind-delegate") : null) : s.kind === "Relayed" ? icon(I.relay) : null; // a run of another harness is named by its kind text
    if (!meta) { const source = harnessIcon(s.harness, { size: 14 }); if (source) c.append(source); } // the top bar's badge (meta) is left to the session bar's own change
    if (mark) c.append(mark);
    const kindText = s.kind ?? (s.harness === "codex" ? "Codex run" : "Subagent"); if (meta) c.dataset.tip = "Kind: " + kindText;
    c.append(el("span", meta ? "meta-value" : null, kindText)); return c;
  }
  // A session's compact metadata line: state, model, machine, branch, tools, runs, tokens and API-equivalent cost.
  // Every item is a static badge: a tooltip (data-tip) for the pointer and hidden text ("Tool calls: ") for a screen reader,
  // so a badge is not a tab stop: its label is read out, and the tooltip only adds the full value for a pointer or a tap.
  // Only the errors jump and the runs item are controls. The session's details live in the ⋯ menu.
  const metaSr = (label) => el("span", "sr-only", label + ": ");
  // "Started 21:57 on <machine>" stays on one line: the machine name ellipsises (its tip, only while cut off, has the whole name).
  const startedDivider = (sid) => { const d = el("div", "divider started"), name = MACHINE[SESS[sid].movedFrom ?? SESS[sid].machine], line = el("span", "dv-text"), m = el("span", "dv-machine", name); m.dataset.tip = name; m.dataset.tipClipped = ""; line.append(el("span", "dv-lead", "Started " + clock(SESS[sid].start) + " on\u00a0"), m); d.append(line); return d; };
  const turnsLabel = (s) => { const n = (TURNS[s.id] ?? []).filter(hasTurn).length; return n + (n === 1 ? " turn" : " turns"); };
  // On phones line 2 leaves the bar; top-level sessions keep the state dot before the title, while children put full status in ⋯.
  // The dot names the state for a screen reader, and its tip (a tap on a phone) adds the turn count. Desktop hides it, since line 2 shows the state there.
  const stateLead = (s) => { const lead = el("span", "l1-state"); lead.dataset.tip = spaced("Status: " + STATE[s.state] + " · " + turnsLabel(s)); lead.append(dot(s.state, false)); return lead; };
  const sessionLine = (s) => (l2) => {
    const calls = countOf(s, "calls"), errors = countOf(s, "errors") ?? 0, turnsText = turnsLabel(s);
    const st = el("span", "meta-item meta-state"); st.dataset.tip = spaced("Status: " + STATE[s.state] + " · " + turnsText); st.append(dot(s.state, false), el("span", "meta-value", STATE[s.state]), el("span", "state-sep", "·"), el("span", "meta-value", turnsText));
    if (errors) {
      const sep = el("span", "state-sep errs-sep", "·"), j = el("button", "errs", errors + (errors === 1 ? " error" : " errors")), mark = el("span", "errs-dot"); mark.setAttribute("aria-hidden", "true"); j.prepend(mark);
      const stepTip = j.textContent + ": step through the failed steps"; j.type = "button"; j.dataset.tip = stepTip; j.setAttribute("aria-label", stepTip);
      j.addEventListener("click", (ev) => { ev.stopPropagation(); openErrors(s.id); });
      st.append(sep, j); }
    const kind = s.kind ? childKindChip(s, true) : null;
    const model = el("span", "meta-item meta-model"); model.append(metaSr("Model"), harnessName(s.harness), el("span", "meta-value", shortModel(s.model)));
    if (s.effort) model.append(el("span", "meta-effort", " · " + s.effort));
    model.dataset.tip = "Model: " + (s.model ?? "Unknown model") + (s.effort ? ". Reasoning effort: " + s.effort : "");
    const machine = el("span", "meta-item meta-machine"); machine.append(metaSr("Machine"), icon(I.machine), el("span", "meta-value", shortHost(s))); machine.dataset.tip = "Machine: " + hostOf(s);
    const branch = el("span", "meta-item meta-branch"); branch.append(metaSr(s.worktree ? "Worktree" : "Branch"), icon(I.branch), el("span", "meta-value", branchOf(s))); branch.dataset.tip = (s.worktree ? "Worktree: " : "Branch: ") + branchOf(s);
    const tools = el("span", "meta-item meta-tools"); tools.append(metaSr("Tool calls"), icon(I.wrench), el("span", "meta-value", calls == null ? "—" : String(calls))); tools.dataset.tip = "Tool calls: " + (calls ?? "—");
    const kids = childSessions(s.id), allKids = descendantsOf(s.id, sessionChildren());
    let runs = null;
    if (kids.length) { const working = allKids.filter((x) => x.state === "work").length; runs = el("button", "meta-item meta-runs"); const runsTip = kids.length + (kids.length === 1 ? " child session" : " child sessions") + (working ? ", work in progress" : "") + ": open runs"; runs.type = "button"; runs.dataset.tip = runsTip; runs.setAttribute("aria-label", runsTip); runs.append(icon(I.stack), el("span", "meta-value", String(kids.length))); runs.addEventListener("click", (e) => { e.stopPropagation(); openRuns(s, runs); }); }
    const totalTokens = usageTotal(s);
    const tokens = el("span", "meta-item meta-tokens"); tokens.append(metaSr("Tokens"), icon(I.tokens), el("span", "meta-value", tok(totalTokens / 1e6))); tokens.dataset.tip = "Tokens: " + totalTokens.toLocaleString();
    const parentCost = kids.length ? costForSessions([s, ...allKids]) : costForSession(s.id), missing = costMissing(parentCost), costItem = el("span", "meta-item meta-cost"); costItem.append(metaSr("API-equivalent cost"), icon(I.coin), el("span", "meta-value", (kids.length ? "incl. runs " : "") + (costText(parentCost) === "—" ? "—" : shortMoney(parentCost.usd)))); costItem.dataset.tip = "API-equivalent cost" + (kids.length ? ", including runs" : "") + ": " + costText(parentCost) + ". " + COST_TIP + (missing.length ? " no price for " + missing.join(", ") : "");
    l2.append(...(kind ? [kind] : []), st, model, machine, branch, tools, ...(runs ? [runs] : []), tokens, costItem);
  };
  const machineLine = (m) => (l2) => { const here = onMachine(m), w = here.filter((s) => s.state === "work").length, up = MACHINE_UP[m];
    const st = el("span", "stat " + (!up ? "err" : w ? "work" : "idle")); st.append(dot(!up ? "err" : w ? "work" : "idle", false), !up ? "Not responding" : w ? "Up" : "Idle"); l2.append(st, el("span", "sep", " · "));
    l2.append(el("span", "rest", up ? w + " working · " + here.length + (here.length === 1 ? " session" : " sessions") : movedOff(m).length ? "Semon moved its sessions to other machines" : [MACHINE_LAST[m] != null ? "Last seen " + clock(MACHINE_LAST[m]) : null, here.length + (here.length === 1 ? " session" : " sessions")].filter(Boolean).join(" · "))); };
  // One observer for the current page title; the previous page's is disconnected so it can't flip the new bar.
  let titleObs = null;
  function observeTitle() { syncBarLine(); }
  // The bar's divider shows only once the page has scrolled.
  function syncBarLine() { const y = phone.matches ? window.scrollY : $("#main").scrollTop; $("#topbar").classList.toggle("scrolled", y > 4); }
  if (!SIDEBAR_ONLY) { window.addEventListener("scroll", syncBarLine, { passive: true }); $("#main").addEventListener("scroll", syncBarLine, { passive: true }); }
  if (!SIDEBAR_ONLY) window.addEventListener("resize", () => { const l2 = $("#topbar .l2.session-meta"); if (l2) fitSessionLine(l2); syncLayoutPrefs(); syncJump(); }, { passive: true });
  function toggleMenu(s, btn) {
    const ex = $(".session-menu"); if (ex) { ex.remove(); btn.setAttribute("aria-expanded", "false"); return; }
    const filterWasOpen = phone.matches && filterOpen;
    closeAccountMenu();
    const m = el("div", "menu session-menu"); m.setAttribute("role", "menu");
    if (phone.matches) {
      const path = lineageOf(s.id);
      if (path.length > 1) {
        // The menu is rebuilt each time it opens, so this status uses the latest state and turn count.
        const status = el("div", "menu-status"); status.setAttribute("role", "presentation"); const statusDot = dot(s.state, false); statusDot.setAttribute("aria-hidden", "true"); status.append(statusDot, el("span", null, STATE[s.state] + " · " + turnsLabel(s)));
        const group = el("div", "menu-path-group"), heading = el("div", "menu-section-heading", "Session path"); heading.id = "menu-path-heading"; group.setAttribute("role", "group"); group.setAttribute("aria-labelledby", heading.id); group.append(heading);
        const ancestors = path.slice(0, -1);
        ancestors.forEach((ancestor, i) => {
          const item = el("button", "menu-path-item"); item.type = "button"; item.setAttribute("role", "menuitem");
          const chevron = i === ancestors.length - 1, slot = el("span", "menu-path-chevron" + (chevron ? "" : " blank"));
          if (chevron) item.setAttribute("aria-label", "Up to " + ancestor.name); else slot.setAttribute("aria-hidden", "true");
          item.append(slot);
          item.append(el("span", "menu-path-name", ancestor.name), harnessName(ancestor.harness));
          item.addEventListener("click", (e) => { e.stopPropagation(); m.remove(); btn.setAttribute("aria-expanded", "false"); goSession(ancestor.id); }); group.append(item);
        });
        const separator = el("div", "menu-separator"); separator.setAttribute("role", "separator");
        m.append(status, group, separator);
      }
      const findItem = el("button"); findItem.type = "button"; findItem.setAttribute("role", "menuitem"); findItem.append(icon(I.search, "icon"), el("span", null, "Find in transcript"));
      findItem.addEventListener("click", (e) => { e.stopPropagation(); m.remove(); findOpen = true; filterOpen = false; render(); $("#find")?.focus(); });
      const filtered = !(show.messages && show.tools && show.thinking), filterItem = el("button"); filterItem.type = "button"; filterItem.setAttribute("role", "menuitem"); filterItem.append(icon(I.filter, "icon"), el("span", null, "Filter transcript"));
      if (filtered) filterItem.append(el("span", "menu-note", "On"));
      filterItem.addEventListener("click", (e) => { e.stopPropagation(); m.remove(); btn.setAttribute("aria-expanded", "false"); const pop = $(".filters.pop"); if (!pop) return; filterOpen = filterWasOpen ? false : pop.hidden; pop.hidden = !filterOpen; if (filterOpen) { pop.style.right = Math.max(0, $("#topbar").getBoundingClientRect().right - btn.getBoundingClientRect().right) + "px"; pop.querySelector("input")?.focus(); } });
      m.append(findItem, filterItem);
      // Line 2 is off the phone bar, so what it held is reached here: the errors, the runs, and (in Session details) the rest.
      const runs = $("#topbar .meta-runs"), kids = childSessions(s.id), errors = countOf(s, "errors") ?? 0;
      if (errors) { const label = errors + (errors === 1 ? " error" : " errors"), item = el("button", "menu-errors"); item.type = "button"; item.setAttribute("role", "menuitem"); const mark = el("span", "dot err"); mark.setAttribute("aria-hidden", "true"); item.append(mark, el("span", null, label), el("span", "menu-note", "Step through")); item.addEventListener("click", (e) => { e.stopPropagation(); m.remove(); btn.setAttribute("aria-expanded", "false"); openErrors(s.id); }); m.append(item); }
      if (kids.length) { const item = el("button", "menu-runs"); item.type = "button"; item.setAttribute("role", "menuitem"); item.append(icon(I.stack, "icon"), el("span", null, "Runs · " + kids.length)); item.addEventListener("click", (e) => { e.stopPropagation(); m.remove(); btn.setAttribute("aria-expanded", "false"); openRuns(s, runs); }); m.append(item); }
    }
    const details = el("button"); details.type = "button"; details.setAttribute("role", "menuitem"); details.append(icon(I.read, "icon"), el("span", null, "Session details")); details.addEventListener("click", (e) => { e.stopPropagation(); openSessionDetails(SESS[s.id] ?? s); }); m.append(details);
    const copy = el("button"); copy.type = "button"; copy.append(icon(I.copy, "icon"), el("span", null, "Copy resume command"));
    const cmd = s.harness === "codex" ? "codex resume " + s.id : "claude --resume " + s.id;
    copy.addEventListener("click", () => { navigator.clipboard?.writeText(cmd).then(() => { copy.lastChild.textContent = "Copied"; }, () => { copy.lastChild.textContent = cmd; }); });
    m.append(copy);
    if (s.harness === "claude") { const a = el("button"); a.type = "button"; a.append(icon(I.ext, "icon"), el("span", null, "Open in claude.ai")); m.append(a); }
    const dl = el("dl");
    for (const [k, v] of [["Model", s.model], ...(s.effort ? [["Effort", s.effort]] : []), ["Machine", MACHINE[s.machine] + (s.movedFrom ? " (moved from " + MACHINE[s.movedFrom] + ")" : "")], ["Started", clock(s.start)], ["Duration", dur(s.start, s.state === "work" ? null : s.last)], ["Input + cache write", tok(s.tokens[0])], ["Output", tok(s.tokens[2])], ["Cache read", tok(s.tokens[1])], ["Session id", s.id]]) dl.append(el("dt", null, k), el("dd", "mono", v));
    m.append(dl); closeFilter(); $("#topbar").append(m); btn.setAttribute("aria-expanded", "true");
  }
  // Shown once in the UI, in Session details' footer; NOTICE.md and the README carry it too. The harness marks are the property of their owners.
  const TRADEMARK_NOTICE = "Third-party trademarks are the property of their respective owners. Semon is not affiliated with or endorsed by these companies.";
  function openSessionDetails(s) {
    $(".session-menu")?.remove(); $("#more-btn")?.setAttribute("aria-expanded", "false"); closeAccountMenu(); closeFilter();
    const d = el("dialog", "session-details"); d.setAttribute("aria-labelledby", "session-details-title");
    const head = el("div", "vh"), title = el("div", "vt"), close = el("button", "vclose");
    title.id = "session-details-title"; title.append(el("span", null, "Session details"));
    close.type = "button"; close.setAttribute("aria-label", "Close session details"); close.append(icon(I.x)); close.addEventListener("click", () => d.close()); head.append(title, close);
    const body = el("div", "vb"), list = el("div", "detail-list"), moved = s.movedFrom ? " (moved from " + (MACHINE[s.movedFrom] ?? s.movedFrom) + ")" : "";
    const calls = countOf(s, "calls"), errors = countOf(s, "errors") ?? 0;
    const rows = [
      ...(s.kind ? [["Kind", s.kind]] : []), ["Status", STATE[s.state] + " · " + turnsLabel(s)],
      ["Harness", withHarnessIcon(el("span", null, HARNESS[s.harness] ?? s.harness), s.harness, { size: 16 })], ["Model", s.model ?? s.modelId ?? "Unknown model"],
      ...(s.effort ? [["Effort", s.effort]] : []),
      ["Machine", (MACHINE[s.machine] ?? s.machine ?? "Unknown machine") + (hostOf(s) !== (MACHINE[s.machine] ?? s.machine) ? " · " + hostOf(s) : "") + moved],
    ];
    const directory = s.cwd ?? s.dir ?? s.directory;
    if (directory != null && directory !== "") rows.push(["Directory", directory]);
    rows.push([s.worktree ? "Worktree" : "Branch", branchOf(s)], ["Tool calls", calls == null ? "—" : String(calls)]);
    if (errors) rows.push(["Errors", String(errors)]);
    if (s.pid != null && s.pid !== "") rows.push(["Process id", String(s.pid)]);
    rows.push(["Session id", s.sessionId ?? s.id], ["Started", clock(s.start)], ["Duration", dur(s.start, s.state === "work" ? null : s.last)], ["Last activity", clock(s.last)], ["Input + cache write", tok(s.tokens?.[0] ?? 0)], ["Output", tok(s.tokens?.[2] ?? 0)], ["Cache read", tok(s.tokens?.[1] ?? 0)]);
    for (const [label, value] of rows) { const row = el("div", "detail-row"); const cell = el("span", "detail-value"); if (value instanceof Node) cell.append(value); else cell.textContent = String(value); row.append(el("span", "detail-label", label), cell); list.append(row); }
    const hasRuns = childSessions(s.id).length > 0;
    if (hasRuns) list.append(el("div", "tokens-own", "This session only; the table below includes its runs."));
    const ownCost = costForSession(s.id), allCost = costForSession(s.id, true);
    const costRow = el("div", "detail-row cost-row");
    costRow.append(el("span", "detail-label", "API-equivalent cost"), el("span", "detail-value", hasRuns ? costText(ownCost) + " own · " + costText(allCost) + " incl. runs" : costText(ownCost)));
    list.append(costRow, costBreakdown(s.id, true));
    const missing = costMissing(allCost); if (missing.length) list.append(el("div", "no-price", "no price for " + missing.join(", ")));
    const reports = s.reported_runs ?? [], reported = reports.filter((run) => Number.isFinite(run.cost_usd));
    if (reports.length) {
      const reportedUsd = reported.reduce((sum, run) => sum + run.cost_usd, 0), phrase = reported.length === 1 ? "its last run" : "its last " + reported.length + " runs";
      list.append(el("div", "reported-cost", reported.length ? "Claude Code reported " + asMoney(reportedUsd) + " for " + phrase : "Claude Code reported a run without a cost figure."));
    }
    const mismatch = [...(s.cost_check ?? [])].reverse().find((check) => check.ok === false && Number.isFinite(check.computed_usd) && Number.isFinite(check.reported_usd));
    if (mismatch) { const diff = Math.abs(mismatch.computed_usd - mismatch.reported_usd), pct = mismatch.reported_usd === 0 ? (diff === 0 ? 0 : 100) : Math.round(diff / Math.abs(mismatch.reported_usd) * 100); list.append(el("div", "cost-warning", "Differs from Claude Code's figure by " + pct + "%")); }
    body.append(list, el("p", "third-party", TRADEMARK_NOTICE)); d.append(head, body); document.body.append(d);
    d.addEventListener("click", (ev) => { if (ev.target === d) d.close(); });
    d.addEventListener("close", () => { d.remove(); document.documentElement.classList.remove("viewer-open"); if (viewerEl === d) { viewerEl = null; if (history.state?.sheet) { skipPop = true; history.back(); } } if (LIVE.pending) refresh(); $("#more-btn")?.focus(); });
    viewerEl = d; document.documentElement.classList.add("viewer-open"); d.showModal(); close.focus();
    try { history.pushState({ ...route, sheet: 1 }, ""); } catch {}
  }
  // 739,682 reads "740k" and 12,422,228 "12.4M"; the exact figure is the cell's tooltip.
  const compactCount = (n) => { if (n < 1e3) return String(n); if (n < 1e4) return +(n / 1e3).toFixed(1) + "k"; const k = Math.round(n / 1e3); return k < 1e3 ? k + "k" : +(n / 1e6).toFixed(1) + "M"; };
  // The tokens and cost of a session, always shown: one table per model, a row for each kind of token that was used or billed.
  function costBreakdown(sid, includeRuns) {
    const cost = costForSession(sid, includeRuns), box = el("div", "cost-breakdown"), head = el("div", "cost-breakdown-head");
    head.append(el("span", null, includeRuns && childSessions(sid).length ? "Tokens and API-equivalent cost · incl. runs" : "Tokens and API-equivalent cost"), costInfoTip()); box.append(head);
    let shown = 0;
    for (const [modelId, model] of Object.entries(cost.by_model)) {
      const priced = model.usd != null && !costMissing(cost).includes(modelId), rows = [];
      for (const [key, label] of TOKEN_KINDS) {
        const tokens = Number(model.tokens?.[key]) || 0, amount = Number(model.usd_by_kind?.[key]) || 0;
        if (tokens === 0 && (!priced || amount < 0.005)) continue;
        const row = el("div", "cost-line"), count = el("span", "cost-amount", tokens ? compactCount(tokens) : "");
        if (tokens) { count.dataset.tip = tokens.toLocaleString() + " tokens"; count.append(el("span", "sr-only", " (" + tokens.toLocaleString() + ")")); }
        row.append(el("span", "cost-kind", label), count, el("span", "cost-value", priced ? asMoney(amount) : "—")); rows.push(row);
      }
      if (!rows.length && priced) continue;
      const group = el("section", "cost-model"), top = el("div", "cost-model-head");
      top.append(el("span", "cost-model-name", modelId), el("span", null, "Tokens"), el("span", null, "Cost")); group.append(top, ...rows);
      if (!priced) group.append(el("div", "no-price", "no price for " + modelId));
      box.append(group); shown++;
    }
    if (!shown) box.append(el("p", "empty", "No token usage recorded.")); return box;
  }
  function runRow(s, depth, sheet) {
    const row = el("button", "runs-row"); row.type = "button"; row.style.paddingLeft = Math.min(depth, 3) * 14 + "px";
    const name = el("span", "run-name"); name.append(dot(s.state), childKindChip(s), el("span", null, s.name));
    const cost = costForSession(s.id); row.append(name, el("span", "run-cost", costText(cost)));
    const origin = originHandoff(s.id), meta = el("span", "run-meta");
    meta.append(el("span", null, STATE[s.state]), el("span", null, dur(s.start, s.state === "work" ? null : s.last)), el("span", null, callsText(countOf(s, "calls")))); row.append(meta);
    if (origin?.brief) row.append(el("span", "run-brief", oneLine(origin.brief)));
    const missing = costMissing(cost); if (missing.length) row.append(el("span", "no-price", "no price for " + missing.join(", ")));
    row.setAttribute("aria-label", [s.name, s.kind, STATE[s.state], dur(s.start, s.state === "work" ? null : s.last), origin?.brief ? oneLine(origin.brief) : "", "API-equivalent cost " + costText(cost)].filter(Boolean).join(" · "));
    row.addEventListener("click", () => { if (sheet) { pendingSessionOpen = s.id; sheet.close(); } else { $(".runs-popover")?.remove(); goSession(s.id); } }); return row;
  }
  function appendRunsTree(parent, box, sheet, seen = new Set([parent.id]), tree = sessionChildren()) {
    const children = [...(tree.get(parent.id) ?? [])].sort((a, b) => b.last - a.last);
    for (const child of children) { if (seen.has(child.id)) continue; seen.add(child.id); box.append(runRow(child, 0, sheet)); const nested = tree.get(child.id) ?? []; if (nested.length) { const group = el("div", "runs-group"); appendRunsTree(child, group, sheet, seen, tree); box.append(group); } }
  }
  function openRuns(s, anchor) {
    $(".runs-popover")?.remove(); if (viewerEl) return; const children = sessionChildren().get(s.id) ?? []; if (!children.length) return;
    if (!phone.matches) { const pop = el("div", "runs-popover"); pop.setAttribute("role", "dialog"); pop.setAttribute("aria-label", "Runs under " + s.name); pop.append(el("h2", null, "Runs · " + children.length + " · API-equivalent cost")); const tree = el("div", "runs-tree"); appendRunsTree(s, tree, false); pop.append(tree); $("#topbar").append(pop);
      const close = (e) => { if (!pop.contains(e.target) && e.target !== anchor) { pop.remove(); document.removeEventListener("click", close); } }; setTimeout(() => document.addEventListener("click", close), 0); return; }
    const d = el("dialog", "viewer runs-sheet"); d.setAttribute("aria-label", "Runs under " + s.name); const head = el("div", "vh"), title = el("div", "vt"), close = el("button", "vclose");
    title.append(el("span", null, "Runs · " + children.length + " · API-equivalent cost")); close.type = "button"; close.setAttribute("aria-label", "Close runs"); close.append(icon(I.x)); close.addEventListener("click", () => d.close()); head.append(title, close);
    const body = el("div", "vb"), tree = el("div", "runs-tree"); appendRunsTree(s, tree, d); body.append(tree); d.append(head, body); document.body.append(d); d.addEventListener("click", (e) => { if (e.target === d) d.close(); });
    d.addEventListener("close", () => { const leaving = !!pendingSessionOpen; d.remove(); document.documentElement.classList.remove("viewer-open"); if (viewerEl === d) { viewerEl = null; if (history.state?.sheet) { skipPop = true; history.back(); } else if (pendingSessionOpen) { const id = pendingSessionOpen; pendingSessionOpen = null; goSession(id); } }
      /* Opened from the ⋯ menu, whose item is gone and whose anchor is not drawn on a phone: focus returns to ⋯, not the page. */
      if (!leaving && (!document.activeElement || document.activeElement === document.body)) $("#more-btn")?.focus({ preventScroll: true }); });
    viewerEl = d; document.documentElement.classList.add("viewer-open"); d.showModal(); close.focus({ focusVisible: false }); try { history.pushState({ ...route, sheet: 1, scrollTop: currentScroll() }, ""); } catch {}
  }
  if (!SIDEBAR_ONLY) document.addEventListener("click", (e) => {
    const account = $(".account-popover"); if (account && !account.parentElement.contains(e.target)) closeAccountMenu();
    const m = $(".session-menu"); if (m && !m.contains(e.target)) { m.remove(); $("#more-btn")?.setAttribute("aria-expanded", "false"); }
    // A checkbox in the filter re-renders the bar, so its (now detached) target still sits inside the old popover.
    if (filterOpen && !e.target.closest?.(".filters, #filter-btn")) closeFilter(); });

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
  function clampBrief(body, text) {
    const br = markdown(text, "brief"); body.append(br);
    const more = el("button", "more", "Show more"); more.type = "button"; more.hidden = true; more.setAttribute("aria-expanded", "false");
    more.addEventListener("click", () => { const o = br.classList.toggle("open"); more.textContent = o ? "Show less" : "Show more"; more.setAttribute("aria-expanded", String(o)); });
    new ResizeObserver(() => { if (br.classList.contains("open") || !br.clientHeight) return; const x = br.scrollHeight > br.clientHeight + 1; more.hidden = !x; br.classList.toggle("clipped", x); }).observe(br);
    body.append(more);
  }
  function traceMeta(body, st, text, sid, turn, note, scope) {
    const meta = el("div", "meta"), s = SESS[sid]; const sw = el("span", "stat " + st); sw.append(st === "work" ? el("span", "spin") : dot(st, false), text); meta.append(sw);
    if (s) { const label = machineLabel(s, scope), chip = el("span", "chip-h " + hcls(sid), [s.kind ?? HARNESS[s.harness], label].filter(Boolean).join(" · ")); if (label) chip.dataset.tip = "Machine: " + hostOf(s); meta.append(withHarnessIcon(chip, s.harness, { size: 14, lead: false })); }
    if (note) meta.append(el("span", "gone", note));
    if (s && !s.stub) { const o = el("button", "open", "Open in " + s.name + " ›"); o.type = "button"; o.addEventListener("click", () => goSession(sid, turn?.id)); meta.append(o); }
    body.append(meta);
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
    page.append(flow);
    // The bar's summary line.
    return [sess.size + (sess.size === 1 ? " session" : " sessions"), n + (n === 1 ? " handoff" : " handoffs"), [...machineLabels(scope).values()].join(", ")].filter(Boolean).join(" · ");
  }

  // ---- Session page --------------------------------------------------------------------------------------------------
  // A child's brief is drawn once, in its intro block, so its transcript leaves out the handoff that started it. Every draw of
  // that transcript takes these options: a live update redraws turns too, and one drawn without them shows the brief again.
  // A page that ends in a footer leaves the last turn's "Still working" line to it; one without a footer (a stub, a session with
  // no repo) keeps that line.
  const transcriptOpts = (sid) => { const origin = originHandoff(sid); return origin ? { excludeH: origin.id } : { footer: !!SESS[sid] && showsFooter(SESS[sid], null) }; };
  function renderSession(page, sid) {
    markSeenResults(H.filter((h) => isResult(h) && h.from === sid));
    const s = SESS[sid], origin = originHandoff(sid), head = el("div", "ph sr"); const h1 = el("h1", null, s.name); head.append(h1); page.append(head); observeTitle(h1);
    if (origin) { page.classList.add("child-page"); page.append(childBriefBlock(origin)); }
    page.append(transcript(sid, transcriptOpts(sid)));
    if (showsFooter(s, origin)) page.append(sessionFooter(s, origin));
  }
  // Whether a session page ends in its status line: a child's when it is running or has returned, any other session's always.
  // renderSession and patchSession share it.
  const showsFooter = (s, origin) => origin ? s.state === "work" || s.state === "done" || s.state === "err" || origin.status === "done" || origin.status === "err" : !s.stub && !s.role && s.state in STATE;

  const thoughtText = (e) => String(e.text ?? "").trim();
  const isPendingThought = (e, entries, i, sid) => !!(e.pending || e.status === "thinking") ||
    Array.isArray(entries) && e.k === "think" && !thoughtText(e) && i === entries.length - 1 && SESS[sid]?.state === "work";
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
  function transcriptEntries(entries, sid) {
    const out = [];
    for (let i = 0; i < entries.length; i++) {
      const e = entries[i]; if (e.k !== "think") { out.push(e); continue; }
      const pending = isPendingThought(e, entries, i, sid);
      const row = { ...e, ...(pending ? { pending: true } : {}), displaySecs: thoughtSeconds(e) };
      out.push(row);
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
  function transcript(sid, opts = {}) {
    const sec = el("section", opts.nested ? "nested" : null); sec.setAttribute("aria-label", opts.nested ? SESS[sid].name + " transcript" : "Transcript"); Object.assign(sec.style, { display: "grid", gap: "10px", gridTemplateColumns: "minmax(0, 1fr)" });
    const entries = transcriptEntries(opts.entries ?? TX[sid] ?? [], sid);
    const turnMode = !opts.nested;
    // On a session page the transcript is a list of turns, each with its own entries; a nested one is a plain list.
    const box = el("div", turnMode ? "turns" : "tx"); let tx = box; const hit = (s) => !find || s.toLowerCase().includes(find);
    const keyed = (n, e) => { if (e.key) n.dataset.e = e.key; return n; };
    const range = turnMode ? TXM[sid] : null;
    if (range?.from > 0) box.append(pager(sid, "before", "Load earlier"));
    else if (!find && turnMode) box.append(startedDivider(sid));
    // Adjacent tool calls collapse into one summary line ("Ran 2 commands, read 1 file · 1 failed"),
    // expandable to the individual steps. A lone call stays a single line; while finding, matches show directly.
    let run = []; const maskedIn = new WeakSet();
    const flush = () => {
      if (!run.length) return;
      const steps = el("div", "steps"); run.forEach((r) => steps.append(r.node));
      if (run.length === 1 || find) { tx.append(steps); run = []; return; }
      const counts = new Map(); for (const r of run) { const [, , p, one, many] = toolInfo(r.k), c = counts.get(p) ?? { n: 0, one, many }; c.n++; counts.set(p, c); }
      let text = [...counts].map(([p, c]) => p + " " + c.n + " " + (c.n === 1 ? c.one : c.many)).join(", ");
      text = text[0].toUpperCase() + text.slice(1);
      const failed = run.filter((r) => r.err).length, live = run.find((r) => r.live);
      const g = el("div", "tgroup"); if (run[0].key) g.dataset.e = "g:" + run[0].key; const b = el("button", "tsum"); b.type = "button"; b.setAttribute("aria-expanded", "false");
      b.append(live ? el("span", "spin") : icon(I.stack), el("span", "tt", text));
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
    const closeTurn = () => { flush(); if (!cur) return; const { t, blk } = cur; cur = null; tx = box;
      if ((find || !show.messages || !show.tools || !show.thinking) && !blk.querySelector(".msg, .step, .hcard, .thought, .think-pending")) { blk.remove(); return; }
      if (opts.excludeH && t.last) return;
      // The page's footer says the session is working, so the last turn doesn't say it too (its trace button stays).
      const shown = turnEnd(t), end = opts.footer && t.last && shown?.st === "work" ? null : shown; if (!end && !t.out.length) return;
      const d = el("div", "turn-end");
      if (end) { const st = el("span", "stat " + end.st); st.append(end.st === "work" ? el("span", "spin") : dot(end.st, false), spaced(end.text)); d.append(st); }
      if (t.out.length) d.append(traceBtn(t)); blk.append(d); };
    const openTurn = (t) => { closeTurn(); const blk = el("section", "turn"); blk.dataset.turn = t.id;
      // Your own message needs no header: the bubble is yours and its time sits under it. A relay or brief says who sent it.
      const h = t.start;
      if (t.u || h?.kind === "ask") blk.setAttribute("aria-label", "Your message" + (h ? " at " + clock(h.at) : ""));
      else if (h && h.id !== opts.excludeH) { const hd = el("h3", "turn-h " + hcls(h.from)); const l = el("span", "lbl"); const b = el("button", "from", nameOf(h.from)); b.type = "button"; b.setAttribute("aria-label", "Open " + nameOf(h.from) + " where it sent this"); b.addEventListener("click", () => openSender(h)); l.append(el("span", "verb", h.kind === "relay" ? "Relay from " : "Brief from "), b); hd.append(icon(I.in)); const sender = SESS[h.from] && harnessIcon(SESS[h.from].harness, { size: 16 }); if (sender) hd.append(sender); hd.append(l, el("span", "tm", clock(h.at))); blk.append(hd); }
      tx = el("div", "tx"); blk.append(tx); box.append(blk); cur = { t, blk }; };
    const toolStep = (e, v, ic, live) => {
      const box = keyed(el("div", "step" + (live ? " live" : e.ok || e.ok === null ? "" : " err")), e);
      if (live) { box.dataset.live = sid; if (e.since != null) box.dataset.since = e.since; }
      const b = el("button"); b.type = "button"; b.setAttribute("aria-expanded", "false");
      const status = live ? e.secs : e.unfinished ? "no result" : e.exit != null ? "exit " + e.exit + " · " + e.secs : e.ok ? e.secs : e.ok === null ? "exit unknown · " + e.secs : "failed · " + e.secs;
      const title = e.title ? String(e.title) : null;
      const command = e.in ?? e.arg;
      const firstNonemptyLine = typeof command === "string" ? command.split(/\r\n|\n|\r/).find((line) => line.trim()) : null;
      const label = title ? el("span", "sa st", title) : el("code", "sa", e.arg);
      if (title) {
        b.setAttribute("aria-label", "Ran: " + title);
        if (firstNonemptyLine != null) label.setAttribute("data-tip", firstNonemptyLine.slice(0, 200));
      }
      b.append(live ? el("span", "spin") : icon(I[ic]));
      if (!title) b.append(el("span", "sv", live && v === "Ran" ? "Running" : v));
      b.append(label, el("span", "sd" + (live ? " tick" : ""), status), icon(I.chev, "chev"));
      const out = el("div", "out"); out.hidden = true;
      // Expanded, a step previews what was asked (the full command or input) and what came back, each cut at about
      // eleven lines. When either is cut, "View all" opens the whole call in a sheet.
      const shell = /^(Bash|shell|exec_command|local_shell)$/.test(e.name), inLabel = shell ? "Command" : "Input";
      const input = shell ? (e.in ?? e.arg) : e.in;
      if (input != null) out.append(el("div", "io", inLabel), el("pre", "in clip", input));
      if (e.cwd && e.cwd !== ".") out.append(el("div", "io", "Working directory · " + e.cwd));
      if (live) out.append(el("div", "noout", "Running · no output yet"));
      else {
        if (e.out != null && e.out !== "") out.append(el("div", "io", "Output"));
        if (e.changes) {
          for (const change of e.changes) {
            out.append(el("div", "io", "Change · " + change.path + (change.move ? " → " + change.move : "")));
            if (change.diff?.length) out.append(diffEl(change.diff, "clip")); else out.append(el("div", "noout", "No diff recorded"));
          }
          if (!e.changes.length) out.append(el("div", "noout", "No changes recorded"));
        } else if (e.diff) out.append(diffEl(e.diff, "clip")); else if (e.out) { out.append(outEl(e, "clip")); if (e.cut) out.append(el("div", "cutnote", cutNoteText(e.cut))); } else out.append(el("div", "noout", e.unfinished ? "No result recorded" : "No output"));
      }
      const actions = el("div", "step-actions");
      if (!live && e.script != null) { const script = el("button", "viewscript", "View script"); script.type = "button"; script.addEventListener("click", () => openScript(e)); actions.append(script); }
      const all = el("button", "viewall"); all.type = "button"; all.hidden = true; all.append(icon(I.expand), el("span", null, "View all"));
      all.addEventListener("click", () => openViewer(e, v, ic, inLabel)); actions.append(all); actions.hidden = e.script == null; out.append(actions);
      b.addEventListener("click", () => { out.hidden = !out.hidden; b.setAttribute("aria-expanded", String(!out.hidden));
        if (!out.hidden) {
          let cut = !!e.more?.length;
          out.querySelectorAll(".clip").forEach((c) => { const x = c.scrollHeight > c.clientHeight + 1; c.classList.toggle("clipped", x); cut ||= x; });
          all.hidden = !cut; actions.hidden = e.script == null && !cut;
          // Text cut when this copy was made, with nothing more to show: say so instead of ending on "…".
          if (!cut && !e.cut && !out.querySelector(".cutnote") && [e.in, e.out].some((t) => /…(\(truncated\))?\s*$/.test(t ?? ""))) out.append(el("div", "cutnote", "Cut short in this copy of the logs"));
        }
      });
      box.append(b, out); return box;
    };
    for (const e of entries) {
      if (owner && !opts.only.has(owner.get(e.key))) continue;
      if (turnMode && isGap(e)) { closeTurn(); if (!find) box.append(el("div", "divider", e.text)); continue; }
      if (turnMode && firsts.has(e.key)) openTurn(firsts.get(e.key));
      if (opts.excludeH && e.k === "h" && e.id === opts.excludeH) continue;
      if (opts.excludeH && e.k === "end" && /^Returned to /.test(e.text ?? "")) continue;
      // Entries that render nothing (hidden kinds) must not split a run of tool calls.
      if (e.k === "think" && (!show.thinking || find)) continue;
      // Masked thinking is one quiet line per turn (per list, when nested), at the first masked thought's place. Later ones draw
      // nothing, and the line never splits a run of steps: drawn before flush(), it lands ahead of a run still being gathered.
      if (e.k === "think" && isMaskedThought(e)) {
        const scope = cur ?? box;
        if (!maskedIn.has(scope)) { maskedIn.add(scope); const m = keyed(el("div", "thought masked"), e); m.append(el("div", "think-label", "Thinking hidden by the harness")); tx.append(m); }
        continue;
      }
      if (e.k === "tool") {
        if (!show.tools || !hit(e.name + " " + (e.title ?? "") + " " + e.arg + " " + (e.in ?? "") + " " + (e.out ?? ""))) continue;
        const [ic, v] = verb(e.name);
        const box = toolStep(e, v, ic, !!e.live);
        if (e.live) run.push({ node: box, v, k: e.name, live: true, secs: e.secs, key: e.key });
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
      else if (e.k === "end") { if (find) continue; tx.append(keyed(el("div", "divider", e.text), e)); }
      else if (e.k === "h") {
        const h = H.find((x) => x.id === e.id); if (!hit(h.brief + " " + (h.result ?? ""))) continue;
        // Your own ask is simply your message.
        if (h.kind === "ask") { if (!show.messages) continue; const m = keyed(el("div", "msg user"), e); userBody(m, e, h.brief); tx.append(m); if (cur?.t.start === h) tx.append(el("div", "msg-tm", clock(h.at))); continue; }
        if (h.kind === "toyou" && h.ask === "result") {
          const marker = el("div", "result-marker " + (SEEN_RESULTS.has(h.id) ? "read" : "new"));
          marker.append(icon(I.result), el("span", "word", statWord(h)), el("span", "tm", clock(h.at)));
          tx.append(keyed(marker, e)); continue;
        }
        // A relay or brief that starts a turn is that turn's message.
        if (cur && cur.t.start === h && e === cur.t.entries[0]) { if (!show.messages) continue; tx.append(keyed(handoffCard(h, sid, true), e)); continue; }
        if (h.kind === "move") { if (find) continue; tx.append(keyed(handoffCard(h, sid), e)); continue; }
        if (!show.tools && h.kind !== "toyou") continue;
        if (opts.nested && h.kind === "spawn" && h.to === sid) continue; // the parent's card already shows this brief
        tx.append(keyed(handoffCard(h, sid), e));
      }
    }
    closeTurn();
    if (range && range.to < range.total) box.append(pager(sid, "after", "Load later"));
    if (!box.querySelector(".msg, .step, .hcard")) box.append(el("p", "empty", find ? "Nothing matches “" + find + "”." : "Nothing to show with these filters."));
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
  function openViewer(e, verb, ic, inLabel) {
    if (e.more?.length && e.slot != null && !e.full) { const open = (f) => openViewer({ ...e, ...f, full: true }, verb, ic, inLabel); fullOf(e).then(open, () => open({ fullFailed: true })); return; }
    const d = el("dialog", "viewer"); d.setAttribute("aria-label", verb + " " + e.arg);
    const head = el("div", "vh"); const t = el("div", "vt"); t.append(icon(I[ic]), el("span", null, verb + " " + e.arg));
    const close = el("button", "vclose"); close.type = "button"; close.setAttribute("aria-label", "Close"); close.append(icon(I.x)); close.addEventListener("click", () => d.close());
    const resultStatus = e.live ? "Running · " + e.secs : e.unfinished ? "no result" : e.ok ? e.secs : e.ok === null ? "exit unknown · " + e.secs : "failed · " + e.secs;
    head.append(t, close, el("div", "vm" + (!e.live && !(e.ok || e.ok === null) ? " err" : ""), e.name + " · " + resultStatus));
    const body = el("div", "vb");
    const section = (label, text) => { const s = el("div", "vs"); s.append(el("span", null, label));
      if (text) { const c = el("button", "vcopy"); c.type = "button"; c.append(icon(I.copy), el("span", null, "Copy"));
        c.addEventListener("click", () => navigator.clipboard?.writeText(text).then(() => { c.lastChild.textContent = "Copied"; }, () => { c.lastChild.textContent = "Copy failed"; })); s.append(c); }
      body.append(s); };
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
      else { section("Output", e.out); if (e.out) { body.append(outEl(e)); cutNote(e.out); } else body.append(el("p", "vnote", e.live ? "Running · no output yet" : e.unfinished ? "No result recorded." : "No output.")); }
    }
    if (e.fullFailed) body.append(el("p", "vnote", "Couldn't load the full text: this is the preview."));
    if (e.fullCut?.length) body.append(el("p", "vnote", "Cut at 8 MB: the rest isn't shown."));
    d.append(head, body); document.body.append(d);
    d.addEventListener("click", (ev) => { if (ev.target === d) d.close(); }); // a tap on the backdrop (wide screens)
    d.addEventListener("close", () => { d.remove(); document.documentElement.classList.remove("viewer-open"); if (viewerEl === d) { viewerEl = null; if (history.state?.sheet) { skipPop = true; history.back(); } } if (LIVE.pending) refresh(); });
    viewerEl = d; document.documentElement.classList.add("viewer-open"); d.showModal(); close.focus();
    try { history.pushState({ ...route, sheet: 1 }, ""); } catch {}
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
    const done = (fields) => openViewer({ ...e, ...fields, full: true }, "View script", "run", "Script");
    api("/api/entry?sid=" + enc(e.sid) + "&slot=" + e.slot + "&as=script")
      .then((result) => done({ scriptText: result.text, scriptTruncated: result.truncated }))
      .catch(() => done({ scriptFailed: true }));
  }

  function openParentAtHandoff(h) { pendingFlashHandoff = h.id; const turn = HOLDS.get(h.id); goSession(h.from, turn?.id); }
  function childBriefBlock(h) {
    const parent = SESS[h.from], block = el("section", "child-intro"); block.setAttribute("aria-label", "Brief from " + parent.name);
    const title = el("div", "intro-title"), open = el("button", null, parent.name); open.type = "button"; open.addEventListener("click", () => openParentAtHandoff(h));
    title.append("Brief from ", open, el("span", "tm", clock(h.at)));
    block.append(title);
    const brief = markdown(h.brief, "brief"), more = el("button", "more", "Show more"); more.type = "button"; more.hidden = true; more.setAttribute("aria-expanded", "false");
    more.addEventListener("click", () => { const expanded = brief.classList.toggle("open"); more.textContent = expanded ? "Show less" : "Show more"; more.setAttribute("aria-expanded", String(expanded)); });
    new ResizeObserver(() => { if (brief.classList.contains("open") || !brief.clientHeight) return; const clipped = brief.scrollHeight > brief.clientHeight + 1; more.hidden = !clipped; brief.classList.toggle("clipped", clipped); }).observe(brief);
    const parentLink = el("button", "intro-open", "Open in " + parent.name); parentLink.type = "button"; parentLink.addEventListener("click", () => openParentAtHandoff(h)); const actions = el("div", "intro-actions"); actions.append(more, parentLink); block.append(brief, actions); return block;
  }
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

  function handoffCard(h, viewer, start) {
    const other = h.kind === "move" ? null : viewer === h.from ? h.to : h.from;
    const child = h.kind === "spawn" && viewer === h.from ? SESS[h.to] : null;
    const answered = h.kind === "toyou" && h.status === "done" && (h.ask === "question" || h.ask === "decision");
    const c = el("div", "hcard " + (child ? "child-card " + hcls(h.to) : h.kind === "toyou" ? "toyou" + (h.status === "wait" ? " waiting" : "") + (answered ? " answered" : "") : h.kind === "move" ? "move" : hcls(other)) + (start ? " start" : "")); c.dataset.h = h.id; c.tabIndex = 0; c.setAttribute("role", "link");
    const [ic, parts] = sentence(h, viewer);
    if (child) { const head = el("div", "child-head"); head.append(childKindChip(child), el("span", "ln", child.name)); c.append(head); }
    else { c.append(icon(ic)); const ln = el("span", "ln"); ln.append(...parts); c.append(ln); }
    const shownState = child?.state ?? h.status, sw = el("span", "stat " + shownState); sw.append(shownState === "work" ? el("span", "spin") : dot(shownState === "done" ? "done" : shownState, false), child ? STATE[shownState] : statWord(h)); c.append(sw);
    if (child) { const meta = el("div", "child-meta"); meta.append(el("span", null, dur(child.start, child.state === "work" ? null : child.last)), el("span", null, callsText(countOf(child, "calls")))); c.append(meta); }
    const br = markdown(h.brief, "brief"); c.append(br);
    // Long messages open in place; the rest of the card still goes to the other session.
    const more = el("button", "more", "Show more"); more.type = "button"; more.hidden = true; more.setAttribute("aria-expanded", "false");
    more.addEventListener("click", (ev) => { ev.stopPropagation(); const open = c.classList.toggle("open"); more.textContent = open ? "Show less" : "Show more"; more.setAttribute("aria-expanded", String(open)); });
    new ResizeObserver(() => { if (c.classList.contains("open") || !br.clientHeight) return; const x = br.scrollHeight > br.clientHeight + 1; more.hidden = !x; br.classList.toggle("clipped", x); }).observe(br);
    c.append(more);
    if (h.result) { const r = el("span", "result"); r.append(el("b", null, (h.status === "err" ? "Failed" : "Returned") + ": ")); inline(r, h.result); c.append(r); }
    const an = answerEl(h, "result"); if (an) c.append(an);
    if (child) {
      const actions = el("div", "child-actions"), openChild = el("button", null, "Open"); openChild.type = "button"; openChild.addEventListener("click", (e) => { e.stopPropagation(); goSession(child.id); }); actions.append(openChild);
      const entries = (STARTS.get(h.id)?.entries ?? TX[child.id] ?? []).filter((e) => !(e.k === "h" && e.id === h.id));
      if (entries.length) {
        const group = el("div", "child-work"); group.dataset.e = "cw:" + h.id; group.addEventListener("click", (e) => e.stopPropagation()); const toggle = el("button", "cw-toggle"); toggle.type = "button"; toggle.setAttribute("aria-expanded", "false"); toggle.setAttribute("aria-label", "Activity of " + child.name); const label = el("span", null, "Activity"); label.append(el("span", "cw-count", " · " + entries.length)); toggle.append(icon(I.chev, "chev"), label);
        const inner = el("div", "cw-body"); inner.hidden = true; let showAll = false;
        const paint = () => { inner.replaceChildren(transcript(child.id, { nested: true, entries: showAll ? entries : entries.slice(-5) })); if (!showAll && entries.length > 5) { const all = el("button", "show-all", "Show all " + entries.length); all.type = "button"; all.addEventListener("click", (e) => { e.stopPropagation(); showAll = true; paint(); }); inner.append(all); } };
        toggle.addEventListener("click", (e) => { e.stopPropagation(); if (inner.hidden && !inner.childElementCount) paint(); inner.hidden = !inner.hidden; toggle.setAttribute("aria-expanded", String(!inner.hidden)); });
        group.append(toggle, inner); actions.append(group);
      }
      c.append(actions);
    }
    // Received: the sender's turn that sent it. Sent on: the turn it started there. To you: this turn's trace. A move: the machine it left.
    const open = () => { if (h.kind === "move") go({ v: "machine", id: h.fromMachine }); else if (h.kind === "toyou") { const t = HOLDS.get(h.id); if (t) goTrace(t.id); } else if (viewer === h.to) openSender(h); else if (SESS[other]) goSession(other, STARTS.get(h.id)?.id); };
    c.addEventListener("click", (ev) => { if (!getSelection().isCollapsed) return; open(); });
    c.addEventListener("keydown", (ev) => { if (ev.target === c && (ev.key === "Enter" || ev.key === " ")) { ev.preventDefault(); open(); } });
    return c;
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
    const focusSearch = focusSessionsSearchOnRender === route; focusSessionsSearchOnRender = null;
    if (SIDEBAR_ONLY) { CHILDREN = null; tick(); rendered = route; renderNav(); renderLanes(); return; } // the embedding page draws its own page and bar
    // The page first, then the bar: the bar's summary (a trace's counts, a search's matches) comes from the page.
    closeAccountMenu(); stopOpeningEndPin(); CHILDREN = null; // a redraw inside the open-at-end window ends the pin
    ordPageState = ordState("page"); tick(); const page = $("#page"), r = route; rendered = r; page.style.paddingBottom = ""; clearBox(page, r); page.classList.remove("child-page");
    if (r.v === "home") { renderHome(page); renderTopbar("Home"); }
    else if (r.v === "analytics") { renderAnalytics(page); renderTopbar("Analytics", null, { analytics: true }); }
    else if (r.v === "sessions") { renderSessions(page, focusSearch); renderTopbar("Sessions"); }
    else if (r.v === "machines") { renderMachines(page); renderTopbar("Machines"); }
    else if (r.v === "machine") { renderMachine(page, r.id); renderTopbar(MACHINE[r.id], { label: "Machines", go: () => go({ v: "machines" }) }, { line2: machineLine(r.id) }); }
    else if (r.v === "trace") { const sum = renderTrace(page, r.turn) ?? ""; renderTopbar("Trace", { label: SESS[r.sid].name, go: () => goSession(r.sid, r.turn) }, { line2: (l2) => l2.append(el("span", "rest", sum)) }); }
    else if (r.v === "session") { const s = SESS[r.id], lineage = lineageOf(r.id).slice(0, -1); renderSession(page, r.id); renderTopbar(s.name, null, { session: s, lineage, line2: sessionLine(s) }); }
    if (r.v === "session" && errOn(r.id)) markError(false);
    document.documentElement.style.setProperty("--barh", $("#topbar").offsetHeight + "px");
    const lanesKept = lanesFor && lanesFor.r === r && lanesFor.version === LIVE.version; lanesFor = null;
    syncLayoutPrefs(); syncBarLine(); renderNav(); if (!lanesKept) renderLanes(); renderDrawerAccount(); syncJump(); syncOrderPill("page"); ordPageState = null;
  }

  // ---- Analytics: the server computes each range (/api/analytics) -----------------------------------------------------------
  // The model holds only its own window (a day), so the page asks the server for the range it shows: 24 h, 7 d or 30 d, with
  // the filters. The answer has every figure, chart column and list the page draws, and the page does no range math. It is
  // asked for when the page opens, when the range or a filter changes, after every model update (the server answers 304
  // while nothing changed), and every 10 s while the page shows, as time moves the range. Answers are kept per range and
  // filters, so switching back draws at once while the page asks again.
  const MIN = 60000, HOUR = 60 * MIN, AN_EVERY = 10000, AN_KEEP = 8;
  const AN = { answers: new Map(), inflight: null, again: false, timer: null, error: null, failedAt: 0 };
  function analyticsQuery() {
    const q = ["range=" + (analyticsRange === 1 ? "24h" : analyticsRange + "d")];
    // "No repo" is an empty repo; an unset filter isn't sent.
    if (sessionFilters.repo) q.push("repo=" + (sessionFilters.repo === "__none__" ? "" : enc(sessionFilters.repo)));
    for (const key of ["machine", "harness", "model"]) if (sessionFilters[key]) q.push(key + "=" + enc(sessionFilters[key]));
    return q.join("&");
  }
  const analyticsData = () => AN.answers.get(analyticsQuery())?.data ?? null;
  // One request at a time: a change while one is out asks again when it is back. Resolves true when the answer changed.
  function fetchAnalytics() {
    if (AN.inflight) { AN.again = true; return AN.inflight; }
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
      AN.again = false; return fetchAnalytics().then((more) => changed || more);
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
    return fetchAnalytics().then((changed) => {
      if (changed && route.v === "analytics" && rendered === route) {
        if (viewerEl || accountOpen) LIVE.pending = true; // drawn when the sheet or the account menu closes
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
  function sessionRow(A, sid, cls, value, onOpen, rank) {
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
          f.chip.hidden = current === ""; if (current !== "") { active++; f.text.textContent = f.label + ": " + f.showValue(current); f.chip.setAttribute("aria-label", "Clear " + f.label + " filter: " + f.showValue(current)); }
        }
        count.hidden = !active; count.textContent = active ? String(active) : ""; btn.setAttribute("aria-label", active ? "Filter, " + active + " active" : "Filter");
      };
      // Opening the sheet is a history entry, like the viewer's other sheets: Back closes it and the page stays. The page behind it
      // doesn't scroll: not by overflow: hidden on the page (that resets a phone's page to the top, as the Select's sheet found), but by
      // refusing the wheel and touch moves that don't start in a list that can move (the Select's own sheet does the same for its list).
      let before = "";
      const hold = (e) => { const t = e.target; if (t.closest?.(".sh-select-list") || (body.contains(t) && body.scrollHeight > body.clientHeight + 1)) return; e.preventDefault(); };
      btn.addEventListener("click", () => {
        if (viewerEl || d.open) return; ctx.sync(); before = JSON.stringify(sessionFilters);
        viewerEl = d; d.showModal(); for (const type of ["wheel", "touchmove"]) document.addEventListener(type, hold, { capture: true, passive: false });
        fields[0].select.focus(); try { history.pushState({ ...route, sheet: 1, scrollTop: currentScroll() }, ""); } catch {}
      });
      clear.addEventListener("click", () => { for (const key of Object.keys(sessionFilters)) sessionFilters[key] = ""; d.close(); });
      done.addEventListener("click", () => d.close());
      d.addEventListener("click", (e) => { if (e.target === d) d.close(); });
      d.addEventListener("close", () => {
        for (const type of ["wheel", "touchmove"]) document.removeEventListener(type, hold, { capture: true });
        if (viewerEl === d) { viewerEl = null; if (history.state?.sheet) { skipPop = true; history.back(); } }
        btn.focus({ preventScroll: true });
        if (JSON.stringify(sessionFilters) !== before) ctx.onChange(); else if (LIVE.pending) refresh();
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
    const legend = el("div", "analytics-legend"); for (const [h, label] of [["claude", "Claude"], ["codex", "Codex"]]) { const item = el("span"), swatch = el("i"); swatch.style.setProperty("--h", "var(--" + h + ")"); item.append(...[swatch, harnessIcon(h, { size: 14 }), label].filter(Boolean)); legend.append(item); } panel.append(legend); return panel;
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
    if (A.cost.unpriced_models.length) panel.append(el("div", "no-price", "no price for " + A.cost.unpriced_models.join(", ") + "; unpriced usage is omitted from bars.")); return panel;
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
      const row = sessionRow(A, item.sid, "analytics-session analytics-slice", costMode ? asMoney(item.usd) : timeText(item.ms) + " busy", () => { pendingSessionOpen = item.sid; d.close(); });
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
    for (const item of items) { const b = sessionRow(A, item.sid, "analytics-session", value(item), () => goSession(item.sid), { measure: measure(item), max }); const missing = item.unpriced_models ?? []; if (missing.length) b.append(el("span", "no-price", "no price for " + missing.join(", "))); list.append(b); } panel.append(list); return panel;
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
    put(renderAgentsChart(A), renderCostChart(A), bdBox, bottom); const allowance = renderCodexAllowance(A.allowance); if (allowance) put(allowance);
    // data-analytics-ready: the figures and charts are drawn (a stable hook for the budget check); data-query: for which range and filters.
    metrics.dataset.analyticsReady = ""; metrics.dataset.query = analyticsQuery();
    put.done();
  }

  // ---- Sessions: every top-level session and its child runs ---------------------------------------------------------------
  const laneOf = (sid) => { const seen = new Set(); while (parentOf(sid) && !seen.has(sid)) { seen.add(sid); sid = parentOf(sid); } return sid; };
  function childRuns(sid) {
    const kids = Object.values(SESS).filter((x) => x.id !== sid && laneOf(x.id) === sid), sub = kids.filter((x) => x.kind === "Subagent").length, cdx = kids.filter((x) => x.kind === "Codex run").length, other = kids.length - sub - cdx;
    return [sub ? sub + (sub === 1 ? " subagent" : " subagents") : null, cdx ? cdx + (cdx === 1 ? " Codex run" : " Codex runs") : null, other ? other + (other === 1 ? " other run" : " other runs") : null].filter(Boolean).join(" · ");
  }
  function renderSessions(page, focusSearch = false) {
    const all = Object.values(SESS).filter(matchesSessionFacets);
    const head = el("div", "ph"); const h1 = el("h1", null, "Sessions"); head.append(h1);
    const sub = el("div", "sub"); for (const [v, l] of [[all.length, all.length === 1 ? "session" : "sessions"], [all.filter((s) => s.state === "work").length, "working"], [all.filter((s) => s.state === "wait").length, "waiting on you"]]) { const x = el("span"); x.append(el("b", null, String(v)), l); sub.append(x); }
    head.append(sub); const put = placer(page); put(head); observeTitle(h1);
    // The search field and the group-by buttons are persistent controls: the page's redraws keep them, and the field's focus, text and caret.
    const found = slot("find", page, (ctx) => {
      const fr = el("label", "find"), fi = el("input"); fi.id = "sq"; fi.type = "search"; fi.placeholder = "Search sessions"; fi.setAttribute("aria-label", "Search sessions"); fi.value = query; fr.append(icon(I.search), fi);
      fi.addEventListener("input", () => { query = fi.value.trim(); ctx.draw(); }); return fr;
    }), fr = found.el, fi = fr.querySelector("input");
    if (fi.value.trim() !== query) fi.value = query;
    const grouped = slot("groupby", page, (ctx) => {
      const gb = el("div", "groupby"); gb.setAttribute("role", "group"); gb.setAttribute("aria-label", "Group by");
      for (const [g, label] of [["recent", "Recent"], ["project", "Project"], ["machine", "Machine"], ["harness", "Harness"]]) { const b = el("button", null, label); b.type = "button"; b.dataset.g = g; b.addEventListener("click", () => { groupBy = g; ctx.draw(); }); gb.append(b); }
      return gb;
    }), gb = grouped.el;
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
        for (const s of items) { const r = el("button", "nrow"); r.type = "button"; r.dataset.id = s.id; r.append(dot(s.state), el("span", "nm", s.name), el("span", "ag", ago(s.last)), withHarnessIcon(el("span", "for", [s.kind ?? HARNESS[s.harness], MACHINE[s.machine], s.repo ? where(s) : "no repo"].join(" · ")), s.harness, { size: 14 })); const k = childRuns(s.id); if (k) r.append(el("span", "kids", k)); r.addEventListener("click", () => goSession(s.id)); list.append(r); }
        box.append(list); out.append(box);
      }
      if (!lanes.length) out.append(el("p", "empty", "No sessions match “" + query + "”."));
      for (const b of gb.querySelectorAll("button[data-g]")) b.setAttribute("aria-pressed", String(b.dataset.g === groupBy));
      syncOrderPill("page"); renderLanes();
    };
    found.ctx.draw = grouped.ctx.draw = draw;
    put(renderFacetFilters(page, () => render()), fr, gb, out); put.done(); draw();
    if (focusSearch) fi.focus({ preventScroll: true });
  }

  // ---- Drawer (phone) ---------------------------------------------------------------------------------------------------------
  const sidebar = $("#sidebar");
  function openDrawer() { if (!phone.matches) return; if (ORD.get("side")?.n) { ORD.delete("side"); renderLanes(); } document.body.classList.add("drawer-open"); $("#lead-btn")?.setAttribute("aria-expanded", "true"); }
  function closeDrawer(quiet) { if (!document.body.classList.contains("drawer-open")) return; document.body.classList.remove("drawer-open"); closeAccountMenu(); const b = $("#lead-btn"); b?.setAttribute("aria-expanded", "false"); if (!quiet) b?.focus(); }
  // On an embedding page shell.js opens and closes the drawer, and names its opening (semon:drawer-open); the viewer binds none of it, "/" included.
  if (!SIDEBAR_ONLY) { $("#drawer-close").addEventListener("click", () => closeDrawer()); $("#scrim").addEventListener("click", () => closeDrawer()); }
  else window.addEventListener("semon:drawer-open", () => { if (ORD.get("side")?.n) { ORD.delete("side"); renderLanes(); } }); // opening the drawer re-sorts what the list held, as openDrawer does
  if (!SIDEBAR_ONLY) document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && accountSheet) closeAccountMenu();
    else if (e.key === "Escape" && !viewerEl) { closeDrawer(); closeAccountMenu(); $(".session-menu")?.remove(); $("#more-btn")?.setAttribute("aria-expanded", "false"); closeFilter(); }
    const target = document.activeElement;
    if (e.key === "/" && !e.ctrlKey && !e.metaKey && !e.altKey && !viewerEl && !accountOpen &&
        !document.querySelector("dialog[open], .menu, .lineage-menu, .runs-popover") &&
        !/^(INPUT|TEXTAREA|SELECT)$/.test(target?.tagName ?? "") && !target?.isContentEditable) {
      e.preventDefault();
      const search = route.v === "sessions" ? $("#sq") : null;
      if (search) {
        focusSessionsSearchOnRender = null; search.focus({ preventScroll: true });
      } else {
        const sessions = { v: "sessions" }; focusSessionsSearchOnRender = sessions;
        go(sessions);
      }
    }
  });
  let sx = null;
  if (!SIDEBAR_ONLY) sidebar.addEventListener("touchstart", (e) => { sx = e.touches[0].clientX; }, { passive: true });
  if (!SIDEBAR_ONLY) sidebar.addEventListener("touchmove", (e) => { if (sx !== null && e.touches[0].clientX - sx < -50) { sx = null; closeDrawer(); } }, { passive: true });
  phone.addEventListener("change", () => {
    closeDrawer(true); syncLayoutPrefs(); expandedAll = null; renderLanes();
    if (!phone.matches && viewerEl?.classList.contains("kids-sheet")) viewerEl.close(); // a sheet is a phone's: a wide screen opens the list in the tree
    if (route.v === "session" || route.v === "analytics") { const top = currentScroll(); render(); restoreScroll(top); }
    else renderLanes();
    const l2 = $("#topbar .l2"); if (l2?.classList.contains("session-meta")) fitSessionLine(l2); syncJump();
  });

  // ---- Live updates (deliberate difference 3) --------------------------------------------------------------------------------
  // Every screen polls /api/model?since= every 2 s while the tab is visible, one request at a time: it backs off up to 30 s
  // on errors and stops on 403 (the server restarted with a new token). A new model swaps the globals and draws the screen
  // again with its view state kept: the scroll position, anchored to the first visible block; what is open, by stable keys;
  // focus, find and filters; the drawer. A session page follows its transcript's tail
  // with /api/tx?after= and replaces only the turns that changed. An open View all sheet holds the redraw until it closes.
  const LIVE = { late: null, lateTries: 0, retry: false, version: null, timer: null, due: 0, busy: false, started: -Infinity, delay: 2000, ended: false, again: false, pending: false, fresh: 0, turns: new Map(), missing: new Set() };
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
    fetch("/api/model?since=" + enc(LIVE.late ? "" : LIVE.version ?? ""), { credentials: "same-origin" })
      .then((r) => r.status === 304 ? null : r.ok ? r.json() : Promise.reject(Object.assign(new Error(r.status + " " + r.statusText), { status: r.status })), (e) => Promise.reject(Object.assign(e, { status: 0 })))
      .then((m) => (m ? update(m) : null))
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
  // The transcripts on screen: a session page's own, and its child runs'. Any other loaded transcript is dropped from TX (the
  // last few opened are kept in TXCACHE, and brought up to date when opened again).
  const viewed = () => { const v = new Set(); if (route.v !== "session") return v; v.add(route.id); for (const h of H) if (h.kind === "spawn" && h.from === route.id && h.to) v.add(h.to); return v; };
  function update(m) {
    const oldH = new Map(H.map((h) => [h.id, handKey(h)])), oldT = LIVE.turns, names = new Map(Object.values(SESS).map((x) => [x.id, x.name]));
    const hadOrigins = new Set([...TXCACHE.keys()].filter((sid) => !!originHandoff(sid)));
    const hadOrigin = route.v === "session" && !!SESS[route.id] && !!originHandoff(route.id);
    adopt(m); remember(m);
    for (const sid of [...TXCACHE.keys()]) if (!hadOrigins.has(sid) && originHandoff(sid)) TXCACHE.delete(sid);
    // A page that had no origin and now has one (its parent's spawn arrived) loads its transcript again: the first prompt it drew as
    // a message is the brief, which the intro now shows. A failed request is retried on the next poll, which backs off, up to
    // LATE_TRIES requests in all; after that the page stays as drawn (the brief shows twice until a reload) and polls as usual.
    if (LIVE.late !== route.id) { if (LIVE.late) TXCACHE.delete(LIVE.late); LIVE.late = null; }
    if (route.v === "session" && !hadOrigin && !!SESS[route.id] && !!originHandoff(route.id)) { LIVE.late = route.id; LIVE.lateTries = 0; }
    if (LIVE.late && !TX[LIVE.late]) LIVE.late = null; // nothing loaded to load again: the page loads it with its origin
    const changedH = new Set(H.filter((h) => oldH.get(h.id) !== handKey(h)).map((h) => h.id));
    const view = viewed(), grown = new Set(), cuts = new Map();
    let full = Object.values(SESS).some((x) => names.has(x.id) && names.get(x.id) !== x.name); // a new name shows in every turn
    for (const sid of Object.keys(TX)) { if (view.has(sid) && SESS[sid]) spread(sid); else { delete TX[sid]; delete TXM[sid]; } }
    // Only transcripts whose mark in the model moved are asked for, one at a time.
    let chain = Promise.resolve();
    for (const sid of view) if (TX[sid] && LIVE.late === sid) chain = chain.then(() => soft(reloadLate(sid).then(() => { grown.add(sid); full = true; })));
    else if (TX[sid] && TXM[sid].tok != null && TOK[sid] != null && shrank(TXM[sid].tok, TOK[sid])) chain = chain.then(() => soft(reload(sid).then(() => { grown.add(sid); full = true; })));
    else if (TX[sid] && TXM[sid].to >= TXM[sid].total && TXM[sid].tok !== TOK[sid]) chain = chain.then(() => soft(tail(sid).then((r) => { grown.add(sid); if (r.cut != null) cuts.set(sid, r.cut); if (r.reload) full = true; })));
    if (route.v === "session" && TX[route.id]) chain = chain.then(() => newKids(route.id, grown));
    return chain.then(() => { LIVE.version = m.version; refresh(full ? null : dirtyTurns(cuts, grown, changedH, oldT)); if (route.v === "analytics") refreshAnalytics(); const e = errorsLive(); return e && soft(e); });
  }
  // The turns of the session page an update changed: those holding entries its tail brought (from the cut on), those
  // whose record or handoffs changed, and those holding the spawn of a child run that grew. Null: draw them all.
  function dirtyTurns(cuts, grown, changedH, oldT) {
    if (route.v !== "session" || !TX[route.id]) return null;
    const sid = route.id, dirty = new Set(), owner = new Map((TURNS[sid] ?? []).flatMap((t) => t.entries.map((e) => [e, t.id])));
    if (cuts.has(sid)) for (const e of TX[sid].slice(cuts.get(sid))) { if (isGap(e)) return null; if (owner.has(e)) dirty.add(owner.get(e)); }
    for (const t of TURNS[sid] ?? []) {
      if (oldT.get(t.id) !== LIVE.turns.get(t.id) || [t.start, ...t.sent].some((h) => h && changedH.has(h.id)) || t.entries.some((e) => e.k === "h" && changedH.has(e.id))) dirty.add(t.id);
    }
    for (const h of H) if (h.kind === "spawn" && h.from === sid && grown.has(h.to) && HOLDS.get(h.id)) dirty.add(HOLDS.get(h.id).id);
    return dirty;
  }
  // The tail of a transcript loaded to its end: from its first call still running (anywhere), or the last turn's first call
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
      TX[sid] = es.slice(0, cut).concat(got); Object.assign(m, { to: last.to, total: last.total, calls: last.calls, errors: last.errors, tok }); spread(sid);
      return { cut };
    });
  }
  // The page's own transcript loads its last page again; a child run's is dropped, and loads again as new child work.
  function reload(sid) {
    if (sid !== route.id) { delete TX[sid]; delete TXM[sid]; return Promise.resolve({ cut: null, reload: true }); }
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
  // A new spawn's child work: its turn, loaded one request at a time. A turn the server doesn't have (404) isn't asked for again.
  function newKids(sid, grown) {
    let chain = Promise.resolve();
    for (const e of TX[sid] ?? []) {
      const h = e.k === "h" ? HID.get(e.id) : null, c = h && h.kind === "spawn" && h.from === sid ? STARTS.get(h.id) : null;
      if (c && !TX[c.sid] && !LIVE.missing.has(c.id)) chain = chain.then(() => (TX[c.sid] ? null : soft(fetchTx(c.sid, "turn=" + enc(c.id)).then(() => { grown.add(c.sid); }, (err) => { if (err?.status === 404) LIVE.missing.add(c.id); throw err; }))));
    }
    return chain;
  }
  // Draws the new model on the screen shown, unless a navigation is still loading (it draws when done), the sheet is open
  // (it draws when the sheet closes), or what the screen shows left the model (it stays as it was).
  function refresh(dirty) {
    if (accountOpen && !$(".account-popover")?.isConnected) closeAccountMenu(); // a menu some redraw took away is closed
    if (viewerEl || accountOpen) { LIVE.pending = true; return; } // drawn whole when the sheet or the account menu closes
    if (SIDEBAR_ONLY) { LIVE.pending = false; render(); return; } // the page is the embedding page's: only the sidebar is redrawn
    LIVE.pending = false; const r = route;
    if (rendered !== r || (r.v === "session" && !SESS[r.id]) || (r.v === "trace" && !SESS[r.sid]) || (r.v === "machine" && !MACHINE[r.id])) return;
    const st = capture(); $("#page").style.paddingBottom = "";
    if (r.v !== "session") { render(); restore(st); syncOrderPill("page"); syncOrderPill("side"); return; }
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
    openingEndUntil = 0;
    clearTimeout(openingEndTimer); openingEndTimer = null;
    openingEndObserver?.disconnect(); openingEndObserver = null;
  }
  function pinOpeningEnd() {
    if (route.v !== "session" || performance.now() >= openingEndUntil) { stopOpeningEndPin(); return; }
    const sc = scroller(); sc.scrollTop = sc.scrollHeight; LIVE.anchor = null; syncJump(); saveHistoryScroll();
  }
  function startOpeningEndPin() {
    stopOpeningEndPin(); if (route.v !== "session" || location.hash) return;
    openingEndUntil = performance.now() + 2000;
    const turns = $("#page section[aria-label='Transcript'] .turns");
    if (turns) { openingEndObserver = new ResizeObserver(pinOpeningEnd); openingEndObserver.observe(turns); }
    pinOpeningEnd(); openingEndTimer = setTimeout(stopOpeningEndPin, 2000);
  }
  const ANCHORS = "[data-e], .turn, .hop, .ib, .nrow, .sec-h, .ph, .divider, .analytics-metric, .analytics-panel, .facet-filters, .groupby, .find, .empty";
  const HOSTS = "[data-e], [data-h], [data-id], [data-sid], [data-go], [data-turn], [data-g], [data-m]";
  const FOCUSABLE = "button, input, [tabindex], a[href]";
  const identOf = (n) => { const d = n.dataset, keys = [d.e, d.turn, d.h, d.id, d.m, d.sid, d.go, d.g];
    return [n.classList[0], ...keys, keys.some((x) => x != null) ? "" : n.firstChild?.nodeType === 3 ? n.firstChild.data : ""].map((x) => x ?? "").join("|"); };
  // Each anchor candidate on the page, in order, with its identity made unique by how many came before it.
  function anchors(fn) { const seen = new Map(); for (const n of $("#page").querySelectorAll(ANCHORS)) { const id = identOf(n), k = seen.get(id) ?? 0; seen.set(id, k + 1); if (fn(n, id + "#" + k)) return; } }
  const opener = (n) => n.classList.contains("step") ? n.querySelector(":scope > button") : n.classList.contains("tgroup") ? n.querySelector(":scope > .tsum") : n.classList.contains("child-work") ? n.querySelector(":scope > .cw-toggle") : null;
  function capture() {
    const sc = scroller(), line = edge();
    const st = { top: sc.scrollTop, bottom: sc.scrollHeight - sc.scrollTop - sc.clientHeight <= 80, anchor: null, open: new Set(), groups: new Set(), focus: null, drawer: document.body.classList.contains("drawer-open") };
    // The first block, innermost, still visible under the bar, and how far its top is from the bar. While the reader hasn't
    // scrolled since the last redraw placed it, that placement's block and offset are kept as they were, so the fraction
    // of a pixel each placement rounds off can't add up over many updates.
    const kept = LIVE.anchor; let still = null;
    if (kept && kept.route === rendered && Math.abs(sc.scrollTop - kept.top) < 1) anchors((n, id) => (id === kept.id ? (still = n) : false));
    // ... and only while that block is still where it was placed (a filter, find or zoom since has moved it).
    if (still && Math.abs(still.getBoundingClientRect().top - line - kept.off) <= 1) st.anchor = { id: kept.id, off: kept.off };
    else anchors((n, id) => { if (n.querySelector(ANCHORS)) return false; const b = n.getBoundingClientRect(); if (!b.height || b.bottom <= line) return false; st.anchor = { id, off: b.top - line }; return true; });
    for (const n of $("#page").querySelectorAll("[data-e]")) {
      if (n.classList.contains("tgroup")) st.groups.add(n.dataset.e);
      if (opener(n)?.getAttribute("aria-expanded") === "true" || (n.classList.contains("hcard") && n.classList.contains("open"))) st.open.add(n.dataset.e);
    }
    for (const n of $("#page").querySelectorAll(".hop")) if (n.querySelector(".brief.open")) st.open.add("hop:" + identOf(n));
    // Child work opened and then closed keeps what was open inside it, for when it opens again.
    st.shut = new Set([...$("#page").querySelectorAll(".child-work[data-e]")].filter((n) => opener(n).getAttribute("aria-expanded") === "false" && n.querySelector(":scope > .cw-body").childElementCount).map((n) => n.dataset.e));
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
    const all = (sel) => [...$("#page").querySelectorAll(sel)], r0 = rendered;
    // A new card or brief measures its "Show more" now, as its ResizeObserver would a frame later, so nothing moves after the
    // scroll position is set.
    const clamp = (br, more) => { if (!br || !more || !br.clientHeight) return; const x = br.scrollHeight > br.clientHeight + 1; more.hidden = !x; br.classList.toggle("clipped", x); };
    for (const c of all(".hcard:not(.open)")) clamp(c.querySelector(":scope > .brief"), c.querySelector(":scope > .more"));
    for (const br of all(".hop .body > .brief:not(.open)")) clamp(br, br.parentElement.querySelector(":scope > .more"));
    // Child work first (opening it draws its steps), then groups (a new one opens if it holds an open step), then steps and cards.
    // Child work that was closed with something open inside is opened for the restore (so its steps measure as shown) and
    // closed again below, in the same task: it is never drawn open.
    const shut = all(".child-work[data-e]").filter((n) => st.shut.has(n.dataset.e) && opener(n).getAttribute("aria-expanded") === "false");
    for (const n of all(".child-work[data-e]")) if ((st.open.has(n.dataset.e) || shut.includes(n)) && opener(n).getAttribute("aria-expanded") === "false") opener(n).click();
    for (const n of all(".tgroup[data-e]")) {
      const want = st.groups.has(n.dataset.e) ? st.open.has(n.dataset.e) : [...n.querySelectorAll(".step[data-e]")].some((x) => st.open.has(x.dataset.e));
      if (want && opener(n).getAttribute("aria-expanded") === "false") opener(n).click();
    }
    for (const n of all(".step[data-e]")) if (st.open.has(n.dataset.e) && opener(n)?.getAttribute("aria-expanded") === "false") opener(n).click();
    for (const n of shut) opener(n).click();
    // A card or brief opened before its size was measured: its "Show less" is shown by hand.
    for (const n of all(".hcard[data-e]")) if (st.open.has(n.dataset.e) && !n.classList.contains("open")) { const m = n.querySelector(":scope > .more"); m.hidden = false; m.click(); }
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
    const place = (first) => {
      const sc = scroller();
      if (pin) sc.scrollTop = sc.scrollHeight;
      else {
        let found = null; if (st.anchor) anchors((n, id) => (id === st.anchor.id ? (found = n) : false));
        if (found) {
          // Rows that moved from below the anchor to above it (a re-sorted list) need more room below than the page may
          // have: the page's bottom padding grows by what is missing, rather than the view sliding. The next redraw or
          // navigation drops it.
          const d = found.getBoundingClientRect().top - edge() - st.anchor.off, want = sc.scrollTop + d, room = sc.scrollHeight - sc.clientHeight;
          if (want > room + 0.5) { const page = $("#page"); page.style.paddingBottom = parseFloat(getComputedStyle(page).paddingBottom) + Math.ceil(want - room) + "px"; }
          if (d) sc.scrollTop = want;
          LIVE.anchor = { ...st.anchor, route: r0, top: sc.scrollTop };
        } else if (first) sc.scrollTop = st.top;
      }
      if (pin) LIVE.anchor = null;
      syncBarLine();
    };
    // And once more two frames later, in case something above changed size after all (as revealTurn does).
    place(true); requestAnimationFrame(() => requestAnimationFrame(() => { if (rendered === r0 && !viewerEl) place(false); })); // not on a page opened since
  }

  // A session page in place: its turns are drawn again and only those that changed (or are new) replace the ones shown, so
  // the rest keep their nodes and state. The bar's summary line, the title and the sidebar follow. Returns how many entries
  // are new.
  function patchSession(dirty) {
    tick(); const s = SESS[route.id], box = $("#page .turns");
    const keys = () => new Set([...$("#page").querySelectorAll(".turns :is(.msg, .step, .hcard, .thought, .think-pending)[data-e]")].filter((n) => !n.closest(".cw-body")).map((n) => n.dataset.e));
    const before = keys();
    // Only the changed turns are drawn again, unless the turns shown no longer match the index or nothing was shown.
    const whole = !dirty || box.querySelector(":scope > p.empty") || [...box.querySelectorAll(":scope > .turn")].some((b) => !TURN.has(b.dataset.turn));
    if (whole) morph(box, transcript(route.id, transcriptOpts(route.id)).querySelector(".turns"));
    else if (dirty.size) morphTurns(box, transcript(route.id, { ...transcriptOpts(route.id), only: dirty }).querySelector(".turns"), dirty);
    // A page follows its origin and status: a child's intro that only now has an origin, and the footer redrawn when its text,
    // tips or state changed (in place, so a focused "Open in" button stays while nothing changes).
    const page = $("#page"), origin = originHandoff(s.id), foot = page.querySelector(":scope > .session-foot");
    if (origin && !page.querySelector(":scope > .child-intro")) { page.classList.add("child-page"); page.querySelector(":scope > .ph")?.after(childBriefBlock(origin)); }
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
    const l2 = $("#topbar .l2"); if (l2) { l2.replaceChildren(); sessionLine(s)(l2); requestAnimationFrame(() => { if (l2.isConnected) fitSessionLine(l2); }); }
    const fc = $("#topbar .fcount"); if (fc) { const n = find ? $("#page").querySelectorAll(".turns .msg, .turns .step, .turns .hcard").length : 0; fc.textContent = find ? (n ? n + (n === 1 ? " match" : " matches") : "No matches") : ""; }
    renderNav(); renderLanes(); ticker();
    let n = 0; for (const k of keys()) if (!before.has(k)) n++;
    return n;
  }
  // What a block shows, without what the reader toggled (open, hidden, measured clipping) or what opening fills in.
  const VIEW = new Set(["open", "clipped", "flash", "err-current", "err-ring"]);
  function sig(root) {
    let s = [...root.classList].filter((c) => !VIEW.has(c)).join(" ");
    const walk = (x) => { for (const c of x.childNodes) {
      if (c.nodeType === 3) { s += c.data; continue; }
      // A running time (the clock rewrites it every second) is left out: it isn't a change.
      if (c.nodeType !== 1 || c.classList.contains("cw-body") || c.classList.contains("cutnote") || c.classList.contains("more") || c.classList.contains("tick")) continue;
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
    const next = [...fresh.children].map((n) => { const o = old.get(tag(n)); return o && sigOf(o) === sigOf(n) ? o : n; });
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

  const jumpButton = el("button", "jump-bottom"); jumpButton.type = "button"; jumpButton.id = "jump-bottom"; jumpButton.setAttribute("aria-label", "Jump to bottom of transcript"); jumpButton.hidden = true; if (!SIDEBAR_ONLY) document.body.append(jumpButton);
  // Centred over the transcript column (#page), not the viewport: the sidebar or rail takes the left, and #main has its own scrollbar.
  const placeJump = () => { const r = $("#page").getBoundingClientRect(); if (r.width) jumpButton.style.setProperty("--jump-x", r.left + r.width / 2 + "px"); };
  if (!SIDEBAR_ONLY) { const ro = new ResizeObserver(placeJump); ro.observe($("#page")); ro.observe($("#main")); }
  function scrollMetrics() {
    if (phone.matches) return { top: window.scrollY, height: document.documentElement.scrollHeight, viewport: window.innerHeight, gap: Math.max(0, document.documentElement.scrollHeight - window.innerHeight - window.scrollY) };
    const m = $("#main"); return { top: m.scrollTop, height: m.scrollHeight, viewport: m.clientHeight, gap: Math.max(0, m.scrollHeight - m.clientHeight - m.scrollTop) };
  }
  function scrollToEnd(behavior = "smooth") { if (phone.matches) window.scrollTo({ top: document.documentElement.scrollHeight, behavior }); else { const m = $("#main"); m.scrollTo({ top: m.scrollHeight, behavior }); } }
  // The button is rebuilt only when what it shows changes (hidden or not, and the new-entry count), not on every scroll.
  let jumpKey = "";
  function syncJump() {
    if (route.v !== "session") { LIVE.fresh = 0; jumpButton.hidden = true; jumpKey = ""; return; }
    const { gap } = scrollMetrics(); if (gap <= 80) LIVE.fresh = 0; else placeJump();
    const key = (gap <= 80) + "|" + LIVE.fresh; if (key === jumpKey) return;
    jumpKey = key; jumpButton.hidden = gap <= 80; jumpButton.replaceChildren();
    if (LIVE.fresh) jumpButton.append(el("span", "new-count", LIVE.fresh + " new"));
    jumpButton.append(icon(I.down));
    jumpButton.setAttribute("aria-label", LIVE.fresh ? "Jump to bottom; " + LIVE.fresh + " new entries" : "Jump to bottom of transcript");
  }
  function clearNewEntries() { LIVE.fresh = 0; jumpButton.hidden = true; jumpKey = ""; }
  jumpButton.addEventListener("click", () => scrollToEnd("smooth"));
  if (!SIDEBAR_ONLY) { window.addEventListener("scroll", syncJump, { passive: true }); $("#main").addEventListener("scroll", syncJump, { passive: true }); }
  const cancelOpeningEndPin = () => { if (openingEndUntil) stopOpeningEndPin(); };
  if (!SIDEBAR_ONLY) { window.addEventListener("wheel", cancelOpeningEndPin, { passive: true }); window.addEventListener("touchmove", cancelOpeningEndPin, { passive: true }); window.addEventListener("pointerdown", cancelOpeningEndPin, { passive: true }); } // a press anywhere, a scrollbar drag included
  if (!SIDEBAR_ONLY) document.addEventListener("keydown", (e) => {
    if (!e.defaultPrevented && !e.target.closest?.("input, textarea, select, [contenteditable='true']") && ["ArrowUp", "ArrowDown", "PageUp", "PageDown", "Home", "End", " "].includes(e.key)) cancelOpeningEndPin();
  });

  // Every second: each running step's elapsed time and a running row's age.
  // A clock that stands still (the checks pin it) changes nothing.
  const running = (ms) => { const x = Math.max(0, Math.floor(ms / 1000)); return x < 60 ? x + "s" : Math.floor(x / 60) + "m " + (x % 60) + "s"; };
  function ticker() {
    if (!visible() || Date.now() === fetchedAt) return;
    tick();
    const groups = new Map();
    for (const n of document.querySelectorAll(".step.live[data-live]")) {
      const a = SESS[n.dataset.live]?.activity, since = n.dataset.since != null ? Number(n.dataset.since) : a?.[3];
      if (since == null || !Number.isFinite(since)) continue;
      const text = running(NOW - since), sd = n.querySelector(".sd");
      if (sd && sd.textContent !== text) sd.textContent = text;
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
