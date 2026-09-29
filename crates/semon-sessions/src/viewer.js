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
  const shortHost = (s) => { const h = hostOf(s).split(".")[0]; return h.length > 14 ? h.slice(0, 14) + "…" : h; };
  const branchOf = (s) => s.worktree ?? s.branch ?? "No branch";
  const shortModel = (model) => String(model ?? "Unknown model").replace(/^gpt-\d+-/i, "").replace(/^claude-/i, "").replace(/^(opus|sonnet|haiku)-(\d+)-(\d+)$/i, "$1 $2.$3").replace(/^(opus|sonnet|haiku)-(\d+)\.(\d+)$/i, "$1 $2.$3").replace(/^(opus|sonnet|haiku)-(\d+)$/i, "$1 $2");
  // A harness is named in plain text, never drawn: no logo and no vendor colour. "short" gives "Claude" where the line is tight.
  // The harness is named in plain muted text (.hname); the word itself tells Claude and Codex apart, so its hue is not used here.
  // The short name ("Claude") gets the long one ("Claude Code") as its tooltip; the long one repeats itself, so it has none.
  const harnessName = (harness, short = false) => { const name = el("span", "hname h-" + harness, (short ? HARNESS_SHORT : HARNESS)[harness] ?? harness); if (short && HARNESS[harness] && HARNESS[harness] !== HARNESS_SHORT[harness]) name.dataset.tip = HARNESS[harness]; return name; };
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
  // A handoff as a sentence: who did what to whom. With `links`, each name (other than the viewed session and you) opens that
  // session, at the turn the handoff sits in there.
  function sentence(h, viewer, links = false) {
    const W = (id, turn) => {
      if (links && id !== "you" && id !== viewer && SESS[id]) { const b = btn("who-link", nameOf(id), "Open " + nameOf(id)); b.addEventListener("click", (e) => { e.stopPropagation(); goSession(id, turn); }); return b; }
      return el("span", "who", nameOf(id));
    };
    const from = () => W(h.from, HOLDS.get(h.id)?.id), to = () => W(h.to, STARTS.get(h.id)?.id);
    const kindOf = (id) => SESS[id]?.kind === "Subagent" ? "subagent" : SESS[id]?.kind ?? "";
    if (h.kind === "ask") return [I.ask, [W("you"), el("span", "verb", " asked "), to()]];
    if (h.kind === "spawn") return viewer === h.to ? [I.in, [el("span", "verb", "Brief from "), from()]] : [I.out, [from(), el("span", "verb", " handed off to " + kindOf(h.to) + " "), to()]];
    if (h.kind === "relay") return viewer === h.to ? [I.in, [el("span", "verb", "Relay from "), from()]] : [I.relay, [from(), el("span", "verb", " relayed to "), to()]];
    if (h.kind === "move") {
      const mb = links ? btn("who-link", MACHINE[h.fromMachine], "Open " + MACHINE[h.fromMachine]) : el("span", "who", MACHINE[h.fromMachine]);
      if (links) mb.addEventListener("click", (e) => { e.stopPropagation(); go({ v: "machine", id: h.fromMachine }); });
      return [I.move, [el("span", "verb", "Semon moved "), el("span", "who", nameOf(h.to)), el("span", "verb", " from "), mb, el("span", "verb", " to " + MACHINE[h.toMachine])]];
    }
    const what = { question: " asked you", result: " sent you a result", decision: " needs your decision" }[h.ask];
    const answered = h.status === "done" && (h.ask === "question" || h.ask === "decision");
    return [answered ? I.done : h.ask === "question" ? I.qc : h.ask === "decision" ? I.decide : I.result, [viewer === h.from ? el("span", "who", nameOf(h.from)) : from(), el("span", "verb", what)]];
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
    const job = moved ? (shrank(m.tok, TOK[sid]) ? reload(sid) : tail(sid)) : null, work = job;
    if (work) work.then(() => { if (route === r && rendered === r) refresh(null); }, () => {});
  }
  // "Load earlier" at the top of a transcript, and "Load later" at its end when a deep link loaded a middle page.
  function pager(sid, where, label) {
    const w = el("div", "list"), b = el("button", "more", label); b.type = "button"; if (where === "before") b.dataset.loadEarlier = ""; w.append(b); // data-load-earlier: a stable hook for the budget check
    b.addEventListener("click", () => {
      stopOpeningEndPin();
      const m = TXM[sid], box = phone.matches ? document.documentElement : $("#main"), h0 = box.scrollHeight; b.disabled = true;
      fetchTx(sid, where === "before" ? "before=" + m.from : "after=" + m.to, where).then(() => {
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
      adopt(m); route = routeOf(location); LIVE.version = m.version; remember(m);
      if (route.v === "machines" && NAV_MACHINES) { location.assign(NAV_MACHINES); return; }
      try { history.replaceState({ ...route, scrollTop: 0 }, "", urlOf(route) + (route.v === "session" ? location.hash : "")); } catch {}
      const done = () => {
        render();
        if (route.v === "session" && route.turn) { revealTurn(route.turn, true); if (location.hash) requestAnimationFrame(() => requestAnimationFrame(revealEntryHash)); }
        else if (route.v === "session" && location.hash) revealEntryHash();
        else if (route.v === "session") { openSessionAtEnd(); syncJump(); }
        else quietTop();
        schedule(2000); setInterval(ticker, 1000);
      };
      const p = load(route); if (p) p.then(done, done); else done();
    }, (err) => { $("#page").replaceChildren(el("p", "empty", "Couldn't load the sessions: " + err.message)); });
  }

  // ---- State & navigation ---------------------------------------------------------------
  const phone = window.matchMedia("(max-width: 760px)");
  let route = { v: "home" }; // (before the layout preferences, which read it)
  let wideMode = false, railMode = false, treePrefs = {};
  try { wideMode = localStorage.getItem("semon.wide") === "1"; } catch {}
  try { railMode = localStorage.getItem("semon.rail") === "1"; } catch {}
  try { const saved = JSON.parse(localStorage.getItem("semon.tree") ?? "{}"); if (saved && typeof saved === "object" && !Array.isArray(saved)) treePrefs = pruneTreePrefs(saved); } catch {}
  const app = $(".app");
  const syncLayoutPrefs = () => { app.classList.toggle("rail", railMode && !phone.matches); $("#page").classList.toggle("wide-mode", wideMode && !phone.matches && route.v === "session"); };
  function setWideMode(on) { wideMode = on; try { localStorage.setItem("semon.wide", on ? "1" : "0"); } catch {} syncLayoutPrefs(); $(".wide-toggle")?.setAttribute("aria-pressed", String(on)); }
  function setRailMode(on) { railMode = on; try { localStorage.setItem("semon.rail", on ? "1" : "0"); } catch {} syncLayoutPrefs(); expandedAll = null; renderLanes(); const b = $("#rail-toggle"); b?.setAttribute("aria-expanded", String(!on)); b?.setAttribute("aria-label", on ? "Expand sidebar" : "Collapse sidebar"); b?.setAttribute("data-tip", on ? "Expand sidebar" : "Collapse sidebar"); }
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
  const railToggle = $("#rail-toggle"); railToggle.append(icon(I.sidebar)); railToggle.setAttribute("aria-expanded", String(!railMode)); railToggle.setAttribute("data-tip", railMode ? "Expand sidebar" : "Collapse sidebar"); railToggle.setAttribute("aria-label", railMode ? "Expand sidebar" : "Collapse sidebar"); railToggle.addEventListener("click", () => setRailMode(!railMode)); syncLayoutPrefs();
  let groupBy = "recent"; let query = ""; let analyticsRange = 7, analyticsMeasure = "hours";
  const sessionFilters = { repo: "", machine: "", harness: "", model: "" };
  let pendingSessionOpen = null;
  let accountOpen = false;
  // The phone's account menu adds a history entry, so the back gesture closes it.
  let accountSheet = false;
  // What to do once the account menu's history entry has been stepped back over (leaving the page from one of its items).
  let afterPop = null;
  try { history.scrollRestoration = "manual"; } catch {}
  const SHOW_ALL = { messages: true, tools: true, thinking: true };
  let show = { ...SHOW_ALL }; let find = ""; let findOpen = false;
  const currentScroll = () => phone.matches ? window.scrollY : $("#main").scrollTop;
  const restoreScroll = (top) => { if (phone.matches) window.scrollTo(0, top); else $("#main").scrollTop = top; };
  const saveHistoryScroll = () => { try { if (history.state?.v) history.replaceState({ ...history.state, scrollTop: currentScroll() }, ""); } catch {} };
  let scrollSaveFrame = false;
  const queueScrollSave = () => { if (scrollSaveFrame) return; scrollSaveFrame = true; requestAnimationFrame(() => { scrollSaveFrame = false; saveHistoryScroll(); }); };
  window.addEventListener("scroll", queueScrollSave, { passive: true });
  $("#main").addEventListener("scroll", queueScrollSave, { passive: true });
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
    stopOpeningEndPin(); navAbort?.abort(); navAbort = null;
    if (r.v === "timeline") { r = { ...r, v: "analytics" }; try { history.replaceState({ ...r, scrollTop: r.scrollTop ?? currentScroll() }, "", urlOf(r)); } catch {} }
    if (r.v === "machines" && NAV_MACHINES) { location.assign(NAV_MACHINES); return; }
    if (!fromHistory) saveHistoryScroll();
    closeAccountMenu(true, true);
    dropErrors(true); // (first: it drops a range the error stepper moved, and that is not kept)
    // The session left is kept for opening it again; weighing it waits until the frame the click drew has been painted.
    if (route.v === "session" && (r.v !== "session" || r.id !== route.id) && TX[route.id] && TXM[route.id]) { const sid = route.id, entries = TX[sid], meta = { ...TXM[sid] }; requestAnimationFrame(() => setTimeout(() => cacheTx(sid, entries, meta), 0)); }
    if (r.v !== "session" || r.id !== route.id) show = { ...SHOW_ALL };
    route = r; find = ""; findOpen = false; closeDrawer(true); clearNewEntries();
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
  window.addEventListener("popstate", (e) => {
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
  function accountPopover() {
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
    closeAccountMenu();
    const menu = accountPopover();
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
  function renderNav() {
    const nav = $("#nav"); nav.replaceChildren();
    // A session or a trace sits under Sessions, a machine under Machines.
    const under = { home: ["home"], analytics: ["analytics"], sessions: ["sessions", "session", "trace"], machines: ["machines", "machine"] };
    const item = (v, label, ic, count, hot) => { const b = el("button", "nav-item"); b.type = "button"; b.dataset.go = v; if (under[v].includes(route.v)) b.setAttribute("aria-current", "page"); b.append(icon(ic, "icon"), el("span", null, label)); if (count) b.append(el("span", "cnt" + (hot ? " hot" : ""), String(count))); b.addEventListener("click", () => go({ v })); nav.append(b); };
    item("home", "Home", I.home, inbox().length, true);
    item("sessions", "Sessions", I.sessions, Object.keys(SESS).length);
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
  const matchesTree = (sid, children, seen = new Set()) => {
    if (seen.has(sid)) return false;
    seen.add(sid);
    return sessMatch(SESS[sid], query) || (children.get(sid) ?? []).some((s) => matchesTree(s.id, children, seen));
  };
  // An open parent lists its waiting children, then its running ones (at most 8), then the newest finished ones until three rows are
  // listed. "All N" opens the rest: a sheet on a phone, the whole list in the tree on a wide screen. While a search is typed, the
  // children that match it are the ones listed.
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
  let expandedAll = null, revealedFor = null, expandedPath = new Set(), expandedUnder = new Set();
  const ancestorsOf = (id) => { const out = new Set(); for (let p = id && SESS[id] ? parentOf(id) : null; p && SESS[p] && p !== id && !out.has(p); p = parentOf(p)) out.add(p); return out; };
  // Fills a parent's group and says whether it holds the parent's whole list, which sticks the parent's row (stickRow).
  function treeGroupFill(group, parent, kids, children, depth, rail) {
    const { current, ancestors } = routedPath(), rank = new Map(kids.map((c) => [c.id, kidRank(c, children)]));
    const sorted = [...kids].sort((a, b) => rank.get(a.id) - rank.get(b.id) || b.last - a.last), keep = new Set();
    // With a search typed, the children that match it are the pool; the short-list rule caps them like any other list.
    const matching = query ? sorted.filter((c) => matchesTree(c.id, children)) : [], pool = matching.length ? matching : sorted;
    for (const c of pool) if (rank.get(c.id) < 2 && keep.size < TREE_ACTIVE) keep.add(c.id);
    for (const c of sorted) if (c.id === current || ancestors.has(c.id) || expandedPath.has(c.id)) keep.add(c.id);
    for (const c of pool) if (keep.size < TREE_ROWS) keep.add(c.id);
    const listed = sorted.filter((c) => keep.has(c.id)), hidden = kids.length - listed.length;
    if (!hidden && expandedAll === parent.id) expandedAll = null; // nothing is left to open: "Show fewer" would have nothing to fold
    const full = hidden > 0 && !rail && (expandedAll === parent.id || expandedUnder.has(parent.id));
    group.replaceChildren();
    for (const child of full ? sorted : listed) group.append(buildLaneItem(child, depth + 1, children, rail));
    if (!hidden || full) return full && expandedAll === parent.id;
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
      if (viewerEl === d) { viewerEl = null; if (history.state?.sheet) { skipPop = true; history.back(); } else if (pendingSessionOpen) { const id = pendingSessionOpen; pendingSessionOpen = null; goSession(id); } }
      if (!picked) ($('#lanes .tree-all[data-id="' + CSS.escape(parent.id) + '"]') ?? trigger).focus();
      if (LIVE.pending) refresh();
    });
    viewerEl = d; document.documentElement.classList.add("viewer-open"); d.showModal(); close.focus({ focusVisible: false }); try { history.pushState({ ...route, sheet: 1, scrollTop: currentScroll() }, ""); } catch {}
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
    const main = el("span", "srow-main"), ag = el("span", "ag", ago(s.last)), nm = el("span", "nm", s.name); nm.dataset.tip = s.name; nm.dataset.tipClipped = ""; main.append(dot(s.state, !rail), nm, ag); // in the rail the row has the tip (the name), and a dot inside it would answer first
    if (rail && allKids.some((x) => x.state === "work" || x.state === "wait")) { const childDot = dot(urgentDescendant(s.id, children) ?? "work", false); childDot.classList.add("child-dot"); childDot.setAttribute("aria-hidden", "true"); main.append(childDot); }
    if (kids.length && !rail && allKids.length) { const summary = el("span", "tree-summary", String(allKids.length)); summary.dataset.tip = parts.join(" · "); summary.classList.toggle("wait", allKids.some((x) => x.state === "wait")); ag.before(summary); }
    const meta = el("span", "srow-meta"); meta.append(icon(I.machine), el("span", "host", shortHost(s)), el("span", "repo-short", s.repo ?? "no repo")); meta.querySelector(".host").dataset.tip = "Machine: " + hostOf(s); meta.querySelector(".repo-short").dataset.tip = spaced("Repo: " + (s.repo ?? "none") + " · " + (s.worktree ? "Worktree: " : "Branch: ") + branchOf(s));
    row.append(main, meta); row.addEventListener("click", () => goSession(s.id)); line.append(row); if (lineToggle) line.append(lineToggle); item.append(line);
    item.addEventListener("keydown", (e) => {
      if (kids.length && !rail && (e.key === "ArrowLeft" || e.key === "ArrowRight")) { if (e.target !== item && e.target !== row && e.target !== lineToggle) return; const next = e.key === "ArrowRight"; if ((item.getAttribute("aria-expanded") === "true") !== next) { e.preventDefault(); item.querySelector(":scope > .tree-row .tree-toggle")?.click(); } }
      else if ((e.key === "Enter" || e.key === " ") && e.target === item) { e.preventDefault(); goSession(s.id); }
    });
    if (kids.length && !rail) { const group = el("div", "tree-group"); group.dataset.depth = String(Math.min(depth + 1, 4)); group.setAttribute("role", "group"); group.setAttribute("aria-label", "Sessions spawned by " + s.name); const full = treeGroupFill(group, s, kids, children, depth, rail); item.append(group); if (full && open) stickRow(line, s); }
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
    const focus = laneFocus(), children = sessionChildren(), lanes = Object.values(SESS).filter((s) => s.lane && !parentOf(s.id) && matchesTree(s.id, children)).sort((a, b) => b.last - a.last);
    if (expandedAll && (phone.matches || railMode || !SESS[expandedAll])) expandedAll = null;
    expandedPath = expandedAll ? ancestorsOf(expandedAll) : new Set(); expandedUnder = expandedAll ? new Set(descendantsOf(expandedAll, children).map((x) => x.id)) : new Set();
    const box = $("#lanes"); box.replaceChildren();
    for (const s of lanes.slice(0, 8)) box.append(buildLaneItem(s, 0, children, railMode && !phone.matches));
    if (!lanes.length) { const empty = el("p", "ghead", "No sessions match"); empty.setAttribute("role", "none"); box.append(empty); }
    const q = $("#q"); if (document.activeElement !== q) q.value = query;
    restoreLaneFocus(focus);
    // A stuck row covers the top of the sidebar: what is scrolled into view (the open session, after a navigation) stays clear of it.
    const stuck = box.querySelector(".tree-row.stuck"), navigated = revealedFor !== route; revealedFor = route;
    ($("#side-list") ?? $("#sidebar")).style.scrollPaddingTop = stuck ? stuck.offsetHeight + 8 + "px" : "";
    if (stuck && navigated) box.querySelector('.srow[aria-current="page"]')?.scrollIntoView({ block: "nearest" });
  }

  // ---- Top bar ---------------------------------------------------------------------------------------
  // The same on every page: the menu button (phones), the title, and at most two actions. A detail page adds a crumb up a
  // level and a second line of labels. Labels are information, never a control: their full values are in the tooltip and in
  // the session menu. On a session, Find takes over the bar and the filters sit under it as chips.
  const btn = (cls, text, label) => { const b = el("button", cls, text); b.type = "button"; if (label) b.setAttribute("aria-label", label); return b; };
  const kindText = (s) => s.kind ?? HARNESS[s.harness];
  const modelIdOf = (s) => Object.keys(s.tokens_by_model ?? {})[0] ?? s.model;
  function renderTopbar(title, crumb, opts = {}) {
    closeAccountMenu(); // the bar is redrawn from scratch, the desktop menu with it: close it properly, not by detaching it
    const bar = $("#topbar"), s = opts.session; clearBox(bar, route); bar.classList.remove("scrolled"); bar.classList.toggle("session-bar", !!s);
    // What the bar holds is added through `put`, so the range control on Analytics (a persistent control) stays where it is.
    const put = placer(bar), sink = { append: put };
    const account = (into) => { const a = accountWidget(false); if (a) (into ? into.append(a) : put(a)); };
    if (s && errOn(s.id)) { errorsBar(sink); account(); put.done(); return; }
    if (s && findOpen) { findBar(sink, s, account); put.done(); return; }
    const m = btn("ibtn lead", null, "Open navigation"); m.id = "lead-btn"; m.setAttribute("aria-controls", "sidebar"); m.setAttribute("aria-expanded", "false"); m.append(icon(I.menu)); m.addEventListener("click", openDrawer); put(m);
    const t = el("div", "ttl"), l1 = el("div", "l1");
    if (opts.lineage?.length) { const parent = opts.lineage.at(-1), c = btn("crumb", parent.name, "Up to " + parent.name); c.addEventListener("click", () => goSession(parent.id)); l1.append(c, el("span", "crumb-sep", "›")); }
    else if (crumb) { const c = btn("crumb", crumb.label, "Back to " + crumb.label); c.addEventListener("click", crumb.go); l1.append(c, el("span", "crumb-sep", "›")); }
    const tt = el("span", "t", title); tt.dataset.tip = title; tt.dataset.tipClipped = ""; if (s) l1.append(stateLead(s)); l1.append(tt); t.append(l1);
    if (opts.line2) { const l2 = el("div", "meta-line"); opts.line2(l2); t.append(l2); if (s) requestAnimationFrame(() => { if (l2.isConnected) fitMeta(l2); }); }
    put(t);
    if (opts.analytics) { put(rangeControl(bar)); account(); put.done(); return; }
    if (!s) { account(); put.done(); return; }
    const fb = btn("ibtn", null, "Find and filter"); fb.id = "find-btn"; fb.append(icon(I.search)); fb.addEventListener("click", () => { findOpen = true; render(); $("#find")?.focus(); });
    const mb = btn("ibtn", null, "Session menu: details, cost and actions"); mb.id = "more-btn"; mb.setAttribute("aria-haspopup", "dialog"); mb.setAttribute("aria-expanded", "false"); mb.append(icon(I.more)); mb.addEventListener("click", () => openSessionMenu(s, mb));
    put(fb, mb); account(); put.done();
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
  const ERR = { on: false, sid: null, slots: [], listed: false, count: 0, version: null, k: -1, slot: null, saved: null, range: null, tools: true, chain: Promise.resolve(), gen: 0 };
  const ERR_NEAR = 400, ERR_AROUND = 40; // slots: a page (at most 200 entries) or two away is added; 40 entries of context above
  const errLive = el("div", "sr-only"); errLive.setAttribute("role", "status"); errLive.setAttribute("aria-live", "polite"); document.body.append(errLive);
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
    stopOpeningEndPin(); find = "";
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
    return Promise.resolve(near ? extend() : null).then(around).then(() => true);
  }
  // The session's own step for a slot: not one in a child run's work drawn inside it.
  function errNode(sid, slot) {
    const e = (TX[sid] ?? []).find((x) => x.k === "tool" && x.slot === slot); if (!e?.key) return null;
    return [...$("#page").querySelectorAll(".turns .step[data-e]")].find((n) => n.dataset.e === e.key) ?? null;
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
      const b0 = $("#topbar .lab-errs") ?? $('#topbar .chip[data-filter="failures"]'), b = b0 && !b0.getClientRects().length ? $("#more-btn") : b0; /* on a phone the line is not drawn: focus goes to ⋯ */ if (b && !b.hidden && document.activeElement !== b && (!document.activeElement || document.activeElement === document.body || !document.activeElement.isConnected)) b.focus({ preventScroll: true });
    });
  }
  // Keys while the mode is on: n and p (and Enter, Shift+Enter in the bar) step, Escape closes. Not while typing, and not
  // under an open sheet.
  document.addEventListener("keydown", (e) => {
    if (!ERR.on || e.defaultPrevented || e.altKey || e.ctrlKey || e.metaKey || document.querySelector("dialog[open]")) return;
    // The drawer and an open menu have the keys first: Escape closes them and leaves the mode on.
    if (document.body.classList.contains("drawer-open") || document.querySelector(".menu, .lineage-menu")) return;
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
    l2.append(lab(shortModel(s.model), "Model: " + modelIdOf(s), 3));
    // Failed steps and runs are the two labels that lead somewhere, so they are the last two the fitter drops.
    if (failed) l2.append(lab(failed + " failed", failed + (failed === 1 ? " failed step" : " failed steps") + ": step through them", 0, "lab-errs", () => openErrors(s.id)));
    const runs = descendantsOf(s.id, sessionChildren());
    if (runs.length) l2.append(lab(runs.length + (runs.length === 1 ? " run" : " runs"), runs.length + (runs.length === 1 ? " run" : " runs") + " under this session: open the list with their cost", 1, "lab-runs", () => openSessionMenu(s, $("#more-btn"), ".runs")));
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
  const matchCount = () => find || !show.messages || !show.tools || !show.thinking ? $("#page").querySelectorAll(".turns .msg, .turns .bubble, .turns .step, .turns .event, .turns .child-card").length : null;
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
    // One choice at a time: everything, only messages, or only steps. "Failed steps" is not a filter: it steps through them (errors mode).
    const MODES = { all: { ...SHOW_ALL }, messages: { messages: true, tools: false, thinking: false }, steps: { messages: false, tools: true, thinking: false } };
    const mode = show.messages && show.tools ? "all" : show.messages ? "messages" : "steps";
    const chip = (key, label, count) => { const b = btn("chip"); b.dataset.filter = key; b.setAttribute("aria-pressed", String(mode === key)); b.append(el("span", null, label)); if (count != null) b.append(el("span", "n", String(count))); b.addEventListener("click", () => { if (key === "failures") { openErrors(s.id); return; } show = { ...MODES[key] }; render(); }); chips.append(b); };
    chip("all", "All"); chip("messages", "Messages"); chip("steps", "Steps"); if (failed) chip("failures", "Failed steps", failed);
    bar.append(chips);
  }
  // One observer for the current page title; the previous page's is disconnected so it can't flip the new bar.
  let titleObs = null;
  function observeTitle() { syncBarLine(); }
  // The bar's divider shows only once the page has scrolled.
  function syncBarLine() { const y = phone.matches ? window.scrollY : $("#main").scrollTop; $("#topbar").classList.toggle("scrolled", y > 4); }
  window.addEventListener("scroll", syncBarLine, { passive: true });
  $("#main").addEventListener("scroll", syncBarLine, { passive: true });
  window.addEventListener("resize", () => { const l2 = $("#topbar .meta-line"); if (l2 && route.v === "session") fitMeta(l2); syncLayoutPrefs(); syncJump(); }, { passive: true });

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
    const open = () => { viewerEl = d; document.documentElement.classList.add("panel-open"); d.showModal(); body.focus({ preventScroll: true }); try { history.pushState({ ...route, sheet: 1, scrollTop: currentScroll() }, ""); } catch {} };
    return { d, body, show: open };
  }

  // The session menu: actions, then details, then cost. It is the one place for all three.
  const RUNS_CAP = 5;
  function openSessionMenu(s, anchor, scrollTo) {
    const kids = descendantsOf(s.id, sessionChildren());
    const { d, body, show: open } = panel(s.name, { cls: "anchored session-menu", label: "Session menu for " + s.name, sub: [STATE[s.state], kindText(s), shortModel(s.model)].join(" · "), onClose: () => { anchor?.setAttribute("aria-expanded", "false"); anchor?.focus({ focusVisible: false }); } });
    const acts = el("div", "menu-list"); acts.setAttribute("role", "menu");
    const cmd = s.harness === "codex" ? "codex resume " + s.id : "claude --resume " + (s.sessionId ?? s.id);
    const copy = btn("menu-item"); copy.setAttribute("role", "menuitem"); copy.append(icon(I.copy, "icon"), el("span", null, "Copy resume command")); copy.addEventListener("click", () => { navigator.clipboard?.writeText(cmd).then(() => { copy.children[1].textContent = "Copied"; }, () => { copy.children[1].textContent = cmd; }); }); acts.append(copy);
    if (s.harness === "claude") { const a = btn("menu-item"); a.setAttribute("role", "menuitem"); a.append(icon(I.ext, "icon"), el("span", null, "Open in claude.ai")); acts.append(a); }
    if (!phone.matches) { const w = btn("menu-item"); w.setAttribute("role", "menuitemcheckbox"); w.setAttribute("aria-checked", String(wideMode)); w.append(icon(I.wide, "icon"), el("span", null, "Wide transcript"), el("span", "switch")); w.addEventListener("click", () => { setWideMode(!wideMode); w.setAttribute("aria-checked", String(wideMode)); }); acts.append(w); }
    // On a phone the line of labels is not in the bar, so what it held that leads somewhere is reached here.
    if (phone.matches) {
      const failed = countOf(s, "errors") ?? 0;
      if (failed) { const e = btn("menu-item menu-errors"); e.setAttribute("role", "menuitem"); const mark = el("span", "dot err"); mark.setAttribute("aria-hidden", "true"); e.append(mark, el("span", null, failed + " failed"), el("span", "menu-note", "Step through")); e.addEventListener("click", () => { d.close(); openErrors(s.id); }); acts.append(e); }
      if (kids.length) { const q = btn("menu-item menu-runs"); q.setAttribute("role", "menuitem"); q.append(icon(I.stack, "icon"), el("span", null, "Runs · " + kids.length)); q.addEventListener("click", () => { body.querySelector(".runs")?.scrollIntoView({ block: "start" }); }); acts.append(q); }
    }
    const a1 = el("section", "panel-sec"); a1.append(acts); body.append(a1);
    const det = el("section", "panel-sec"); det.append(el("h3", null, "Details"));
    const dl = el("dl", "kv"), machine = MACHINE[s.machine] ?? s.machine ?? "Unknown machine";
    const calls = countOf(s, "calls"), errorCount = countOf(s, "errors") ?? 0;
    const rows = [["Status", STATE[s.state] + " · " + turnsLabel(s)], ...(s.kind ? [["Kind", s.kind]] : []), ["Harness", HARNESS[s.harness]], ["Model", modelIdOf(s), true], ["Machine", machine + (hostOf(s) !== machine ? " · " + hostOf(s) : "") + (s.movedFrom ? " (moved from " + (MACHINE[s.movedFrom] ?? s.movedFrom) + ")" : "")], ["Directory", s.cwd ?? s.dir ?? s.directory, true], [s.worktree ? "Worktree" : "Branch", branchOf(s), true], ["Tool calls", calls == null ? "—" : String(calls)], ...(errorCount ? [["Errors", String(errorCount)]] : []), ["Started", clock(s.start)], ["Duration", dur(s.start, s.state === "work" ? null : s.last)], ["Process id", s.pid, true], ["Session id", s.sessionId ?? s.id, true]];
    for (const [k, v, mono] of rows) { if (v == null || v === "") continue; dl.append(el("dt", null, k), el("dd", mono ? "mono" : null, String(v))); }
    det.append(dl); body.append(det, costSection(s, kids, d));
    anchor?.setAttribute("aria-expanded", "true");
    open(); if (scrollTo) body.querySelector(scrollTo)?.scrollIntoView({ block: "nearest" }); return d;
  }
  // 739,682 reads "740k" and 12,422,228 "12.4M"; the exact figure is the cell's tooltip.
  const compactCount = (n) => { if (n < 1e3) return String(n); if (n < 1e4) return +(n / 1e3).toFixed(1) + "k"; const k = Math.round(n / 1e3); return k < 1e3 ? k + "k" : +(n / 1e6).toFixed(1) + "M"; };
  const MENU_KINDS = [["Input", ["input"]], ["Output", ["output"]], ["Cache write", ["cache_write_5m", "cache_write_1h"]], ["Cache read", ["cache_read"]]];
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
    const t = btn("disclose"); t.setAttribute("aria-expanded", "false"); t.append(el("span", null, "Tokens by model"), icon(I.chev, "chev"));
    const tb = el("div", "tokens"); tb.hidden = true;
    for (const [modelId, model] of Object.entries(all.by_model ?? {})) {
      const priced = model.usd != null && !missing.includes(modelId); tb.append(el("div", "tok-model", modelId));
      for (const [label, keys] of MENU_KINDS) { const tokens = keys.reduce((n, k) => n + (Number(model.tokens?.[k]) || 0), 0), amount = keys.reduce((n, k) => n + (Number(model.usd_by_kind?.[k]) || 0), 0), r = el("div", "tok-line"); if (tokens === 0 && (!priced || amount < 0.005)) continue; const count = el("span", null, tokens ? compactCount(tokens) : ""); if (tokens) { count.dataset.tip = tokens.toLocaleString() + " tokens"; count.append(el("span", "sr-only", " (" + tokens.toLocaleString() + ")")); } r.append(el("span", null, label), count, el("span", null, priced ? asMoney(amount) : "—")); tb.append(r); }
    }
    t.addEventListener("click", () => { tb.hidden = !tb.hidden; t.setAttribute("aria-expanded", String(!tb.hidden)); });
    sec.append(t, tb); return sec;
  }
  document.addEventListener("click", (e) => {
    const account = $(".account-popover"); if (account && !account.parentElement.contains(e.target)) closeAccountMenu(); });

  // ---- Home: what needs you, then what is running ------------------------------------------------------
  const secHead = (title, n) => { const s = el("div", "sec-h", title); s.append(el("span", "n", String(n))); return s; };
  const upCount = () => Object.keys(MACHINE).filter((m) => MACHINE_UP[m]).length;
  let allAnswered = false;
  function renderHome(page) {
    const open = inbox(), w = working().sort((a, b) => b.last - a.last), many = Object.keys(MACHINE).length > 1;
    const head = el("div", "ph"); const h1 = el("h1", null, "Home"); head.append(h1);
    const sub = el("div", "sub"); for (const [v, l] of [[open.length, "waiting on you"], [w.length, "working"], [upCount() + " of " + Object.keys(MACHINE).length, Object.keys(MACHINE).length === 1 ? "machine up" : "machines up"]]) { const x = el("span"); x.append(el("b", null, String(v)), l); sub.append(x); }
    head.append(sub); page.append(head); observeTitle(h1);
    page.append(secHead("Needs you", open.length));
    const list = el("div", "list");
    for (const h of open) list.append(inboxItem(h, false));
    if (!open.length) list.append(el("p", "empty", "Nothing is waiting on you."));
    page.append(list);
    page.append(secHead("Working now", w.length));
    const live = el("div", "list");
    for (const s of w) live.append(liveRow(s, many));
    if (!w.length) live.append(el("p", "empty", "Nothing is running."));
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
    ctx.append(el("span", null, [HARNESS[s.harness], MACHINE[s.machine]].join(" · "))); if (t?.out.length) ctx.append(traceBtn(t));
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
    r.append(dot(s.state), el("span", "nm", s.name), el("span", "ag", s.state === "work" ? HARNESS[s.harness] : ago(s.last)));
    const cur = (TURNS[s.id] ?? []).at(-1), msg = cur?.start?.brief ?? cur?.u?.text;
    const inb = cur?.start ?? (cur?.u ? { from: "you" } : H.find((h) => h.to === s.id && h.kind !== "move"));
    r.append(el("span", "for", [showMachine ? MACHINE[s.machine] : null, inb ? (inb.from === "you" ? "for you" : "for " + nameOf(inb.from)) : null, msg ? oneLine(msg) : null].filter(Boolean).join(" · ")));
    if (s.activity && s.state === "work") { const a = el("span", "act"); a.append(el("span", "spin"), el("span", null, s.activity[0]), el("code", null, s.activity[1]), el("span", "el", Math.max(0, s.activity[2]) + "s")); r.append(a); }
    return r;
  }

  // ---- Machines: where sessions run, and what happens when a machine goes away ------------------------------
  const onMachine = (m) => Object.values(SESS).filter((s) => s.machine === m).sort((a, b) => (RANK[a.state] ?? 3) - (RANK[b.state] ?? 3) || b.last - a.last);
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
      const list = el("div", "list"); for (const s of here) list.append(liveRow(s, false)); page.append(list);
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
  function traceMeta(body, st, text, sid, turn, note) {
    const meta = el("div", "meta"), s = SESS[sid]; const sw = el("span", "stat " + st); sw.append(st === "work" ? el("span", "spin") : dot(st, false), text); meta.append(sw);
    if (s) meta.append(el("span", "chip-h " + hcls(sid), (s.kind ?? HARNESS[s.harness]) + " · " + MACHINE[s.machine]));
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
    const hop = (cls, ic, hc, parts, at) => { const x = el("div", "hop " + cls); const node = el("div", "node " + hc); node.append(icon(ic)); const body = el("div", "body"); const sent = el("div", "sent"); sent.append(...parts); if (at != null) sent.append(el("span", "tm", clock(at))); body.append(sent); x.append(node, body); flow.append(x); return [x, body]; };
    const s0 = root.start, text = s0 ? s0.brief : root.u?.text;
    const [ic0, parts0] = s0 ? sentence(s0, null) : root.u ? sentence({ kind: "ask", to: root.sid }, null) : [I.more, [el("span", "who", SESS[root.sid].name), el("span", "verb", " · a turn whose start isn't in these logs")]];
    const [r0, b0] = hop("k-root", ic0, hcls(s0 ? s0.from : root.u ? "you" : root.sid), parts0, s0?.at); r0.dataset.turn = root.id; if (s0) r0.dataset.h = s0.id;
    if (text) clampBrief(b0, text);
    const e0 = turnEnd(root); traceMeta(b0, e0?.st ?? "idle", e0?.text ?? "Nothing recorded", root.sid, root);
    const walk = (t) => {
      for (const h of t.sent) {
        const result = h.kind === "toyou" && h.ask === "result";
        const c = h.kind === "spawn" || h.kind === "relay" ? STARTS.get(h.id) : null, tgt = h.kind === "toyou" ? h.from : h.to, [ic, parts] = result ? [I.result, [el("span", "verb", statWord(h))]] : sentence(h, null);
        const [x, b] = hop("child k-" + h.kind + " s-" + h.status + (c || h.kind === "toyou" || h.kind === "move" ? "" : " stub"), ic, hcls(tgt), parts, h.at); x.dataset.h = h.id; if (c) x.dataset.turn = c.id;
        if (result) { n++; continue; }
        clampBrief(b, h.brief); const an = answerEl(h, "result"); if (an) b.append(an);
        if (h.result) { const r = el("div", "result"); r.append(el("span", "rl", "Result:")); const s = el("span"); inline(s, h.result); r.append(s); b.append(r); }
        if (h.kind === "move") { traceMeta(b, "done", "Moved", h.to, t); continue; }
        n++;
        if (h.kind === "toyou") { traceMeta(b, isResult(h) ? SEEN_RESULTS.has(h.id) ? "read" : "new" : h.status === "done" ? "done" : h.status, statWord(h), h.from, t); continue; }
        sess.add(h.to); const e = c && turnEnd(c);
        traceMeta(b, e ? e.st : h.status === "done" ? "done" : h.status, e ? e.text : statWord(h), h.to, c, c ? null : "Its turn isn't in these logs");
        if (c && !seen.has(c.id)) { seen.add(c.id); walk(c); } // a turn is drawn once, so a loop in the data can't recurse forever
      }
    };
    walk(root);
    page.append(flow);
    // The bar's summary line.
    return [sess.size + (sess.size === 1 ? " session" : " sessions"), n + (n === 1 ? " handoff" : " handoffs"), [...new Set([...sess].map((x) => MACHINE[SESS[x]?.machine]).filter(Boolean))].join(", ")].join(" · ");
  }

  // ---- Session page --------------------------------------------------------------------------------------------------
  function renderSession(page, sid) {
    markSeenResults(H.filter((h) => isResult(h) && h.from === sid));
    const s = SESS[sid], head = el("div", "ph sr"); const h1 = el("h1", null, s.name); head.append(h1); page.append(head); observeTitle(h1);
    page.append(transcript(sid));
    page.append(jumpWrap);
  }

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
  const NOW_VERB = { Ran: "Running", Read: "Reading", Edited: "Editing", Patched: "Patching", Wrote: "Writing", "Searched for": "Searching for" };
  const verbNow = (name) => NOW_VERB[verb(name)[1]] ?? verb(name)[1];
  const isCmd = (name) => /^(Bash|shell|exec_command|local_shell)$/.test(name);
  function transcript(sid, opts = {}) {
    const sec = el("section", "transcript"); sec.setAttribute("aria-label", "Transcript");
    const entries = transcriptEntries(TX[sid] ?? [], sid);
    const turnMode = true;
    // The transcript is a list of turns, each with its own entries.
    const box = el("div", "turns"); let tx = box; const hit = (s) => !find || s.toLowerCase().includes(find);
    const filtering = !!find || !show.messages || !show.tools || !show.thinking;
    const keyed = (n, e) => { if (e.key) n.dataset.e = e.key; return n; };
    const range = turnMode ? TXM[sid] : null;
    if (range?.from > 0) box.append(pager(sid, "before", "Load earlier"));
    else if (!filtering) box.append(startedDivider(sid));
    // Adjacent tool calls collapse into one summary line ("Ran 2 commands, read 1 file · 1 failed"),
    // expandable to the individual steps. A lone call stays a single line; while finding, matches show directly.
    let run = []; const maskedIn = new WeakSet();
    const flush = () => {
      if (!run.length) return;
      const steps = el("div", "steps"); run.forEach((r) => steps.append(r.node));
      if (run.length === 1 || filtering) { tx.append(steps); run = []; return; }
      const counts = new Map(); for (const r of run) { const [, , p, one, many] = toolInfo(r.k), c = counts.get(p) ?? { n: 0, one, many }; c.n++; counts.set(p, c); }
      let text = [...counts].map(([p, c]) => p + " " + c.n + " " + (c.n === 1 ? c.one : c.many)).join(", ");
      text = text[0].toUpperCase() + text.slice(1);
      const failed = run.filter((r) => r.err).length, live = run.find((r) => r.live);
      const g = el("div", "tgroup"); if (run[0].key) g.dataset.e = "g:" + run[0].key; const b = el("button", "tsum"); b.type = "button"; b.setAttribute("aria-expanded", "false");
      b.append(live ? el("span", "spin") : icon(I.stack), el("span", "tt", text));
      if (failed) b.append(el("span", "tf", "· " + failed + " failed"));
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
      else if (h) { const hd = el("div", "turn-h"); const [, parts] = sentence(h, sid, true); hd.append(...parts, el("span", "tm", clock(h.at))); blk.append(hd); }
      tx = el("div", "tx"); blk.append(tx); box.append(blk); cur = { t, blk }; };
    for (const e of entries) {
      if (owner && !opts.only.has(owner.get(e.key))) continue;
      if (turnMode && isGap(e)) { closeTurn(); if (!filtering) box.append(el("div", "divider", e.text)); continue; }
      if (turnMode && firsts.has(e.key)) openTurn(firsts.get(e.key));
      // Entries that render nothing (empty thinking, hidden kinds) must not split a run of tool calls.
      if (e.k === "think" && (!show.thinking || find)) continue;
      // Masked thinking is one quiet line per turn (per list, when nested), at the first masked thought's place. Later ones draw
      // nothing, and the line never splits a run of steps: drawn before flush(), it lands ahead of a run still being gathered.
      if (e.k === "think" && isMaskedThought(e)) {
        const scope = cur ?? box;
        if (!maskedIn.has(scope)) { maskedIn.add(scope); const m = keyed(el("div", "thought masked"), e); m.append(el("div", "think-label", "Thinking hidden by the harness")); tx.append(m); }
        continue;
      }
      if (e.k === "tool") {
        if (!show.tools || !hit(e.name + " " + e.arg + " " + (e.in ?? "") + " " + (e.out ?? ""))) continue;
        const [ic, v] = verb(e.name);
        if (e.live) { const r = keyed(el("div", "step live"), e); r.dataset.live = sid; r.append(el("span", "spin"), el("span", "sv", v === "Ran" ? "Running" : v), el("code", "sa", e.arg), el("span", "sd tick", e.secs)); run.push({ node: r, v, k: e.name, live: true, secs: e.secs, key: e.key }); continue; }
        const box2 = keyed(el("div", "step" + (e.ok || e.ok === null ? "" : " err")), e), b = btn(); b.setAttribute("aria-expanded", "false");
        b.append(icon(I[ic]), el("span", "sv", v), el("code", "sa", e.arg), el("span", "sd", e.unfinished ? "No result" : e.exit != null ? "Exit " + e.exit + " · " + e.secs : e.ok ? e.secs : e.ok === null ? "Exit unknown · " + e.secs : "Failed · " + e.secs), icon(I.chev, "chev"));
        // The detail is built when the step is first opened: what was asked first, then what came back.
        let out = null; b.addEventListener("click", () => { if (!out) { out = stepDetail(e, v, ic); out.hidden = true; box2.append(out); } out.hidden = !out.hidden; b.setAttribute("aria-expanded", String(!out.hidden)); });
        box2.append(b); run.push({ node: box2, v, k: e.name, err: e.ok === false, key: e.key }); continue;
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
        const h = H.find((x) => x.id === e.id); if (!hit(h.brief + " " + (h.result ?? ""))) continue;
        // Your own ask is simply your message.
        if (h.kind === "ask") { if (!show.messages) continue; const m = keyed(el("div", "msg user"), e); userBody(m, e, h.brief); tx.append(m); if (cur?.t.start === h) tx.append(el("div", "msg-tm", clock(h.at))); continue; }
        // A relay or brief that starts a turn is that turn's incoming message, under the header that names its sender.
        if (cur && cur.t.start === h && e === cur.t.entries[0]) { if (!show.messages) continue; const m = keyed(el("div", "bubble in"), e); m.dataset.h = h.id; m.append(markdown(h.brief)); tx.append(m); continue; }
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
    if (cmd) out.append(ioHead(isCmd(e.name) ? "Command" : "Input", cmd), el("pre", "in", cmd));
    else if (!e.diff && !e.changes) out.append(ioHead(/^(Read|Grep|Glob)$/.test(e.name) ? (e.name === "Read" ? "File" : "Pattern") : "Input"), el("pre", "in", e.arg));
    if (e.cwd && e.cwd !== ".") out.append(ioHead("Working directory · " + e.cwd));
    const actions = () => {
      if (e.script != null) { const sv = btn("viewall viewscript"); sv.append(icon(I.expand), el("span", null, "View script")); sv.addEventListener("click", () => openScript(e)); out.append(sv); }
    };
    if (e.changes) {
      for (const change of e.changes) { out.append(ioHead("Change · " + change.path + (change.move ? " → " + change.move : ""))); if (change.diff?.length) out.append(diffEl(change.diff)); else out.append(el("div", "noout", "No diff recorded")); }
      if (!e.changes.length) out.append(el("div", "noout", "No changes recorded"));
      actions(); return out;
    }
    if (e.diff) { out.append(ioHead("Change · " + e.arg), diffEl(e.diff)); actions(); return out; }
    if (!e.out) { out.append(ioHead("Output"), el("div", "noout", e.unfinished ? "No result recorded: the machine stopped responding while this ran." : "No output")); actions(); return out; }
    const lines = e.out.split("\n"), cut = lines.length > PREVIEW_LINES, tail = e.ok === false, parts = !!e.cut?.parts?.length;
    const shown = !cut || parts ? lines : tail ? lines.slice(-PREVIEW_LINES) : lines.slice(0, PREVIEW_LINES), more = !!e.more?.length;
    out.append(ioHead(parts || !cut ? "Output" : (tail ? "Output · last " : "Output · first ") + PREVIEW_LINES + (more ? "" : " of " + lines.length) + " lines"));
    if (parts) out.append(outEl(e)); else out.append(el("pre", null, shown.join("\n")));
    if (e.cut) out.append(el("div", "cutnote", cutNoteText(e.cut)));
    else if ([e.in, e.out].some((t) => /…(\(truncated\))?\s*$/.test(t ?? ""))) out.append(el("div", "cutnote", "Cut short in this copy of the logs"));
    if ((cut && !parts) || more) { const all = btn("viewall"); all.append(icon(I.expand), el("span", null, more || parts ? "View all" : "View all " + lines.length + " lines")); all.addEventListener("click", () => openStepViewer(e, v, ic, cmd ? (isCmd(e.name) ? "Command" : "Input") : "Input")); out.append(all); }
    actions(); return out;
  }
  // The whole call, in a full sheet: what the preview cut, fetched from the server when it is longer than the preview.
  function openStepViewer(e, v, ic, inLabel) {
    if (e.more?.length && e.slot != null && !e.full) { const open = (f) => openStepViewer({ ...e, ...f, full: true }, v, ic, inLabel); fullOf(e).then(open, () => open({ fullFailed: true })); return; }
    const status = e.unfinished ? "No result" : e.ok ? "Done · " + e.secs : e.ok === null ? "Exit unknown · " + e.secs : "Failed · " + e.secs;
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
      else { section("Output", e.out); if (e.out) { body.append(outEl(e)); cutNote(e.out); } else body.append(el("p", "vnote", e.unfinished ? "No result recorded." : "No output.")); }
    }
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
    new ResizeObserver(() => { if (t.classList.contains("open") || !t.clientHeight) return; const x = t.scrollHeight > t.clientHeight + 1; more.hidden = !x; t.classList.toggle("clipped", x); }).observe(t);
    parent.append(more); return t;
  }
  // A session this one started: a card that opens it. Its state, what it is, the brief, and what it is doing or returned.
  function childCard(h, c) {
    const b = btn("child-card"); b.dataset.h = h.id;
    const calls = countOf(c, "calls");
    const head = el("span", "cc-head"); head.append(el("span", "cc-name", c.name), stateLabel(c.state), icon(I.chev, "chev"));
    b.append(head, el("span", "cc-meta", [kindText(c), shortModel(c.model), dur(c.start, c.state === "work" ? null : c.last), (calls ?? "—") + (calls === 1 ? " step" : " steps")].join(" · ")));
    b.append(rich("span", "cc-brief", preview(h.brief)));
    if (h.result) { const r = el("span", "cc-result" + (h.status === "err" ? " err" : "")); r.append(el("span", "rl", h.status === "err" ? "Result: " : "Returned: ")); inline(r, h.result); b.append(r); }
    else if (c.state === "work" && c.activity) { const n = el("span", "cc-now"); n.append(el("span", "spin"), el("span", null, verbNow(c.activity[0])), el("code", null, c.activity[1])); b.append(n); }
    b.setAttribute("aria-label", "Open " + c.name + ": " + kindText(c) + ", " + STATE[c.state]);
    b.addEventListener("click", () => goSession(c.id));
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
    // The page first, then the bar: the bar's summary (a trace's counts, a search's matches) comes from the page.
    closeAccountMenu(); stopOpeningEndPin(); CHILDREN = null; // a redraw inside the open-at-end window ends the pin
    tick(); const page = $("#page"), r = route; rendered = r; page.style.paddingBottom = ""; clearBox(page, r); page.classList.remove("child-page");
    if (r.v === "home") { renderHome(page); renderTopbar("Home"); }
    else if (r.v === "analytics") { renderAnalytics(page); renderTopbar("Analytics", null, { analytics: true }); }
    else if (r.v === "sessions") { renderSessions(page); renderTopbar("Sessions"); }
    else if (r.v === "machines") { renderMachines(page); renderTopbar("Machines"); }
    else if (r.v === "machine") { renderMachine(page, r.id); renderTopbar(MACHINE[r.id], { label: "Machines", go: () => go({ v: "machines" }) }, { line2: machineLine(r.id) }); }
    else if (r.v === "trace") { const sum = renderTrace(page, r.turn) ?? ""; renderTopbar("Trace", { label: SESS[r.sid].name, go: () => goSession(r.sid, r.turn) }, sum ? { line2: (l2) => l2.append(lab(sum, null, 0)) } : {}); }
    else if (r.v === "session") { const s = SESS[r.id], lineage = lineageOf(r.id).slice(0, -1); renderSession(page, r.id); renderTopbar(s.name, null, { session: s, lineage, line2: sessionLine(s) }); }
    if (r.v === "session" && errOn(r.id)) markError(false);
    document.documentElement.style.setProperty("--barh", $("#topbar").offsetHeight + "px");
    const lanesKept = lanesFor && lanesFor.r === r && lanesFor.version === LIVE.version; lanesFor = null;
    syncLayoutPrefs(); syncBarLine(); renderNav(); if (!lanesKept) renderLanes(); renderDrawerAccount(); syncJump();
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
  document.addEventListener("visibilitychange", () => { if (visible() && route.v === "analytics") refreshAnalytics(); else if (!visible()) { clearTimeout(AN.timer); AN.timer = null; } });
  const nameOfSid = (A, sid) => SESS[sid]?.name ?? A.sessions[sid]?.name ?? sid;
  const harnessOfSid = (A, sid) => SESS[sid]?.harness ?? A.sessions[sid]?.harness ?? "";
  // A session older than the model's window has no page to open: its row is text.
  function sessionRow(A, sid, cls, value, onOpen) {
    const open = !!SESS[sid], b = el(open ? "button" : "div", cls);
    if (open) { b.type = "button"; b.addEventListener("click", onOpen); }
    b.append(el("span", "session-name", nameOfSid(A, sid)), harnessName(harnessOfSid(A, sid), true), el("span", "session-value", value));
    return b;
  }
  const sessionFacetValue = (s, key) => key === "repo" ? s.repo ?? "__none__" : key === "model" ? s.model ?? s.modelId ?? "Unknown model" : s[key] ?? "";
  function matchesSessionFacets(s) { return Object.keys(sessionFilters).every((key) => !sessionFilters[key] || sessionFacetValue(s, key) === sessionFilters[key]); }
  // The four filters (Repo, Machine, Harness, Model) of Analytics and Sessions: one persistent control per page. A redraw
  // (`sync`) brings its option lists and selections up to date in place. A selected value that no session has now stays
  // selected, marked "(no sessions)", until the reader changes it.
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
      for (const [key, label, allLabel, valuesOf, showValue] of FACETS) {
        const select = SemonShell.select({ label, options: [{ value: "", label: allLabel }], value: sessionFilters[key], onChange: (value) => { sessionFilters[key] = value; ctx.onChange(); } });
        bar.append(select.el); fields.push({ key, select, allLabel, valuesOf, showValue });
      }
      ctx.sync = () => {
        for (const f of fields) {
          const current = sessionFilters[f.key], values = f.valuesOf().filter((v) => v !== ""), gone = current !== "" && !values.includes(current);
          if (gone) values.push(current);
          f.select.setOptions([{ value: "", label: f.allLabel }, ...values.map((v) => ({ value: v, label: f.showValue(v) + (gone && v === current ? " (no sessions)" : "") }))]);
          f.select.setValue(current);
        }
      };
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
    const legend = el("div", "analytics-legend"); for (const [h, label] of [["claude", "Claude"], ["codex", "Codex"]]) { const item = el("span"), swatch = el("i"); swatch.style.setProperty("--h", "var(--" + h + ")"); item.append(swatch, label); legend.append(item); } panel.append(legend); return panel;
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
    for (const [h, label] of [["claude", "Claude"], ["codex", "Codex"]]) { const item = el("span"), swatch = el("i"); swatch.style.setProperty("--h", "var(--" + h + ")"); item.append(swatch, label); legend.append(item); } panel.append(legend);
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
      const measure = selected(item), track = el("span", "row-track"), bar = el("i", "row-bar"); bar.style.width = Math.max(measure ? 2 : 0, measure / max * 100) + "%"; if (groupKey === "harness") bar.style.background = item.key.startsWith("claude") ? "var(--claude)" : "var(--codex)"; track.append(bar);
      b.append(track, el("span", "row-hours" + (analyticsMeasure === "hours" ? " on" : ""), hoursText(item.ms)), el("span", "row-cost" + (analyticsMeasure === "cost" ? " on" : ""), item.unknown.length ? "—" : asMoney(item.cost))); if (item.unknown.length) b.append(el("span", "no-price", "no price for " + item.unknown.join(", ")));
      b.dataset.breakdown = groupKey; b.dataset.key = item.key;
      b.addEventListener("click", () => { if (groupKey === "repo") sessionFilters.repo = item.key; else if (groupKey === "machine") sessionFilters.machine = item.key; else { const [harness, model] = item.key.split("\u0000"); sessionFilters.harness = harness; sessionFilters.model = model; } query = ""; groupBy = "recent"; go({ v: "sessions" }); }); list.append(b); }
    if (!items.length) list.append(el("p", "empty", "No activity in this range.")); panel.append(list); return panel;
  }
  function analyticsList(A, title, items, value) {
    const panel = el("section", "analytics-panel"); panel.append(el("h2", null, title)); const list = el("div", "analytics-list"); if (!items.length) list.append(el("p", "empty", "No sessions in this range."));
    for (const item of items) { const b = sessionRow(A, item.sid, "analytics-session", value(item), () => goSession(item.sid)); const missing = item.unpriced_models ?? []; if (missing.length) b.append(el("span", "no-price", "no price for " + missing.join(", "))); list.append(b); } panel.append(list); return panel;
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
    // The breakdown's heading and its measure toggle: a persistent control (the toggle's state is the page's).
    const bdHead = slot("measure", page, () => {
      const bar = el("div", "analytics-bd-head"), title = el("div"), toggle = el("div", "analytics-measure"); title.append(el("h2", null, "Breakdown"), el("div", "panel-sub", "Agent-hours and API-equivalent cost; bars follow the toggle"));
      toggle.setAttribute("role", "group"); toggle.setAttribute("aria-label", "Breakdown bar measure");
      for (const [key, label] of [["hours", "Agent-hours"], ["cost", "API-equivalent cost"]]) { const b = el("button", null, label); b.type = "button"; b.dataset.measure = key; b.addEventListener("click", () => { if (analyticsMeasure === key) return; const top = currentScroll(); analyticsMeasure = key; render(); restoreScroll(top); }); toggle.append(b); }
      bar.append(title, toggle); return bar;
    }).el;
    for (const b of bdHead.querySelectorAll(".analytics-measure button")) b.setAttribute("aria-pressed", String(analyticsMeasure === b.dataset.measure));
    const bottom = el("div", "analytics-split");
    bottom.append(analyticsList(A, "Top sessions · busy time", A.top.busy, (x) => timeText(x.ms)), analyticsList(A, "Top sessions · waited on", A.top.waited, (x) => timeText(x.ms)), analyticsList(A, "Most expensive sessions · API-equivalent cost", A.top.cost, (x) => x.usd == null ? "—" : asMoney(x.usd)));
    put(renderAgentsChart(A), renderCostChart(A), bdHead, breakdowns, bottom); const allowance = renderCodexAllowance(A.allowance); if (allowance) put(allowance);
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
  function renderSessions(page) {
    const all = Object.values(SESS).filter(matchesSessionFacets);
    const head = el("div", "ph"); const h1 = el("h1", null, "Sessions"); head.append(h1);
    const sub = el("div", "sub"); for (const [v, l] of [[all.length, all.length === 1 ? "session" : "sessions"], [all.filter((s) => s.state === "work").length, "working"], [all.filter((s) => s.state === "wait").length, "waiting on you"]]) { const x = el("span"); x.append(el("b", null, String(v)), l); sub.append(x); }
    head.append(sub); const put = placer(page); put(head); observeTitle(h1);
    // The search field and the group-by buttons are persistent controls: the page's redraws keep them, and the field's focus, text and caret.
    const found = slot("find", page, (ctx) => {
      const fr = el("label", "find"), fi = el("input"); fi.id = "sq"; fi.type = "search"; fi.placeholder = "Search sessions"; fi.setAttribute("aria-label", "Search sessions"); fi.value = query; fr.append(icon(I.search), fi);
      fi.addEventListener("input", () => { query = fi.value.trim(); ctx.draw(); }); return fr;
    }), fr = found.el, fi = fr.querySelector("input");
    if (fi.value.trim() !== query) fi.value = query; // the sidebar's search can have changed it
    const grouped = slot("groupby", page, (ctx) => {
      const gb = el("div", "groupby"); gb.setAttribute("role", "group"); gb.setAttribute("aria-label", "Group by");
      for (const [g, label] of [["recent", "Recent"], ["project", "Project"], ["machine", "Machine"], ["harness", "Harness"]]) { const b = el("button", null, label); b.type = "button"; b.dataset.g = g; b.addEventListener("click", () => { groupBy = g; ctx.draw(); }); gb.append(b); }
      return gb;
    }), gb = grouped.el;
    const out = el("div", "sess"); out.style.display = "grid"; out.style.gap = "16px";
    const draw = () => {
      out.replaceChildren(); const lanes = all.filter((s) => sessMatch(s, query));
      let groups;
      if (groupBy === "recent") groups = [["", [...lanes].sort((a, b) => b.last - a.last)]];
      else { const key = { machine: (s) => MACHINE[s.machine], project: (s) => s.repo ?? "No repo (roles)", harness: (s) => HARNESS[s.harness] }[groupBy]; const keys = [...new Set(lanes.map(key))].sort((a, b) => a.startsWith("No repo") - b.startsWith("No repo") || a.localeCompare(b)); groups = keys.map((k) => [k, lanes.filter((s) => key(s) === k).sort((a, b) => b.last - a.last)]); }
      for (const [title, items] of groups) {
        const box = el("div"); box.style.display = "grid"; if (title) box.append(secHead(title, items.length));
        const list = el("div", "list");
        for (const s of items) { const r = el("button", "nrow"); r.type = "button"; r.dataset.id = s.id; r.append(dot(s.state), el("span", "nm", s.name), el("span", "ag", ago(s.last)), el("span", "for", [s.kind ?? HARNESS[s.harness], MACHINE[s.machine], s.repo ? where(s) : "no repo"].join(" · "))); const k = childRuns(s.id); if (k) r.append(el("span", "kids", k)); r.addEventListener("click", () => goSession(s.id)); list.append(r); }
        box.append(list); out.append(box);
      }
      if (!lanes.length) out.append(el("p", "empty", "No sessions match “" + query + "”."));
      for (const b of gb.children) b.setAttribute("aria-pressed", String(b.dataset.g === groupBy));
      renderLanes();
    };
    found.ctx.draw = grouped.ctx.draw = draw;
    put(renderFacetFilters(page, () => render()), fr, gb, out); put.done(); draw();
  }

  // ---- Drawer (phone) ---------------------------------------------------------------------------------------------------------
  const sidebar = $("#sidebar");
  function openDrawer() { if (!phone.matches) return; document.body.classList.add("drawer-open"); $("#lead-btn")?.setAttribute("aria-expanded", "true"); }
  function closeDrawer(quiet) { if (!document.body.classList.contains("drawer-open")) return; document.body.classList.remove("drawer-open"); closeAccountMenu(); const b = $("#lead-btn"); b?.setAttribute("aria-expanded", "false"); if (!quiet) b?.focus(); }
  $("#drawer-close").addEventListener("click", () => closeDrawer());
  $("#scrim").addEventListener("click", () => closeDrawer());
    document.addEventListener("keydown", (e) => { if (e.key === "Escape" && accountSheet) closeAccountMenu(); else if (e.key === "Escape" && !viewerEl) { closeDrawer(); closeAccountMenu(); } if (e.key === "/" && !/INPUT/.test(document.activeElement?.tagName ?? "")) { e.preventDefault(); openDrawer(); $("#q").focus(); } });
  let sx = null;
  sidebar.addEventListener("touchstart", (e) => { sx = e.touches[0].clientX; }, { passive: true });
  sidebar.addEventListener("touchmove", (e) => { if (sx !== null && e.touches[0].clientX - sx < -50) { sx = null; closeDrawer(); } }, { passive: true });
  // The sidebar search narrows the Recent list as you type; Enter opens the Sessions page with the same query.
  $("#q").addEventListener("input", (e) => { query = e.target.value.trim(); renderLanes(); });
  $("#q").addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); query = e.target.value.trim(); go({ v: "sessions" }); } });
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
  const LIVE = { version: null, timer: null, due: 0, busy: false, started: -Infinity, delay: 2000, ended: false, again: false, pending: false, fresh: 0, turns: new Map(), missing: new Set() };
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
    LIVE.timer = null; if (LIVE.busy || LIVE.ended || !visible()) return; LIVE.busy = true; LIVE.started = performance.now();
    let ok = false;
    fetch("/api/model?since=" + enc(LIVE.version ?? ""), { credentials: "same-origin" })
      .then((r) => r.status === 304 ? null : r.ok ? r.json() : Promise.reject(Object.assign(new Error(r.status + " " + r.statusText), { status: r.status })), (e) => Promise.reject(Object.assign(e, { status: 0 })))
      .then((m) => (m ? update(m) : null))
      .then(() => { LIVE.delay = 2000; ok = true; }, (err) => {
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
    LIVE.ended = true; clearTimeout(LIVE.timer); LIVE.timer = null; if ($(".livenote")) return;
    if (!window.dispatchEvent(new CustomEvent("semon:ended", { cancelable: true, detail: { status } }))) return;
    const n = el("p", "livenote", "Session ended: reload with the printed URL"); n.setAttribute("role", "status"); document.body.append(n);
  }
  // A 403 or a dropped connection fails the update (and backs off); anything else skips that one transcript.
  const soft = (p) => p.catch((e) => { if (e?.status === 403 || e?.status === 0) throw e; });
  // The transcript on screen: a session page's own. Any other loaded transcript is dropped from TX (the last few opened are kept in
  // TXCACHE, and brought up to date when opened again). A child run's card is drawn from the model, so its transcript is not loaded.
  const viewed = () => { const v = new Set(); if (route.v === "session") v.add(route.id); return v; };
  // What a child card shows, so an update knows which cards changed: the run's name, state, kind, model, steps and current call.
  const cardKeys = () => new Map(H.filter((h) => h.kind === "spawn" && SESS[h.to]).map((h) => { const c = SESS[h.to]; return [h.id, [c.name, c.state, c.kind, c.model, countOf(c, "calls"), c.activity?.join("|"), h.status, h.result].join("\u0001")]; }));
  function update(m) {
    const oldH = new Map(H.map((h) => [h.id, handKey(h)])), oldT = LIVE.turns, oldCards = cardKeys(), names = new Map(Object.values(SESS).map((x) => [x.id, x.name]));
    adopt(m); remember(m);
    const changedH = new Set(H.filter((h) => oldH.get(h.id) !== handKey(h)).map((h) => h.id));
    const newCards = cardKeys(), changedCards = new Set([...newCards].filter(([id, k]) => oldCards.has(id) && oldCards.get(id) !== k).map(([id]) => id));
    const view = viewed(), grown = new Set(), cuts = new Map();
    let full = Object.values(SESS).some((x) => names.has(x.id) && names.get(x.id) !== x.name); // a new name shows in every turn
    for (const sid of Object.keys(TX)) { if (view.has(sid) && SESS[sid]) spread(sid); else { delete TX[sid]; delete TXM[sid]; } }
    // Only transcripts whose mark in the model moved are asked for, one at a time.
    let chain = Promise.resolve();
    for (const sid of view) if (TX[sid] && TXM[sid].tok != null && TOK[sid] != null && shrank(TXM[sid].tok, TOK[sid])) chain = chain.then(() => soft(reload(sid).then(() => { grown.add(sid); full = true; })));
    else if (TX[sid] && TXM[sid].to >= TXM[sid].total && TXM[sid].tok !== TOK[sid]) chain = chain.then(() => soft(tail(sid).then((r) => { grown.add(sid); if (r.cut != null) cuts.set(sid, r.cut); if (r.reload) full = true; })));
    return chain.then(() => { LIVE.version = m.version; refresh(full ? null : dirtyTurns(cuts, grown, changedH, oldT, changedCards)); if (route.v === "analytics") refreshAnalytics(); const e = errorsLive(); return e && soft(e); });
  }
  // The turns of the session page an update changed: those holding entries its tail brought (from the cut on), those
  // whose record or handoffs changed, and those holding the card of a child run that changed. Null: draw them all.
  function dirtyTurns(cuts, grown, changedH, oldT, changedCards) {
    if (route.v !== "session" || !TX[route.id]) return null;
    const sid = route.id, dirty = new Set(), owner = new Map((TURNS[sid] ?? []).flatMap((t) => t.entries.map((e) => [e, t.id])));
    if (cuts.has(sid)) for (const e of TX[sid].slice(cuts.get(sid))) { if (isGap(e)) return null; if (owner.has(e)) dirty.add(owner.get(e)); }
    for (const t of TURNS[sid] ?? []) {
      if (oldT.get(t.id) !== LIVE.turns.get(t.id) || [t.start, ...t.sent].some((h) => h && changedH.has(h.id)) || t.entries.some((e) => e.k === "h" && changedH.has(e.id))) dirty.add(t.id);
    }
    for (const h of H) if (h.kind === "spawn" && h.from === sid && changedCards.has(h.id) && HOLDS.get(h.id)) dirty.add(HOLDS.get(h.id).id);
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
  // Draws the new model on the screen shown, unless a navigation is still loading (it draws when done), the sheet is open
  // (it draws when the sheet closes), or what the screen shows left the model (it stays as it was).
  function refresh(dirty) {
    if (accountOpen && !$(".account-popover")?.isConnected) closeAccountMenu(); // a menu some redraw took away is closed
    if (viewerEl || accountOpen) { LIVE.pending = true; return; } // drawn whole when the sheet or the account menu closes
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
  const FOCUSABLE = "button, input, [tabindex]";
  const identOf = (n) => { const d = n.dataset, keys = [d.e, d.turn, d.h, d.id, d.m, d.sid, d.go, d.g];
    return [n.classList[0], ...keys, keys.some((x) => x != null) ? "" : n.firstChild?.nodeType === 3 ? n.firstChild.data : ""].map((x) => x ?? "").join("|"); };
  // Each anchor candidate on the page, in order, with its identity made unique by how many came before it.
  function anchors(fn) { const seen = new Map(); for (const n of $("#page").querySelectorAll(ANCHORS)) { const id = identOf(n), k = seen.get(id) ?? 0; seen.set(id, k + 1); if (fn(n, id + "#" + k)) return; } }
  const opener = (n) => n.classList.contains("step") ? n.querySelector(":scope > button") : n.classList.contains("tgroup") ? n.querySelector(":scope > .tsum") : null;
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
      if (opener(n)?.getAttribute("aria-expanded") === "true" || (n.classList.contains("event") && n.querySelector(":scope > .ev-text.open"))) st.open.add(n.dataset.e);
    }
    for (const n of $("#page").querySelectorAll(".hop")) if (n.querySelector(".brief.open")) st.open.add("hop:" + identOf(n));
    const a = document.activeElement;
    if (a && a !== document.body && !a.closest("dialog")) {
      const host = a.id ? null : a.closest(HOSTS), sel = host && host !== a ? a.tagName.toLowerCase() + [...a.classList].map((c) => "." + CSS.escape(c)).join("") : null;
      // By id, else by its keyed block and place in it, else by its label, else by its place among the page's controls.
      st.focus = { id: a.id || null, host: host ? identOf(host) : null, sel, i: sel ? [...host.querySelectorAll(sel)].indexOf(a) : 0, range: null,
        label: a.getAttribute("aria-label"), at: [...$("#page").querySelectorAll(FOCUSABLE)].indexOf(a), of: $("#page").querySelectorAll(FOCUSABLE).length };
      try { if (typeof a.selectionStart === "number") st.focus.range = [a.selectionStart, a.selectionEnd]; } catch {}
    }
    return st;
  }
  function restore(st, pin) {
    const all = (sel) => [...$("#page").querySelectorAll(sel)], r0 = rendered;
    // A new card or brief measures its "Show more" now, as its ResizeObserver would a frame later, so nothing moves after the
    // scroll position is set.
    const clamp = (br, more) => { if (!br || !more || !br.clientHeight) return; const x = br.scrollHeight > br.clientHeight + 1; more.hidden = !x; br.classList.toggle("clipped", x); };
    for (const c of all(".event")) clamp(c.querySelector(":scope > .ev-text"), c.querySelector(":scope > .ev-more"));
    for (const br of all(".hop .body > .brief:not(.open)")) clamp(br, br.parentElement.querySelector(":scope > .more"));
    // Groups first (a new one opens if it holds an open step), then steps and events.
    for (const n of all(".tgroup[data-e]")) {
      const want = st.groups.has(n.dataset.e) ? st.open.has(n.dataset.e) : [...n.querySelectorAll(".step[data-e]")].some((x) => st.open.has(x.dataset.e));
      if (want && opener(n).getAttribute("aria-expanded") === "false") opener(n).click();
    }
    for (const n of all(".step[data-e]")) if (st.open.has(n.dataset.e) && opener(n)?.getAttribute("aria-expanded") === "false") opener(n).click();
    // An event opened before its size was measured: its "Show less" is shown by hand.
    for (const n of all(".event[data-e]")) if (st.open.has(n.dataset.e) && !n.querySelector(":scope > .ev-text.open")) { const m = n.querySelector(":scope > .ev-more"); m.hidden = false; m.click(); }
    for (const n of all(".hop")) if (st.open.has("hop:" + identOf(n)) && !n.querySelector(".brief.open")) { const m = n.querySelector(".body > .more"); m.hidden = false; m.click(); }
    if (st.drawer) { document.body.classList.add("drawer-open"); $("#lead-btn")?.setAttribute("aria-expanded", "true"); }
    if (st.focus) {
      let n = st.focus.id ? document.getElementById(st.focus.id) : null;
      if (!n && st.focus.host) { const host = [...document.querySelectorAll(HOSTS)].find((x) => identOf(x) === st.focus.host); n = host && st.focus.sel ? host.querySelectorAll(st.focus.sel)[st.focus.i] : host; }
      if (!n && st.focus.label) n = [...document.querySelectorAll("#page [aria-label], #topbar [aria-label]")].find((x) => x.getAttribute("aria-label") === st.focus.label);
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
    const keys = () => new Set([...$("#page").querySelectorAll(".turns :is(.msg, .bubble, .step, .event, .child-card, .thought, .think-pending)[data-e]")].map((n) => n.dataset.e));
    const before = keys();
    // Only the changed turns are drawn again, unless the turns shown no longer match the index or nothing was shown.
    const whole = !dirty || box.querySelector(":scope > p.empty") || [...box.querySelectorAll(":scope > .turn")].some((b) => !TURN.has(b.dataset.turn));
    if (whole) morph(box, transcript(route.id).querySelector(".turns"));
    else if (dirty.size) morphTurns(box, transcript(route.id, { only: dirty }).querySelector(".turns"), dirty);
    const h1 = $("#page .ph h1"); if (h1) h1.textContent = s.name;
    const t = $("#topbar .t"); if (t) { t.textContent = s.name; t.dataset.tip = s.name; }
    const lead = $("#topbar .l1-state"); if (lead) lead.replaceWith(stateLead(s));
    const l2 = $("#topbar .meta-line"); if (l2) { l2.replaceChildren(); sessionLine(s)(l2); requestAnimationFrame(() => { if (l2.isConnected) fitMeta(l2); }); }
    // Find's count and the Failed steps chip's count follow the page; a chip that appears or goes redraws the bar.
    const fc = $("#topbar .fcount"); if (fc) fc.textContent = matchText(matchCount());
    if (findOpen && !errOn(s.id)) { const chip = $('#topbar .chip[data-filter="failures"]'), failed = countOf(s, "errors") ?? 0; if (!!chip !== !!failed) keepFocus(drawSessionBar); else if (chip) { const nn = chip.querySelector(".n"); if (nn) nn.textContent = String(failed); } }
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

  // Jump to the latest: centred at the transcript column's foot, sticky, with the count of what arrived while the reader was away.
  const jumpWrap = el("div", "jump-wrap"), jumpButton = el("button", "jump"); jumpButton.type = "button"; jumpButton.id = "jump-bottom"; jumpButton.setAttribute("aria-label", "Jump to bottom of transcript"); jumpWrap.hidden = true; jumpWrap.append(jumpButton);
  function scrollMetrics() {
    if (phone.matches) return { top: window.scrollY, height: document.documentElement.scrollHeight, viewport: window.innerHeight, gap: Math.max(0, document.documentElement.scrollHeight - window.innerHeight - window.scrollY) };
    const m = $("#main"); return { top: m.scrollTop, height: m.scrollHeight, viewport: m.clientHeight, gap: Math.max(0, m.scrollHeight - m.clientHeight - m.scrollTop) };
  }
  function scrollToEnd(behavior = "smooth") { if (phone.matches) window.scrollTo({ top: document.documentElement.scrollHeight, behavior }); else { const m = $("#main"); m.scrollTo({ top: m.scrollHeight, behavior }); } }
  // The button is rebuilt only when what it shows changes (hidden or not, and the new-entry count), not on every scroll.
  let jumpKey = "";
  function syncJump() {
    if (route.v !== "session") { LIVE.fresh = 0; jumpWrap.hidden = true; jumpKey = ""; return; }
    const { gap } = scrollMetrics(); if (gap <= 80) LIVE.fresh = 0;
    const key = (gap <= 80) + "|" + LIVE.fresh; if (key === jumpKey && jumpWrap.isConnected) return;
    jumpKey = key; jumpWrap.hidden = gap <= 80; jumpButton.replaceChildren();
    if (LIVE.fresh) jumpButton.append(el("span", "new-count", LIVE.fresh + " new"));
    jumpButton.append(icon(I.down));
    jumpButton.setAttribute("aria-label", LIVE.fresh ? "Jump to bottom; " + LIVE.fresh + " new entries" : "Jump to bottom of transcript");
  }
  function clearNewEntries() { LIVE.fresh = 0; jumpWrap.hidden = true; jumpKey = ""; }
  jumpButton.addEventListener("click", () => scrollToEnd("smooth"));
  window.addEventListener("scroll", syncJump, { passive: true });
  $("#main").addEventListener("scroll", syncJump, { passive: true });
  const cancelOpeningEndPin = () => { if (openingEndUntil) stopOpeningEndPin(); };
  window.addEventListener("wheel", cancelOpeningEndPin, { passive: true });
  window.addEventListener("touchmove", cancelOpeningEndPin, { passive: true });
  window.addEventListener("pointerdown", cancelOpeningEndPin, { passive: true }); // a press anywhere, a scrollbar drag included
  document.addEventListener("keydown", (e) => {
    if (!e.defaultPrevented && !e.target.closest?.("input, textarea, select, [contenteditable='true']") && ["ArrowUp", "ArrowDown", "PageUp", "PageDown", "Home", "End", " "].includes(e.key)) cancelOpeningEndPin();
  });

  // Every second: a running step's elapsed time, from its session's activity[3] (the call's start), and a running row's age.
  // A clock that stands still (the checks pin it) changes nothing.
  const running = (ms) => { const x = Math.max(0, Math.floor(ms / 1000)); return x < 60 ? x + "s" : Math.floor(x / 60) + "m " + (x % 60) + "s"; };
  function ticker() {
    if (!visible() || Date.now() === fetchedAt) return;
    tick();
    const last = new Map(); for (const n of document.querySelectorAll(".step.live[data-live]")) last.set(n.dataset.live, n);
    for (const [sid, n] of last) {
      const a = SESS[sid]?.activity; if (!a || a[3] == null) continue; const text = running(NOW - a[3]), sd = n.querySelector(".sd");
      if (sd && sd.textContent !== text) sd.textContent = text;
      const tl = n.closest(".tgroup")?.querySelector(":scope > .tsum > .tl"); if (tl) tl.textContent = "· running " + text;
    }
    for (const n of document.querySelectorAll(".nrow[data-id] .act .el")) { const a = SESS[n.closest(".nrow").dataset.id]?.activity; if (a) n.textContent = Math.max(0, a[2]) + "s"; }
  }

  boot();
})();
