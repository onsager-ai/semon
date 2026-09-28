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
  let PRICING = {};
  let ACCOUNT = null;
  let NAV_MACHINES = null;
  const HARNESS = { claude: "Claude Code", codex: "Codex" };
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
  const el = (tag, cls, text) => { const n = document.createElement(tag); if (cls) n.className = cls; if (text != null) n.textContent = text; return n; };
  const SVGNS = "http://www.w3.org/2000/svg";
  function icon(d, cls) { const s = document.createElementNS(SVGNS, "svg"); s.setAttribute("viewBox", "0 0 24 24"); s.setAttribute("aria-hidden", "true"); s.setAttribute("fill", "none"); s.setAttribute("stroke", "currentColor"); s.setAttribute("stroke-width", "1.8"); s.setAttribute("stroke-linecap", "round"); s.setAttribute("stroke-linejoin", "round"); if (cls) s.setAttribute("class", cls); const p = document.createElementNS(SVGNS, "path"); p.setAttribute("d", d); s.append(p); return s; }
  const I = {
    menu: "M4 7h16M4 12h16M4 17h16", more: "M5 12h.01M12 12h.01M19 12h.01", back: "M15 6l-6 6 6 6", chev: "M9 6l6 6-6 6", search: "M11 18a7 7 0 1 1 0-14 7 7 0 0 1 0 14zM20 20l-4-4", filter: "M4 6h16M7 12h10M10 18h4",
    home: "M4 11l8-7 8 7M6 9.5V20h12V9.5M10 20v-5h4v5", inbox: "M4 13l2.5-8h11L20 13v6H4zM4 13h5l1 2h4l1-2h5", now: "M3 12h4l2.5-6 5 12 2.5-6h4", trace: "M6 4v10a4 4 0 0 0 4 4h8M6 10h12M15 7l3 3-3 3M15 15l3 3-3 3", sessions: "M9 6h11M9 12h11M9 18h11M4.5 6h.01M4.5 12h.01M4.5 18h.01",
    run: "M4 17l5-5-5-5M12 19h8", stack: "M12 3l9 5-9 5-9-5zM3 13l9 5 9-5", read: "M6 3h8l4 4v14H6zM14 3v4h4", edit: "M4 20h4L19 9l-4-4L4 16zM13 7l4 4", find: "M11 18a7 7 0 1 1 0-14 7 7 0 0 1 0 14zM20 20l-4-4",
    machine: "M3 5h18v11H3zM8 20h8M12 16v4", repo: "M6 3v12M6 15a3 3 0 1 0 0 6 3 3 0 0 0 0-6zM18 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM18 9c0 6-12 3-12 6", role: "M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM4 21c1-4 4-6 8-6s7 2 8 6",
    out: "M7 17L17 7M9 7h8v8", in: "M17 7L7 17M15 17H7V9", move: "M4 8h13l-3-3M20 16H7l3 3", ask: "M5 18l-1 3 3-1 11-11-2-2zM14 6l4 4", you: "M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM4 21c1-4 4-6 8-6s7 2 8 6", q: "M9 9a3 3 0 1 1 4 2.8c-.7.3-1 .9-1 1.7V14M12 18h.01", check: "M5 12l4 4 10-10", qc: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM9.6 9.4a2.5 2.5 0 1 1 3.4 2.4c-.6.3-1 .8-1 1.5v.4M12 17h.01", decide: "M12 21v-6M12 15L6 9M12 15l6-6M6 9V4M18 9V4M4 6l2-2 2 2M16 6l2-2 2 2", result: "M14 3H6v18h12V7zM14 3v4h4M9 12h6M9 16h6", done: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM8 12.5l2.7 2.7L16 9.8", x: "M6 6l12 12M18 6L6 18", expand: "M15 3h6v6M9 21H3v-6M21 3l-7 7M3 21l7-7", copy: "M9 9h11v11H9zM5 15H4V4h11v1", ext: "M14 4h6v6M20 4l-9 9M18 14v6H4V6h6",
    down: "M12 4v15M5 12l7 7 7-7", branch: "M6 3v12M6 15a3 3 0 1 0 0 6 3 3 0 0 0 0-6zM18 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM18 9c0 6-12 3-12 6",
    wrench: "M14.5 6.5a5 5 0 0 0-6.9 6.9l-4.8 4.8a2 2 0 0 0 2.8 2.8l4.8-4.8a5 5 0 0 0 6.9-6.9l-3 3-2.8-2.8z", wide: "M4 9V4h5M15 4h5v5M20 15v5h-5M9 20H4v-5", sidebar: "M4 5h16v14H4zM9 5v14", tokens: "M5 5h14M12 5v14M9 19h6", chart: "M4 19V5M4 19h17M8 15l3-4 3 2 5-7", coin: "M12 3v18M17 7.5C17 6.1 14.8 5 12 5S7 6.1 7 7.5 9.2 10 12 10s5 1.1 5 2.5-2.2 2.5-5 2.5-5-1.1-5-2.5", relay: "M4 7h13l-3-3M20 17H7l3 3M17 4l-3 3 3 3M7 14l3 3-3 3",
  };
  const clock = (t) => { const d = new Date(t), n = new Date(NOW); const hm = String(d.getHours()).padStart(2, "0") + ":" + String(d.getMinutes()).padStart(2, "0"); return d.toDateString() === n.toDateString() ? hm : d.toLocaleDateString(undefined, { weekday: "short" }) + " " + hm; };
  const ago = (t) => { const d = Math.floor((NOW - t) / 60000); return d < 1 ? "now" : d < 60 ? d + "m" : d < 2880 ? Math.floor(d / 60) + "h" : Math.floor(d / 1440) + "d"; };
  const dur = (a, b) => { const d = Math.max(0, Math.floor(((b ?? NOW) - a) / 60000)); return d >= 1440 ? Math.floor(d / 1440) + "d " + Math.floor((d % 1440) / 60) + "h" : d >= 60 ? Math.floor(d / 60) + "h " + (d % 60) + "m" : d + "m"; };
  const tok = (m) => m >= 1 ? m.toFixed(1) + "M" : Math.round(m * 1000) + "k";
  const dot = (st) => { const d = el("span", "dot " + st); d.title = STATE[st] ?? st; d.setAttribute("role", "img"); d.setAttribute("aria-label", d.title); return d; };
  const STATE = { work: "Working", wait: "Needs you", idle: "Idle", done: "Done", err: "Failed", new: "New result", read: "Read result" };
  const nameOf = (id) => id === "you" ? "You" : SESS[id].name;
  const hcls = (id) => id === "you" ? "h-you" : "h-" + SESS[id].harness;
  const where = (s) => s.repo ? s.repo + (s.branch && s.branch !== "main" && s.branch !== s.name ? " · " + s.branch : "") : "No repo";
  const hostOf = (s) => s.host ?? MACHINE[s.machine] ?? s.machine ?? "Unknown machine";
  const shortHost = (s) => { const h = hostOf(s).split(".")[0]; return h.length > 14 ? h.slice(0, 14) + "…" : h; };
  const branchOf = (s) => s.worktree ?? s.branch ?? "No branch";
  const shortModel = (model) => String(model ?? "Unknown model").replace(/^gpt-\d+-/i, "").replace(/^claude-/i, "").replace(/^(opus|sonnet|haiku)-(\d+)-(\d+)$/i, "$1 $2.$3").replace(/^(opus|sonnet|haiku)-(\d+)\.(\d+)$/i, "$1 $2.$3").replace(/^(opus|sonnet|haiku)-(\d+)$/i, "$1 $2");
  const harnessMark = (harness) => { const mark = el("span", "hmark h-" + harness, harness === "claude" ? "✳" : "⌘"); mark.setAttribute("aria-hidden", "true"); mark.title = HARNESS[harness] ?? harness; return mark; };
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
    if (h.kind === "move") return [I.move, [el("span", "verb", "Semon moved "), W(h.to), el("span", "verb", " from " + MACHINE[h.fromMachine] + " to " + MACHINE[h.toMachine])]];
    const what = { question: " asked you", result: " sent you a result", decision: " needs your decision" }[h.ask];
    return [h.ask === "question" ? I.qc : h.ask === "decision" ? I.decide : I.result, [W(h.from), el("span", "verb", what)]];
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
    if (!hasTurn(t) || !t.end) return null;
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
  const ANALYTICS_COUNTS = {}; // aggregate tool calls and errors from /api/tx, binned at each session's start
  let serverNow = 0, fetchedAt = 0;
  let TOK = {}; // per session: its transcript's growth mark in the model; a loaded transcript is tailed only when it moved
  const enc = encodeURIComponent;
  const safePath = (href) => typeof href === "string" && href.startsWith("/") && !href.startsWith("//") && !href.includes("\\") && !/[\u0000-\u001f\u007f-\u009f]/.test(href) && href.length <= 512;
  const textField = (value, min, max) => typeof value === "string" && [...value].length >= min && [...value].length <= max && !/[\u0000-\u001f\u007f-\u009f]/.test(value);
  function accountOf(value) {
    if (!value || !textField(value.name, 1, 80) || !value.name.trim() || !textField(value.login, 0, 80) || !textField(value.initials, 1, 3) || !value.initials.trim()) return null;
    if (value.avatar_href != null && !safePath(value.avatar_href)) return null;
    if (!Array.isArray(value.workspaces) || value.workspaces.length > 50 || !Array.isArray(value.links) || value.links.length > 12) return null;
    if (value.workspaces.some((w) => !w || !textField(w.name, 1, 80) || !w.name.trim() || !textField(w.role, 0, 80) || typeof w.current !== "boolean" || !safePath(w.switch_href))) return null;
    if (value.links.some((a) => !a || !textField(a.label, 1, 80) || !a.label.trim() || !safePath(a.href) || (a.method !== "get" && a.method !== "post") || typeof a.danger !== "boolean")) return null;
    return value;
  }
  // An error carries the HTTP status (0: no response), so live polling can tell a 403 from a dropped connection.
  const api = (path) => fetch(path, { credentials: "same-origin" }).then((r) => { if (!r.ok) throw Object.assign(new Error(r.status + " " + r.statusText), { status: r.status }); return r.json(); }, (e) => { throw Object.assign(e, { status: 0 }); });
  // NOW follows the client clock from the model's `now`, so every "ago" keeps moving; a running tool's age follows NOW.
  function tick() {
    NOW = serverNow + (Date.now() - fetchedAt);
    for (const s of Object.values(SESS)) if (s.activity && s.activity[3] != null) s.activity[2] = Math.floor((NOW - s.activity[3]) / 1000);
  }
  function adopt(m) {
    serverNow = m.now; fetchedAt = Date.now(); TOK = m.tx ?? {}; PRICING = m.pricing?.models ?? {};
    for (const k of Object.keys(MACHINE)) { delete MACHINE[k]; delete MACHINE_UP[k]; delete MACHINE_LAST[k]; }
    // Several machines come as `machines`; one comes as `machine` alone.
    for (const x of m.machines ?? [m.machine]) { MACHINE[x.id] = x.name; MACHINE_UP[x.id] = x.up; if (x.last != null) MACHINE_LAST[x.id] = x.last; }
    ADMIN = m.admin && safePath(m.admin.href) ? m.admin : null;
    ACCOUNT = accountOf(m.account);
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
  function fetchTx(sid, q, where) {
    const tok = TOK[sid];
    return api("/api/tx?sid=" + enc(sid) + (q ? "&" + q : "")).then((p) => {
      const es = p.entries.map((e) => txEntry({ ...e, sid })), m = TXM[sid];
      if (where === "before" && m) { TX[sid] = es.concat(TX[sid]); m.from = p.from; }
      else if (where === "after" && m) { TX[sid] = TX[sid].concat(es); m.to = p.to; }
      else { TX[sid] = es; TXM[sid] = { from: p.from, to: p.to }; }
      Object.assign(TXM[sid], { total: p.total, calls: p.calls, errors: p.errors }); if (where !== "before" && p.to >= p.total) TXM[sid].tok = tok; spread(sid);
    });
  }
  // A spawn's child work opens inline under its card: load the turn each brief started, when its session isn't loaded.
  function kids(sid) {
    const jobs = [];
    for (const e of TX[sid] ?? []) {
      const h = e.k === "h" ? HID.get(e.id) : null, c = h && h.kind === "spawn" && h.from === sid ? STARTS.get(h.id) : null;
      if (c && !TX[c.sid]) jobs.push(fetchTx(c.sid, "turn=" + enc(c.id)));
    }
    return jobs.length ? Promise.all(jobs) : null;
  }
  // What a route needs before it can draw: a session's page (the one holding a deep-linked turn), and its child work.
  function load(r) {
    if (r.v === "analytics") return loadAnalyticsCounts();
    if (r.v !== "session" || !SESS[r.id]) return null;
    const t = r.turn ? TURN.get(r.turn) : null, deep = t && t.sid === r.id && !t.entries.length;
    if (TX[r.id] && !deep) return kids(r.id);
    return fetchTx(r.id, deep ? "turn=" + enc(t.id) : "").then(() => kids(r.id));
  }
  function loadAnalyticsCounts() {
    const ids = Object.keys(SESS), live = new Set(ids);
    for (const id of Object.keys(ANALYTICS_COUNTS)) if (!live.has(id)) delete ANALYTICS_COUNTS[id];
    return ids.filter((id) => ANALYTICS_COUNTS[id]?.mark !== TOK[id]).reduce((chain, id) => chain.then(() =>
      api("/api/tx?sid=" + enc(id)).then((page) => { ANALYTICS_COUNTS[id] = { calls: page.calls ?? 0, errors: page.errors ?? 0, mark: TOK[id] }; })
    ), Promise.resolve());
  }
  // "Load earlier" at the top of a transcript, and "Load later" at its end when a deep link loaded a middle page.
  function pager(sid, where, label) {
    const w = el("div", "list"), b = el("button", "more", label); b.type = "button"; w.append(b);
    b.addEventListener("click", () => {
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
      for (const [k, r] of rs) { f[k] = k === "diff" ? r.diff : r.text; if (k === "diff" && r.changes) f.changes = r.changes; if (r.truncated) f.fullCut.push(k); }
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
    if (p[0] === "s" && SESS[p[2]]) return turn ? { v: "session", id: p[2], turn } : { v: "session", id: p[2] };
    if (p[0] === "trace" && SESS[p[2]] && p[3]) return { v: "trace", sid: p[2], turn: p[3] };
    return { v: "home" };
  }
  function boot() {
    api("/api/model").then((m) => {
      adopt(m); route = routeOf(location); LIVE.version = m.version; remember(m);
      if (route.v === "machines" && NAV_MACHINES) { location.assign(NAV_MACHINES); return; }
      try { history.replaceState({ ...route, scrollTop: 0 }, "", urlOf(route)); } catch {}
      const done = () => {
        render();
        if (route.v === "session" && route.turn) revealTurn(route.turn, true);
        else if (route.v === "session") { openSessionAtEnd(); syncJump(); }
        else quietTop();
        schedule(2000); setInterval(ticker, 1000);
      };
      const p = load(route); if (p) p.then(done, done); else done();
    }, (err) => { $("#page").replaceChildren(el("p", "empty", "Couldn't load the sessions: " + err.message)); });
  }

  // ---- State & navigation ---------------------------------------------------------------
  const phone = window.matchMedia("(max-width: 760px)");
  let wideMode = false, railMode = false, treePrefs = {};
  try { wideMode = localStorage.getItem("semon.wide") === "1"; } catch {}
  try { railMode = localStorage.getItem("semon.rail") === "1"; } catch {}
  try { const saved = JSON.parse(localStorage.getItem("semon.tree") ?? "{}"); if (saved && typeof saved === "object" && !Array.isArray(saved)) treePrefs = saved; } catch {}
  const app = $(".app");
  const syncLayoutPrefs = () => { app.classList.toggle("rail", railMode && !phone.matches); $("#page").classList.toggle("wide-mode", wideMode && !phone.matches); };
  function setWideMode(on) { wideMode = on; try { localStorage.setItem("semon.wide", on ? "1" : "0"); } catch {} syncLayoutPrefs(); $(".wide-toggle")?.setAttribute("aria-pressed", String(on)); }
  function setRailMode(on) { railMode = on; try { localStorage.setItem("semon.rail", on ? "1" : "0"); } catch {} syncLayoutPrefs(); renderLanes(); const b = $("#rail-toggle"); b?.setAttribute("aria-expanded", String(!on)); b?.setAttribute("aria-label", on ? "Expand sidebar" : "Collapse sidebar"); b?.setAttribute("title", on ? "Expand sidebar" : "Collapse sidebar"); }
  function saveTreePref(id, open) {
    treePrefs[id] = { open, at: Date.now() };
    treePrefs = Object.fromEntries(Object.entries(treePrefs).sort((a, b) => (b[1]?.at ?? 0) - (a[1]?.at ?? 0)).slice(0, 500));
    try { localStorage.setItem("semon.tree", JSON.stringify(treePrefs)); } catch {}
  }
  const railToggle = $("#rail-toggle"); railToggle.append(icon(I.sidebar)); railToggle.setAttribute("aria-expanded", String(!railMode)); railToggle.setAttribute("title", railMode ? "Expand sidebar" : "Collapse sidebar"); railToggle.setAttribute("aria-label", railMode ? "Expand sidebar" : "Collapse sidebar"); railToggle.addEventListener("click", () => setRailMode(!railMode)); syncLayoutPrefs();
  let route = { v: "home" }; let groupBy = "recent"; let query = ""; let analyticsRange = 7, analyticsMeasure = "hours";
  const sessionFilters = { repo: "", machine: "", harness: "", model: "" };
  let pendingSessionOpen = null, pendingFlashHandoff = null;
  let accountOpen = false;
  try { history.scrollRestoration = "manual"; } catch {}
  let show = { messages: true, tools: true, thinking: true }; let find = ""; let findOpen = false; let filterOpen = false;
  const currentScroll = () => phone.matches ? window.scrollY : $("#main").scrollTop;
  const restoreScroll = (top) => { if (phone.matches) window.scrollTo(0, top); else $("#main").scrollTop = top; };
  const saveHistoryScroll = () => { try { if (history.state?.v) history.replaceState({ ...history.state, scrollTop: currentScroll() }, ""); } catch {} };
  let scrollSaveFrame = false;
  const queueScrollSave = () => { if (scrollSaveFrame) return; scrollSaveFrame = true; requestAnimationFrame(() => { scrollSaveFrame = false; saveHistoryScroll(); }); };
  window.addEventListener("scroll", queueScrollSave, { passive: true });
  $("#main").addEventListener("scroll", queueScrollSave, { passive: true });
  const quietTop = () => { if (phone.matches) window.scrollTo(0, 0); else $("#main").scrollTop = 0; };
  const openSessionAtEnd = () => { if (phone.matches) window.scrollTo(0, document.documentElement.scrollHeight); else { const m = $("#main"); m.scrollTop = m.scrollHeight; } saveHistoryScroll(); };
  function go(r, fromHistory) {
    if (r.v === "timeline") { r = { ...r, v: "analytics" }; try { history.replaceState({ ...r, scrollTop: r.scrollTop ?? currentScroll() }, "", urlOf(r)); } catch {} }
    if (r.v === "machines" && NAV_MACHINES) { location.assign(NAV_MACHINES); return; }
    if (!fromHistory) saveHistoryScroll();
    closeAccountMenu();
    route = r; find = ""; findOpen = false; filterOpen = false; closeDrawer(true); $(".session-menu")?.remove(); clearNewEntries();
    if (!fromHistory) { const state = { ...r }; delete state.scrollTop; try { history.pushState(state, "", urlOf(r)); } catch {} }
    const done = () => {
      if (route !== r) return;
      render();
      if (fromHistory && Number.isFinite(r.scrollTop)) restoreScroll(r.scrollTop);
      else if (r.v === "session" && r.turn) revealTurn(r.turn, !fromHistory);
      else if (r.v === "session") openSessionAtEnd();
      else quietTop();
      syncJump();
      if (pendingFlashHandoff && r.v === "session" && HID.get(pendingFlashHandoff)?.from === r.id) {
        const id = pendingFlashHandoff; pendingFlashHandoff = null;
        requestAnimationFrame(() => { const card = [...document.querySelectorAll(".hcard")].find((x) => x.dataset.h === id); if (!card) return; card.scrollIntoView({ behavior: matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth", block: "center" }); card.classList.add("flash"); setTimeout(() => card.classList.remove("flash"), 1500); });
      }
    };
    const p = load(r); if (p) p.then(done, done); else done();
  }
  window.addEventListener("popstate", (e) => {
    if (skipPop) { skipPop = false; if (pendingSessionOpen) { const id = pendingSessionOpen; pendingSessionOpen = null; goSession(id); } return; } // close a sheet before opening its session
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
      const links = el("section", "account-section account-links");
      for (const link of ACCOUNT.links) {
        if (!safePath(link.href)) continue;
        const row = el(link.method === "post" ? "button" : "a", "account-menu-row" + (link.danger ? " danger" : ""), link.label);
        row.setAttribute("role", "menuitem");
        if (link.method === "post") {
          const form = el("form", "account-menu-form account-link-form"); form.setAttribute("method", "post"); form.setAttribute("action", link.href);
          row.type = "submit"; form.append(row); links.append(form);
        } else {
          row.setAttribute("href", link.href); links.append(row);
        }
      }
      menu.append(links);
    }
    return menu;
  }
  function closeAccountMenu() {
    document.querySelectorAll(".account-popover").forEach((menu) => menu.remove());
    document.querySelectorAll(".account-trigger").forEach((button) => button.setAttribute("aria-expanded", "false"));
    accountOpen = false;
  }
  function toggleAccountMenu(widget, trigger, compact) {
    if (accountOpen) { closeAccountMenu(); return; }
    closeAccountMenu(); closeFilter(); $(".session-menu")?.remove(); $("#more-btn")?.setAttribute("aria-expanded", "false");
    const menu = accountPopover();
    if (compact) widget.insertBefore(menu, trigger); else widget.append(menu);
    accountOpen = true; trigger.setAttribute("aria-expanded", "true");
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
    item("analytics", "Analytics", I.chart);
    item("sessions", "Sessions", I.sessions, Object.keys(SESS).length);
    item("machines", "Machines", I.machine, Object.keys(MACHINE).filter((m) => !MACHINE_UP[m]).length, true);
  }
  // Sessions match by name, repo, branch, machine, harness and the messages that started their turns.
  const sessMatch = (s, q) => !q || [s.name, s.repo, s.branch, MACHINE[s.machine], s.movedFrom ? MACHINE[s.movedFrom] : "", HARNESS[s.harness], s.role ? "role no repo" : "", ...(TURNS[s.id] ?? []).map((t) => t.start?.brief ?? t.u?.text ?? "")].join(" ").toLowerCase().includes(q.toLowerCase());
  const sessionChildren = () => {
    const children = new Map();
    for (const s of Object.values(SESS)) { const parent = parentOf(s.id); if (parent && SESS[parent]) { if (!children.has(parent)) children.set(parent, []); children.get(parent).push(s); } }
    for (const xs of children.values()) xs.sort((a, b) => b.last - a.last);
    return children;
  };
  const childSessions = (sid) => sessionChildren().get(sid) ?? [];
  const descendantsOf = (sid, children, out = [], seen = new Set([sid])) => {
    for (const child of children.get(sid) ?? []) if (!seen.has(child.id)) { seen.add(child.id); out.push(child); descendantsOf(child.id, children, out, seen); }
    return out;
  };
  const TOKEN_KINDS = [["input", "Input"], ["output", "Output"], ["cache_write", "Cache write"], ["cache_read", "Cache read"]];
  const asMoney = (usd) => "$" + usd.toFixed(2), shortMoney = (usd) => "$" + usd.toFixed(1);
  const usageTotal = (s) => Object.values(s.tokens_by_model ?? {}).reduce((sum, usage) => sum + TOKEN_KINDS.reduce((n, [key]) => n + (Number(usage[key]) || 0), 0), 0);
  function costForSessions(sessions) {
    const kinds = Object.fromEntries(TOKEN_KINDS.map(([key]) => [key, { tokens: 0, usd: 0 }])), models = new Map(), unknown = new Set(); let knownUsd = 0;
    for (const s of sessions) for (const [modelId, usage] of Object.entries(s.tokens_by_model ?? {})) {
      const price = PRICING[modelId], current = models.get(modelId) ?? { modelId, kinds: Object.fromEntries(TOKEN_KINDS.map(([key]) => [key, { tokens: 0, usd: 0 }])), usd: 0, priced: !!price };
      if (!price) { unknown.add(modelId); current.priced = false; }
      for (const [key] of TOKEN_KINDS) {
        const tokens = Number(usage[key]) || 0; current.kinds[key].tokens += tokens; kinds[key].tokens += tokens;
        if (price) { const amount = tokens * price[key] / 1e6; current.kinds[key].usd += amount; kinds[key].usd += amount; current.usd += amount; knownUsd += amount; }
      }
      models.set(modelId, current);
    }
    return { usd: unknown.size ? null : knownUsd, knownUsd, kinds, models: [...models.values()], unknown: [...unknown] };
  }
  const costForSession = (sid, includeRuns = false) => costForSessions(SESS[sid] ? [SESS[sid], ...(includeRuns ? descendantsOf(sid, sessionChildren()) : [])] : []);
  const costText = (cost) => cost.unknown.length ? "—" : asMoney(cost.usd);
  const COST_TIP = "What these tokens would cost at API rates. Subscriptions (Claude Max, ChatGPT plans) aren't billed this way.";
  function costInfoTip() { const b = el("span", "cost-info"); b.title = COST_TIP; b.setAttribute("role", "img"); b.setAttribute("aria-label", COST_TIP); b.append(icon(I.q)); return b; }
  const TREE_RANK = { wait: 0, work: 1, err: 2, idle: 3, done: 4 };
  const urgentDescendant = (sid, children) => descendantsOf(sid, children).filter((s) => s.state in TREE_RANK).sort((a, b) => TREE_RANK[a.state] - TREE_RANK[b.state] || b.last - a.last)[0]?.state;
  const defaultTreeOpen = (sid, children) => descendantsOf(sid, children).some((s) => s.state === "wait" || s.state === "work");
  const matchesTree = (sid, children, seen = new Set()) => {
    if (seen.has(sid)) return false;
    seen.add(sid);
    return sessMatch(SESS[sid], query) || (children.get(sid) ?? []).some((s) => matchesTree(s.id, children, seen));
  };
  // The sidebar keeps the 8 most recently active top-level sessions, with children nested beneath their parent.
  function buildLaneItem(s, depth, children, rail) {
    const kids = children.get(s.id) ?? [], allKids = descendantsOf(s.id, children), item = el("div", "treeitem");
    item.dataset.id = s.id; item.setAttribute("role", "treeitem"); item.setAttribute("aria-label", s.name); item.tabIndex = 0;
    const saved = treePrefs[s.id], open = typeof saved?.open === "boolean" ? saved.open : defaultTreeOpen(s.id, children);
    if (kids.length && !rail) item.setAttribute("aria-expanded", String(open));
    const line = el("div", "tree-row");
    if (kids.length && !rail) {
      const toggle = el("button", "tree-toggle"); toggle.type = "button"; toggle.dataset.treeToggle = s.id; toggle.setAttribute("aria-label", (open ? "Collapse " : "Expand ") + s.name); toggle.setAttribute("aria-expanded", String(open)); toggle.append(icon(I.chev));
      toggle.addEventListener("click", (e) => { e.stopPropagation(); const value = item.getAttribute("aria-expanded") !== "true"; item.setAttribute("aria-expanded", String(value)); toggle.setAttribute("aria-expanded", String(value)); toggle.setAttribute("aria-label", (value ? "Collapse " : "Expand ") + s.name); saveTreePref(s.id, value); });
      line.append(toggle);
    } else line.append(el("span", "tree-spacer"));
    const row = el("button", "srow"); row.type = "button"; row.dataset.id = s.id; row.title = s.name;
    row.setAttribute("aria-label", s.name + ", " + (STATE[s.state] ?? s.state) + ", " + (HARNESS[s.harness] ?? s.harness) + ", " + shortHost(s));
    const target = route.v === "session" ? route.id : route.v === "trace" ? route.sid : null;
    let ancestor = target, selected = false; const seen = new Set();
    while (ancestor && SESS[ancestor] && !seen.has(ancestor)) { if (ancestor === s.id) { selected = true; break; } seen.add(ancestor); ancestor = parentOf(ancestor); }
    if (selected) row.setAttribute("aria-current", "page");
    const main = el("span", "srow-main"); main.append(dot(s.state), harnessMark(s.harness), el("span", "nm", s.name), el("span", "ag", ago(s.last)));
    if (rail && allKids.some((x) => x.state === "work" || x.state === "wait")) { const childDot = dot(urgentDescendant(s.id, children) ?? "work"); childDot.classList.add("child-dot"); childDot.setAttribute("aria-hidden", "true"); main.append(childDot); }
    if (kids.length && !rail && !open && allKids.length) { const summary = el("span", "tree-summary"); const state = urgentDescendant(s.id, children); if (state) summary.append(dot(state)); summary.append(String(allKids.length)); main.append(summary); }
    const meta = el("span", "srow-meta"); meta.append(icon(I.machine), el("span", "host", shortHost(s)), el("span", "repo-short", s.repo ?? "no repo")); meta.querySelector(".host").title = hostOf(s); meta.querySelector(".repo-short").title = branchOf(s);
    row.append(main, meta); row.addEventListener("click", () => goSession(s.id)); line.append(row); item.append(line);
    item.addEventListener("keydown", (e) => {
      if (kids.length && !rail && (e.key === "ArrowLeft" || e.key === "ArrowRight")) { const next = e.key === "ArrowRight"; if ((item.getAttribute("aria-expanded") === "true") !== next) { e.preventDefault(); item.querySelector(":scope > .tree-row .tree-toggle")?.click(); } }
      else if ((e.key === "Enter" || e.key === " ") && e.target === item) { e.preventDefault(); goSession(s.id); }
    });
    if (kids.length && !rail) { const group = el("div", "tree-group"); group.dataset.depth = String(Math.min(depth + 1, 4)); group.setAttribute("role", "group"); group.setAttribute("aria-label", "Sessions spawned by " + s.name); for (const child of kids) group.append(buildLaneItem(child, depth + 1, children, rail)); item.append(group); }
    return item;
  }
  function renderLanes() {
    const children = sessionChildren(), lanes = Object.values(SESS).filter((s) => s.lane && !parentOf(s.id) && matchesTree(s.id, children)).sort((a, b) => b.last - a.last);
    const box = $("#lanes"), more = $("#lanes-all"); box.replaceChildren(); more.replaceChildren();
    for (const s of lanes.slice(0, 8)) box.append(buildLaneItem(s, 0, children, railMode && !phone.matches));
    if (!lanes.length) { const empty = el("p", "ghead", "No sessions match"); empty.setAttribute("role", "none"); box.append(empty); }
    const all = el("button", "side-all", query ? "All matching sessions ›" : "All sessions ›"); all.type = "button"; all.id = "all-sessions"; all.addEventListener("click", () => go({ v: "sessions" })); more.append(all);
    const q = $("#q"); if (document.activeElement !== q) q.value = query;
  }

  // ---- Top bar ---------------------------------------------------------------------------------------
  // Always visible, the same on every page: the menu toggle (phones), the title, and the page's actions. A list page
  // (Home, Sessions, Machines) shows its name. A detail page (a session, a trace, a machine) shows two lines: its name
  // after a crumb up a level, then a one-line summary that ellipsizes. On a session, search takes over the bar and the
  // filter drops down from it; tapping the title block opens the session's details.
  function renderTopbar(title, crumb, opts = {}) {
    const bar = $("#topbar"), s = opts.session; bar.replaceChildren(); bar.classList.remove("scrolled");
    bar.classList.toggle("detail", !!opts.line2); bar.classList.toggle("searching", !!(s && findOpen));
    if (s && findOpen) { searchBar(bar); appendWideToggle(bar); const account = accountWidget(false); if (account) bar.append(account); return; }
    const m = el("button", "ibtn lead"); m.id = "lead-btn"; m.type = "button"; m.setAttribute("aria-label", "Open navigation"); m.setAttribute("aria-controls", "sidebar"); m.setAttribute("aria-expanded", "false"); m.append(icon(I.menu)); m.addEventListener("click", openDrawer); bar.append(m);
    const t = el("div", "ttl"), l1 = el("div", "l1");
    if (opts.lineage?.length) {
      if (phone.matches) { const parent = opts.lineage.at(-1), c = el("button", "crumb lineage-parent", parent.name); c.type = "button"; c.setAttribute("aria-label", "Open session path through " + parent.name); c.addEventListener("click", () => showLineageMenu(s.id, bar)); l1.append(c, el("span", "sep", "›")); }
      else opts.lineage.forEach((item) => { const c = el("button", "crumb", item.name); c.type = "button"; c.setAttribute("aria-label", "Open " + item.name); c.addEventListener("click", () => goSession(item.id)); l1.append(c, el("span", "sep", "›")); });
    } else if (crumb) { const c = el("button", "crumb", crumb.label); c.type = "button"; c.setAttribute("aria-label", "Back to " + crumb.label); c.addEventListener("click", crumb.go); l1.append(c, el("span", "sep", "›")); }
    const tt = el("span", "t", title); tt.title = title; l1.append(tt); t.append(l1);
    if (opts.line2) {
      const l2 = el("div", "l2" + (s ? " session-meta" : ""));
      if (s) { const hit = el("button", "meta-hit"); hit.type = "button"; hit.setAttribute("aria-label", s.name + ": open Session details"); hit.addEventListener("click", () => openSessionDetails(s)); l2.append(hit); }
      opts.line2(l2); t.append(l2);
      if (s) requestAnimationFrame(() => { if (l2.isConnected) fitSessionLine(l2); });
    }
    bar.append(t); if (s && !phone.matches) { const nav = siblingNav(s); if (nav) bar.append(nav); }
    if (opts.analytics) { appendAnalyticsRange(bar); appendWideToggle(bar); const account = accountWidget(false); if (account) bar.append(account); return; }
    if (!s) { appendWideToggle(bar); const account = accountWidget(false); if (account) bar.append(account); return; }
    const fb = el("button", "ibtn"); fb.type = "button"; fb.setAttribute("aria-label", "Find in transcript"); fb.append(icon(I.search));
    fb.addEventListener("click", () => { findOpen = true; filterOpen = false; render(); $("#find")?.focus(); });
    const pop = el("div", "filters pop"); pop.hidden = !filterOpen;
    for (const [key, label] of [["messages", "Messages"], ["tools", "Tool steps"], ["thinking", "Thinking"]]) { const l = el("label"); const cb = el("input"); cb.type = "checkbox"; cb.checked = show[key]; cb.id = "f-" + key; cb.addEventListener("change", () => { show[key] = cb.checked; render(); }); l.append(cb, label); pop.append(l); }
    const filtered = !(show.messages && show.tools && show.thinking);
    const tb = el("button", "ibtn" + (filtered ? " on" : "")); tb.id = "filter-btn"; tb.type = "button"; tb.setAttribute("aria-label", "Filter transcript"); tb.setAttribute("aria-expanded", String(filterOpen)); tb.append(icon(I.filter));
    tb.addEventListener("click", () => { $(".session-menu")?.remove(); closeAccountMenu(); $("#more-btn")?.setAttribute("aria-expanded", "false"); filterOpen = pop.hidden; pop.hidden = !filterOpen; tb.setAttribute("aria-expanded", String(filterOpen)); });
    const more = el("button", "ibtn" + (phone.matches && filtered ? " on" : "")); more.id = "more-btn"; more.type = "button"; more.setAttribute("aria-label", "Session details and actions"); more.setAttribute("aria-expanded", "false"); more.append(icon(I.more)); more.addEventListener("click", (e) => { e.stopPropagation(); toggleMenu(s, more); });
    // On phones search and filter live in the ⋯ menu, and the filter hangs from that button.
    const place = () => { const anchor = phone.matches ? more : tb; pop.style.right = Math.max(0, bar.getBoundingClientRect().right - anchor.getBoundingClientRect().right) + "px"; };
    tb.addEventListener("click", place);
    if (phone.matches) bar.append(more, pop); else bar.append(fb, tb, more, pop);
    appendWideToggle(bar); if (filterOpen) place();
    const account = accountWidget(false); if (account) bar.append(account);
  }
  function appendWideToggle(bar) {
    const b = el("button", "ibtn wide-toggle"); b.type = "button"; b.setAttribute("aria-label", "Wide reading mode"); b.setAttribute("aria-pressed", String(wideMode)); b.title = "Wide reading mode"; b.append(icon(I.wide));
    b.addEventListener("click", () => setWideMode(!wideMode)); bar.append(b);
  }
  function appendAnalyticsRange(bar) {
    const group = el("div", "analytics-range"); group.setAttribute("role", "group"); group.setAttribute("aria-label", "Analytics range");
    for (const [days, label] of [[1, "24 h"], [7, "7 d"], [30, "30 d"]]) {
      const b = el("button", null, label); b.type = "button"; b.dataset.e = "analytics-range:" + days; b.setAttribute("aria-pressed", String(analyticsRange === days));
      b.addEventListener("click", () => { if (analyticsRange === days) return; const top = currentScroll(); analyticsRange = days; render(); restoreScroll(top); }); group.append(b);
    }
    bar.append(group);
  }
  function lineageOf(sid) {
    const path = [], seen = new Set(); let id = sid;
    while (id && SESS[id] && !seen.has(id)) { seen.add(id); path.push(SESS[id]); id = parentOf(id); }
    return path.reverse();
  }
  function showLineageMenu(sid, bar) {
    bar.querySelector(".lineage-menu")?.remove(); const path = lineageOf(sid), menu = el("div", "lineage-menu"); menu.setAttribute("role", "menu"); menu.setAttribute("aria-label", "Session path");
    path.forEach((s, i) => { const b = el("button"); b.type = "button"; b.setAttribute("role", "menuitem"); if (i === path.length - 1) b.setAttribute("aria-current", "page"); b.append(harnessMark(s.harness), el("span", null, s.name)); b.addEventListener("click", () => { menu.remove(); goSession(s.id); }); menu.append(b); });
    bar.append(menu); const close = (e) => { if (!menu.contains(e.target) && !e.target.closest?.(".lineage-parent")) { menu.remove(); document.removeEventListener("click", close); } }; setTimeout(() => document.addEventListener("click", close), 0);
  }
  function siblingNav(s) {
    const parent = parentOf(s.id); if (!parent) return null;
    const siblings = childSessions(parent), at = siblings.findIndex((x) => x.id === s.id); if (siblings.length < 2 || at < 0) return null;
    const nav = el("div", "sibling-nav"); nav.setAttribute("role", "group"); nav.setAttribute("aria-label", "Sibling sessions");
    for (const [delta, label, path] of [[-1, "Previous sibling", I.back], [1, "Next sibling", I.chev]]) {
      const target = siblings[at + delta], b = el("button", "ibtn"); b.type = "button"; b.disabled = !target; b.setAttribute("aria-label", target ? label + ": " + target.name : label); b.append(icon(path)); if (target) b.addEventListener("click", () => goSession(target.id)); nav.append(b);
    }
    nav.insertBefore(el("span", "sibling-count", (at + 1) + " of " + siblings.length), nav.lastChild); return nav;
  }
  // Kind and state are the two items line 2 always keeps. Everything else drops from the right in append order
  // (model, machine, branch, tools, runs, tokens, cost); errors, the kind word, turn count, then state word give way last.
  function fitSessionLine(l2) {
    const keep = new Set(["meta-kind", "meta-state"]), droppable = [...l2.children].filter((n) => (n.classList.contains("meta-item") || n.classList.contains("meta-runs")) && ![...n.classList].some((c) => keep.has(c)));
    droppable.forEach((n) => { n.hidden = false; });
    const errs = l2.querySelector(".errs"); if (errs) errs.hidden = false;
    const kindValue = l2.querySelector(".meta-kind .meta-value"); if (kindValue) kindValue.hidden = false;
    const stateValues = [...l2.querySelectorAll(".meta-state .meta-value")], sep = l2.querySelector(".meta-state .state-sep"); stateValues.forEach((n) => { n.hidden = false; }); if (sep) sep.hidden = false;
    const fits = () => l2.scrollWidth <= l2.clientWidth + 1;
    for (let i = droppable.length - 1; i >= 0 && !fits(); i--) droppable[i].hidden = true;
    if (!fits() && errs) errs.hidden = true;
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
  function childKindChip(s, meta = false) {
    const c = el("span", meta ? "meta-item meta-kind" : "child-kind"); c.style.setProperty("--h", "var(--" + s.harness + ")");
    const mark = s.kind === "Subagent" ? icon(I.role) : s.kind === "Relayed" ? icon(I.relay) : harnessMark(s.harness);
    c.append(mark, el("span", meta ? "meta-value" : null, s.kind ?? (s.harness === "codex" ? "Codex run" : "Subagent"))); return c;
  }
  // A session's compact metadata line: state, model, machine, branch, tools, runs, tokens and API-equivalent cost.
  const sessionLine = (s) => (l2) => {
    const es = TX[s.id] ?? [], m = TXM[s.id], calls = m ? m.calls : es.filter((e) => e.k === "tool").length, errors = m ? m.errors : es.filter((e) => e.k === "tool" && e.ok === false).length, nT = (TURNS[s.id] ?? []).filter(hasTurn).length;
    const st = el("span", "meta-item meta-state"); st.append(dot(s.state), el("span", "meta-value", STATE[s.state]), el("span", "state-sep", "·"), el("span", "meta-value", nT + (nT === 1 ? " turn" : " turns")));
    if (errors) { const j = el("button", "errs", errors + (errors === 1 ? " error" : " errors")); j.type = "button"; j.setAttribute("aria-label", j.textContent + ": jump to the first failed step");
      j.addEventListener("click", (ev) => { ev.stopPropagation(); const e = $(".step.err"); const gs = e?.closest(".tgroup")?.querySelector(".tsum"); if (gs?.getAttribute("aria-expanded") === "false") gs.click(); if (e) { e.scrollIntoView({ behavior: "smooth", block: "center" }); const t = e.querySelector("button"); if (t?.getAttribute("aria-expanded") === "false") t.click(); } });
      st.append(j); }
    const kind = s.kind ? childKindChip(s, true) : null;
    const model = el("span", "meta-item meta-model"); model.append(harnessMark(s.harness), el("span", "meta-value", shortModel(s.model))); model.title = s.model ?? "Unknown model";
    const machine = el("span", "meta-item meta-machine"); machine.append(icon(I.machine), el("span", "meta-value", shortHost(s))); machine.title = hostOf(s);
    const branch = el("span", "meta-item meta-branch"); branch.append(icon(I.branch), el("span", "meta-value", branchOf(s))); branch.title = branchOf(s);
    const tools = el("span", "meta-item meta-tools"); tools.append(icon(I.wrench), el("span", "meta-value", String(calls))); tools.setAttribute("aria-label", calls + (calls === 1 ? " tool call" : " tool calls"));
    const kids = childSessions(s.id), allKids = descendantsOf(s.id, sessionChildren());
    let runs = null;
    if (kids.length) { runs = el("button", "meta-item meta-runs"); runs.type = "button"; runs.setAttribute("aria-label", kids.length + (kids.length === 1 ? " child session" : " child sessions") + (allKids.some((x) => x.state === "work") ? ", work in progress" : "") + ": open runs"); runs.append(icon(I.stack), el("span", "meta-value", String(kids.length))); if (allKids.some((x) => x.state === "work")) runs.append(dot("work")); runs.addEventListener("click", (e) => { e.stopPropagation(); openRuns(s, runs); }); }
    const totalTokens = usageTotal(s);
    const tokens = el("span", "meta-item meta-tokens"); tokens.append(icon(I.tokens), el("span", "meta-value", tok(totalTokens / 1e6))); tokens.title = totalTokens.toLocaleString() + " tokens";
    const parentCost = kids.length ? costForSessions([s, ...allKids]) : costForSession(s.id), costItem = el("span", "meta-item meta-cost"); costItem.append(icon(I.coin), el("span", "meta-value", (kids.length ? "incl. runs " : "") + (parentCost.unknown.length ? "—" : shortMoney(parentCost.usd)))); costItem.title = "API-equivalent cost. " + COST_TIP + (parentCost.unknown.length ? " no price for " + parentCost.unknown.join(", ") : ""); costItem.setAttribute("aria-label", "API-equivalent cost " + costText(parentCost) + (kids.length ? ", including runs" : "") + ". " + COST_TIP + (parentCost.unknown.length ? " no price for " + parentCost.unknown.join(", ") : ""));
    l2.append(...(kind ? [kind] : []), st, model, machine, branch, tools, ...(runs ? [runs] : []), tokens, costItem);
  };
  const machineLine = (m) => (l2) => { const here = onMachine(m), w = here.filter((s) => s.state === "work").length, up = MACHINE_UP[m];
    const st = el("span", "stat " + (!up ? "err" : w ? "work" : "idle")); st.append(dot(!up ? "err" : w ? "work" : "idle"), !up ? "Not responding" : w ? "Up" : "Idle"); l2.append(st, el("span", "sep", " · "));
    l2.append(el("span", "rest", up ? w + " working · " + here.length + (here.length === 1 ? " session" : " sessions") : movedOff(m).length ? "Semon moved its sessions to other machines" : [MACHINE_LAST[m] != null ? "Last seen " + clock(MACHINE_LAST[m]) : null, here.length + (here.length === 1 ? " session" : " sessions")].filter(Boolean).join(" · "))); };
  // One observer for the current page title; the previous page's is disconnected so it can't flip the new bar.
  let titleObs = null;
  function observeTitle() { syncBarLine(); }
  // The bar's divider shows only once the page has scrolled.
  function syncBarLine() { const y = phone.matches ? window.scrollY : $("#main").scrollTop; $("#topbar").classList.toggle("scrolled", y > 4); }
  window.addEventListener("scroll", syncBarLine, { passive: true });
  $("#main").addEventListener("scroll", syncBarLine, { passive: true });
  window.addEventListener("resize", () => { const l2 = $("#topbar .l2.session-meta"); if (l2) fitSessionLine(l2); syncLayoutPrefs(); syncJump(); }, { passive: true });
  function toggleMenu(s, btn) {
    const ex = $(".session-menu"); if (ex) { ex.remove(); btn.setAttribute("aria-expanded", "false"); return; }
    const filterWasOpen = phone.matches && filterOpen;
    closeAccountMenu();
    const m = el("div", "menu session-menu"); m.setAttribute("role", "menu");
    if (phone.matches) {
      const findItem = el("button"); findItem.type = "button"; findItem.setAttribute("role", "menuitem"); findItem.append(icon(I.search, "icon"), el("span", null, "Find in transcript"));
      findItem.addEventListener("click", (e) => { e.stopPropagation(); m.remove(); findOpen = true; filterOpen = false; render(); $("#find")?.focus(); });
      const filtered = !(show.messages && show.tools && show.thinking), filterItem = el("button"); filterItem.type = "button"; filterItem.setAttribute("role", "menuitem"); filterItem.append(icon(I.filter, "icon"), el("span", null, "Filter transcript"));
      if (filtered) filterItem.append(el("span", "menu-note", "On"));
      filterItem.addEventListener("click", (e) => { e.stopPropagation(); m.remove(); btn.setAttribute("aria-expanded", "false"); const pop = $(".filters.pop"); if (!pop) return; filterOpen = filterWasOpen ? false : pop.hidden; pop.hidden = !filterOpen; if (filterOpen) { pop.style.right = Math.max(0, $("#topbar").getBoundingClientRect().right - btn.getBoundingClientRect().right) + "px"; pop.querySelector("input")?.focus(); } });
      m.append(findItem, filterItem);
      const runs = $("#topbar .meta-runs"), kids = childSessions(s.id);
      if (runs?.hidden && kids.length) { const item = el("button", "menu-runs"); item.type = "button"; item.setAttribute("role", "menuitem"); item.append(icon(I.stack, "icon"), el("span", null, "Runs · " + kids.length)); item.addEventListener("click", (e) => { e.stopPropagation(); m.remove(); btn.setAttribute("aria-expanded", "false"); openRuns(s, runs); }); m.append(item); }
    }
    const copy = el("button"); copy.type = "button"; copy.append(icon(I.copy, "icon"), el("span", null, "Copy resume command"));
    const cmd = s.harness === "codex" ? "codex resume " + s.id : "claude --resume " + s.id;
    copy.addEventListener("click", () => { navigator.clipboard?.writeText(cmd).then(() => { copy.lastChild.textContent = "Copied"; }, () => { copy.lastChild.textContent = cmd; }); });
    m.append(copy);
    if (s.harness === "claude") { const a = el("button"); a.type = "button"; a.append(icon(I.ext, "icon"), el("span", null, "Open in claude.ai")); m.append(a); }
    const dl = el("dl");
    for (const [k, v] of [["Model", s.model], ["Machine", MACHINE[s.machine] + (s.movedFrom ? " (moved from " + MACHINE[s.movedFrom] + ")" : "")], ["Started", clock(s.start)], ["Duration", dur(s.start, s.state === "work" ? null : s.last)], ["Tokens in / out", tok(s.tokens[0]) + " / " + tok(s.tokens[2])], ["Cached context", tok(s.tokens[1])], ["Session id", s.id]]) dl.append(el("dt", null, k), el("dd", "mono", v));
    m.append(dl); closeFilter(); $("#topbar").append(m); btn.setAttribute("aria-expanded", "true");
  }
  function openSessionDetails(s) {
    $(".session-menu")?.remove(); $("#more-btn")?.setAttribute("aria-expanded", "false"); closeAccountMenu(); closeFilter();
    const d = el("dialog", "session-details"); d.setAttribute("aria-labelledby", "session-details-title");
    const head = el("div", "vh"), title = el("div", "vt"), close = el("button", "vclose");
    title.id = "session-details-title"; title.append(el("span", null, "Session details"));
    close.type = "button"; close.setAttribute("aria-label", "Close session details"); close.append(icon(I.x)); close.addEventListener("click", () => d.close()); head.append(title, close);
    const body = el("div", "vb"), list = el("div", "detail-list"), moved = s.movedFrom ? " (moved from " + (MACHINE[s.movedFrom] ?? s.movedFrom) + ")" : "";
    const rows = [
      ["Harness", HARNESS[s.harness] ?? s.harness], ["Model", s.model ?? s.modelId ?? "Unknown model"],
      ["Machine", (MACHINE[s.machine] ?? s.machine ?? "Unknown machine") + (hostOf(s) !== (MACHINE[s.machine] ?? s.machine) ? " · " + hostOf(s) : "") + moved],
    ];
    const directory = s.cwd ?? s.dir ?? s.directory;
    if (directory != null && directory !== "") rows.push(["Directory", directory]);
    rows.push([s.worktree ? "Worktree" : "Branch", branchOf(s)]);
    if (s.pid != null && s.pid !== "") rows.push(["Process id", String(s.pid)]);
    rows.push(["Session id", s.sessionId ?? s.id], ["Started", clock(s.start)], ["Duration", dur(s.start, s.state === "work" ? null : s.last)], ["Last activity", clock(s.last)], ["Tokens in / out", tok(s.tokens?.[0] ?? 0) + " / " + tok(s.tokens?.[2] ?? 0)], ["Cached context", tok(s.tokens?.[1] ?? 0)]);
    for (const [label, value] of rows) { const row = el("div", "detail-row"); row.append(el("span", "detail-label", label), el("span", "detail-value", String(value))); list.append(row); }
    const ownCost = costForSession(s.id), allCost = costForSession(s.id, true), hasRuns = childSessions(s.id).length > 0;
    const costRow = el("button", "detail-row cost-row"); costRow.type = "button"; costRow.setAttribute("aria-expanded", "false");
    const costLabel = el("span", "detail-label", "API-equivalent cost"); costLabel.append(costInfoTip());
    costRow.append(costLabel, el("span", "detail-value", hasRuns ? costText(ownCost) + " own · " + costText(allCost) + " incl. runs" : costText(ownCost)));
    const breakdown = costBreakdown(s.id, true); breakdown.hidden = true;
    costRow.addEventListener("click", () => { breakdown.hidden = !breakdown.hidden; costRow.setAttribute("aria-expanded", String(!breakdown.hidden)); });
    list.append(costRow); if (allCost.unknown.length) list.append(el("div", "no-price", "no price for " + allCost.unknown.join(", "))); list.append(breakdown);
    body.append(list); d.append(head, body); document.body.append(d);
    d.addEventListener("click", (ev) => { if (ev.target === d) d.close(); });
    d.addEventListener("close", () => { d.remove(); document.documentElement.classList.remove("viewer-open"); if (viewerEl === d) { viewerEl = null; if (history.state?.sheet) { skipPop = true; history.back(); } } if (LIVE.pending) refresh(); });
    viewerEl = d; document.documentElement.classList.add("viewer-open"); d.showModal(); close.focus();
    try { history.pushState({ ...route, sheet: 1 }, ""); } catch {}
  }
  function costBreakdown(sid, includeRuns) {
    const cost = costForSession(sid, includeRuns), box = el("div", "cost-breakdown");
    box.append(el("div", "cost-breakdown-head", includeRuns && childSessions(sid).length ? "Tokens and API-equivalent cost · incl. runs" : "Tokens and API-equivalent cost"));
    for (const model of cost.models) {
      const group = el("section", "cost-model"); group.append(el("div", "cost-model-name", model.modelId));
      for (const [key, label] of TOKEN_KINDS) {
        const item = model.kinds[key], row = el("div", "cost-line"); row.append(el("span", null, label), el("span", "cost-amount", item.tokens.toLocaleString() + " tokens"), el("span", "cost-value", model.priced ? asMoney(item.usd) : "—")); group.append(row);
      }
      if (!model.priced) group.append(el("div", "no-price", "no price for " + model.modelId));
      box.append(group);
    }
    if (!cost.models.length) box.append(el("p", "empty", "No token usage recorded.")); return box;
  }
  function runRow(s, depth, sheet) {
    const row = el("button", "runs-row"); row.type = "button"; row.style.paddingLeft = Math.min(depth, 3) * 14 + "px";
    const name = el("span", "run-name"); name.append(dot(s.state), childKindChip(s), el("span", null, s.name));
    const cost = costForSession(s.id); row.append(name, el("span", "run-cost", costText(cost)));
    const calls = TXM[s.id]?.calls ?? ANALYTICS_COUNTS[s.id]?.calls ?? (TX[s.id] ?? []).filter((e) => e.k === "tool").length, origin = originHandoff(s.id), meta = el("span", "run-meta");
    meta.append(el("span", null, STATE[s.state]), el("span", null, dur(s.start, s.state === "work" ? null : s.last)), el("span", null, calls + (calls === 1 ? " tool call" : " tool calls"))); row.append(meta);
    if (origin?.brief) row.append(el("span", "run-brief", oneLine(origin.brief)));
    if (cost.unknown.length) row.append(el("span", "no-price", "no price for " + cost.unknown.join(", ")));
    row.setAttribute("aria-label", [s.name, s.kind, STATE[s.state], dur(s.start, s.state === "work" ? null : s.last), origin?.brief ? oneLine(origin.brief) : "", "API-equivalent cost " + costText(cost)].filter(Boolean).join(" · "));
    row.addEventListener("click", () => { if (sheet) { pendingSessionOpen = s.id; sheet.close(); } else { $(".runs-popover")?.remove(); goSession(s.id); } }); return row;
  }
  function appendRunsTree(parent, box, sheet, seen = new Set([parent.id])) {
    const children = [...(sessionChildren().get(parent.id) ?? [])].sort((a, b) => b.last - a.last);
    for (const child of children) { if (seen.has(child.id)) continue; seen.add(child.id); box.append(runRow(child, 0, sheet)); const nested = sessionChildren().get(child.id) ?? []; if (nested.length) { const group = el("div", "runs-group"); appendRunsTree(child, group, sheet, seen); box.append(group); } }
  }
  function openRuns(s, anchor) {
    $(".runs-popover")?.remove(); if (viewerEl) return; const children = sessionChildren().get(s.id) ?? []; if (!children.length) return;
    if (!phone.matches) { const pop = el("div", "runs-popover"); pop.setAttribute("role", "dialog"); pop.setAttribute("aria-label", "Runs under " + s.name); pop.append(el("h2", null, "Runs · " + children.length + " · API-equivalent cost")); const tree = el("div", "runs-tree"); appendRunsTree(s, tree, false); pop.append(tree); $("#topbar").append(pop);
      const close = (e) => { if (!pop.contains(e.target) && e.target !== anchor) { pop.remove(); document.removeEventListener("click", close); } }; setTimeout(() => document.addEventListener("click", close), 0); return; }
    const d = el("dialog", "viewer runs-sheet"); d.setAttribute("aria-label", "Runs under " + s.name); const head = el("div", "vh"), title = el("div", "vt"), close = el("button", "vclose");
    title.append(el("span", null, "Runs · " + children.length + " · API-equivalent cost")); close.type = "button"; close.setAttribute("aria-label", "Close runs"); close.append(icon(I.x)); close.addEventListener("click", () => d.close()); head.append(title, close);
    const body = el("div", "vb"), tree = el("div", "runs-tree"); appendRunsTree(s, tree, d); body.append(tree); d.append(head, body); document.body.append(d); d.addEventListener("click", (e) => { if (e.target === d) d.close(); });
    d.addEventListener("close", () => { d.remove(); document.documentElement.classList.remove("viewer-open"); if (viewerEl === d) { viewerEl = null; if (history.state?.sheet) { skipPop = true; history.back(); } else if (pendingSessionOpen) { const id = pendingSessionOpen; pendingSessionOpen = null; goSession(id); } } });
    viewerEl = d; document.documentElement.classList.add("viewer-open"); d.showModal(); close.focus({ focusVisible: false }); try { history.pushState({ ...route, sheet: 1, scrollTop: currentScroll() }, ""); } catch {}
  }
  document.addEventListener("click", (e) => {
    const account = $(".account-popover"); if (account && !account.parentElement.contains(e.target)) closeAccountMenu();
    const m = $(".session-menu"); if (m && !m.contains(e.target)) { m.remove(); $("#more-btn")?.setAttribute("aria-expanded", "false"); }
    // A checkbox in the filter re-renders the bar, so its (now detached) target still sits inside the old popover.
    if (filterOpen && !e.target.closest?.(".filters, #filter-btn")) closeFilter(); });

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
    const meta = el("div", "meta"), s = SESS[sid]; const sw = el("span", "stat " + st); sw.append(st === "work" ? el("span", "spin") : dot(st), text); meta.append(sw);
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
    const s = SESS[sid], origin = originHandoff(sid), head = el("div", "ph sr"); const h1 = el("h1", null, s.name); head.append(h1); page.append(head); observeTitle(h1);
    if (origin) { page.classList.add("child-page"); page.style.setProperty("--h", "var(--" + s.harness + ")"); page.append(childBriefBlock(origin)); }
    page.append(transcript(sid, origin ? { excludeH: origin.id } : {}));
    if (origin && (s.state === "work" || s.state === "done" || s.state === "err" || origin.status === "done" || origin.status === "err")) page.append(childReturnBlock(s, origin));
  }

  const thoughtText = (e) => String(e.text ?? "").trim();
  const isPendingThought = (e) => !!(e.pending || e.status === "thinking");
  const isMaskedThought = (e) => e.k === "think" && !isPendingThought(e) && !thoughtText(e);
  function thoughtSeconds(e) {
    if (Number.isFinite(e?.secs) && e.secs >= 0) return e.secs;
    if (typeof e?.secs === "string") { const m = /^(\d+(?:\.\d+)?)s?$/.exec(e.secs.trim()); if (m) return Number(m[1]); }
    return null;
  }
  function transcriptEntries(entries) {
    const out = [];
    for (let i = 0; i < entries.length; i++) {
      const e = entries[i]; if (e.k !== "think") { out.push(e); continue; }
      const row = { ...e, displaySecs: thoughtSeconds(e) };
      if (!isMaskedThought(e)) { out.push(row); continue; }
      let j = i, sum = 0, measured = true;
      while (j < entries.length && isMaskedThought(entries[j])) { const secs = thoughtSeconds(entries[j]); if (secs == null) measured = false; else sum += secs; j++; }
      out.push({ ...row, displaySecs: measured ? Math.round(sum) : null, grouped: j - i }); i = j - 1;
    }
    return out;
  }

  // What a tool call did: an icon and verb for its step row, and a phrase and nouns for a group summary
  // ("ran 2 commands, asked you 1 question"). An unknown tool keeps its own name ("TodoWrite 1 step").
  const RUN = ["run", "Ran", "ran", "command", "commands"], FIND = ["find", "Searched for", "searched", "time", "times"];
  const TOOLS = { Bash: RUN, shell: RUN, exec_command: RUN, local_shell: RUN, Grep: FIND, Glob: FIND,
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
    const sec = el("section", opts.nested ? "nested" : parentOf(sid) ? "transcript-linked" : null); sec.setAttribute("aria-label", opts.nested ? SESS[sid].name + " transcript" : "Transcript"); Object.assign(sec.style, { display: "grid", gap: "10px", gridTemplateColumns: "minmax(0, 1fr)" });
    const entries = transcriptEntries(opts.entries ?? TX[sid] ?? []);
    const turnMode = !opts.nested;
    // On a session page the transcript is a list of turns, each with its own entries; a nested one is a plain list.
    const box = el("div", turnMode ? "turns" : "tx"); let tx = box; const hit = (s) => !find || s.toLowerCase().includes(find);
    const keyed = (n, e) => { if (e.key) n.dataset.e = e.key; return n; };
    const range = turnMode ? TXM[sid] : null;
    if (range?.from > 0) box.append(pager(sid, "before", "Load earlier"));
    else if (!find && turnMode) box.append(el("div", "divider", "Started " + clock(SESS[sid].start) + " on " + MACHINE[SESS[sid].movedFrom ?? SESS[sid].machine]));
    // Adjacent tool calls collapse into one summary line ("Ran 2 commands, read 1 file · 1 failed"),
    // expandable to the individual steps. A lone call stays a single line; while finding, matches show directly.
    let run = [];
    const flush = () => {
      if (!run.length) return;
      const steps = el("div", "steps"); run.forEach((r) => steps.append(r.node));
      const scripts = [], scriptKeys = new Set();
      for (const r of run) if (r.entry?.script != null && !scriptKeys.has(String(r.entry.script))) { scriptKeys.add(String(r.entry.script)); scripts.push(r.entry); }
      if ((run.length === 1 && !scripts.length) || find) { tx.append(steps); run = []; return; }
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
      for (const e of scripts) { const view = el("button", "viewscript", "View script"); view.type = "button"; view.addEventListener("click", () => openScript(e)); g.append(view); }
      g.append(steps); tx.append(g); run = [];
    };
    // A turn block: who started it, the work, and how it ended. While finding or filtering, a turn left with nothing drops out.
    const firsts = turnMode ? new Map((TURNS[sid] ?? []).filter((t) => t.entries[0]?.key).map((t) => [t.entries[0].key, t])) : new Map(); let cur = null;
    // A live update draws only the turns that changed (opts.only, by turn id).
    const owner = opts.only ? new Map((TURNS[sid] ?? []).flatMap((t) => t.entries.map((e) => [e.key, t.id]))) : null;
    const closeTurn = () => { flush(); if (!cur) return; const { t, blk } = cur; cur = null; tx = box;
      if ((find || !show.messages || !show.tools || !show.thinking) && !blk.querySelector(".msg, .step, .hcard, .think, .think-masked, .think-pending")) { blk.remove(); return; }
      if (opts.excludeH && t.last) return;
      const end = turnEnd(t); if (!end) return;
      const d = el("div", "turn-end"); const st = el("span", "stat " + end.st); st.append(end.st === "work" ? el("span", "spin") : dot(end.st), end.text); d.append(st);
      if (t.out.length) d.append(traceBtn(t)); blk.append(d); };
    const openTurn = (t) => { closeTurn(); const blk = el("section", "turn"); blk.dataset.turn = t.id;
      // Your own message needs no header: the bubble is yours and its time sits under it. A relay or brief says who sent it.
      const h = t.start;
      if (t.u || h?.kind === "ask") blk.setAttribute("aria-label", "Your message" + (h ? " at " + clock(h.at) : ""));
      else if (h && h.id !== opts.excludeH) { const hd = el("h3", "turn-h " + hcls(h.from)); const l = el("span", "lbl"); const b = el("button", "from", nameOf(h.from)); b.type = "button"; b.setAttribute("aria-label", "Open " + nameOf(h.from) + " where it sent this"); b.addEventListener("click", () => openSender(h)); l.append(el("span", "verb", h.kind === "relay" ? "Relay from " : "Brief from "), b); hd.append(icon(I.in), l, el("span", "tm", clock(h.at))); blk.append(hd); }
      tx = el("div", "tx"); blk.append(tx); box.append(blk); cur = { t, blk }; };
    for (const e of entries) {
      if (owner && !opts.only.has(owner.get(e.key))) continue;
      if (turnMode && isGap(e)) { closeTurn(); if (!find) box.append(el("div", "divider", e.text)); continue; }
      if (turnMode && firsts.has(e.key)) openTurn(firsts.get(e.key));
      if (opts.excludeH && e.k === "h" && e.id === opts.excludeH) continue;
      if (opts.excludeH && e.k === "end" && /^Returned to /.test(e.text ?? "")) continue;
      // Entries that render nothing (empty thinking, hidden kinds) must not split a run of tool calls.
      if (e.k === "think" && (!show.thinking || find)) continue;
      if (e.k === "tool") {
        if (!show.tools || !hit(e.name + " " + e.arg + " " + (e.in ?? "") + " " + (e.out ?? ""))) continue;
        const [ic, v] = verb(e.name);
        if (e.live) { const r = keyed(el("div", "step live"), e); r.dataset.live = sid; r.append(el("span", "spin"), el("span", "sv", v === "Ran" ? "Running" : v), el("code", "sa", e.arg), el("span", "sd tick", e.secs)); run.push({ node: r, v, k: e.name, live: true, secs: e.secs, key: e.key }); continue; }
        const box = keyed(el("div", "step" + (e.ok || e.ok === null ? "" : " err")), e); const b = el("button"); b.type = "button"; b.setAttribute("aria-expanded", "false");
        b.append(icon(I[ic]), el("span", "sv", v), el("code", "sa", e.arg), el("span", "sd", e.unfinished ? "no result" : e.exit != null ? "exit " + e.exit + " · " + e.secs : e.ok ? e.secs : e.ok === null ? "exit unknown · " + e.secs : "failed · " + e.secs), icon(I.chev, "chev"));
        const out = el("div", "out"); out.hidden = true;
        // Expanded, a step previews what was asked (the full command or input) and what came back, each cut at about
        // eleven lines. When either is cut, "View all" opens the whole call in a sheet.
        const inLabel = /^(Bash|shell|exec_command|local_shell)$/.test(e.name) ? "Command" : "Input";
        if (e.in) out.append(el("div", "io", inLabel), el("pre", "in clip", e.in));
        if (e.cwd) out.append(el("div", "io", "Working directory · " + e.cwd));
        if (e.in) out.append(el("div", "io", "Output"));
        if (e.changes) {
          for (const change of e.changes) {
            out.append(el("div", "io", "Change · " + change.path + (change.move ? " → " + change.move : "")));
            if (change.diff?.length) out.append(diffEl(change.diff, "clip")); else out.append(el("div", "noout", "No diff recorded"));
          }
          if (!e.changes.length) out.append(el("div", "noout", "No changes recorded"));
        } else if (e.diff) out.append(diffEl(e.diff, "clip")); else if (e.out) out.append(el("pre", "clip", e.out)); else out.append(el("div", "noout", e.unfinished ? "No result recorded" : "No output"));
        if (find && e.script != null) { const script = el("button", "viewscript", "View script"); script.type = "button"; script.addEventListener("click", () => openScript(e)); out.append(script); }
        const all = el("button", "viewall"); all.type = "button"; all.hidden = true; all.append(icon(I.expand), el("span", null, "View all"));
        all.addEventListener("click", () => openViewer(e, v, ic, inLabel)); out.append(all);
        b.addEventListener("click", () => { out.hidden = !out.hidden; b.setAttribute("aria-expanded", String(!out.hidden));
          if (!out.hidden) { let cut = !!e.more?.length; out.querySelectorAll(".clip").forEach((c) => { const x = c.scrollHeight > c.clientHeight + 1; c.classList.toggle("clipped", x); cut ||= x; }); all.hidden = !cut;
            // Text cut when this copy was made, with nothing more to show: say so instead of ending on "…".
            if (!cut && !out.querySelector(".cutnote") && [e.in, e.out].some((t) => /…(\(truncated\))?\s*$/.test(t ?? ""))) out.append(el("div", "cutnote", "Cut short in this copy of the logs")); } });
        box.append(b, out); run.push({ node: box, v, k: e.name, err: e.ok === false, key: e.key, entry: e }); continue;
      }
      flush();
      if (e.k === "u") { if (!show.messages || !hit(e.text)) continue; const m = keyed(el("div", "msg user"), e); m.append(markdown(e.text)); tx.append(m); }
      else if (e.k === "a") { if (!show.messages || !hit(e.text)) continue; const m = keyed(el("div", "msg assistant"), e); m.append(markdown(e.text)); tx.append(m); }
      else if (e.k === "think") {
        if (isPendingThought(e)) { const pending = keyed(el("div", "think-pending"), e); pending.append(el("span", "spin"), el("span", null, "Thinking…")); tx.append(pending); }
        else if (isMaskedThought(e)) { const label = e.displaySecs == null ? "Thought" : "Thought for " + e.displaySecs + "s"; tx.append(keyed(el("div", "think-masked", label), e)); }
        else {
          const group = el("div", "thought"), b = keyed(el("button", "think"), e), text = markdown(thoughtText(e), "think-text");
          b.type = "button"; b.setAttribute("aria-expanded", "false"); b.append(el("span", null, e.displaySecs == null ? "Thought" : "Thought for " + Math.round(e.displaySecs) + "s"), icon(I.chev, "chev"));
          text.hidden = true; b.addEventListener("click", () => { text.hidden = !text.hidden; b.setAttribute("aria-expanded", String(!text.hidden)); }); group.append(b, text); tx.append(group);
        }
      }
      else if (e.k === "harness") { if (!show.messages || find) continue; tx.append(keyed(el("div", "harness-note", "Harness text added before the prompt (" + e.label + ")"), e)); }
      else if (e.k === "end") { if (find) continue; tx.append(keyed(el("div", "divider", e.text), e)); }
      else if (e.k === "h") {
        const h = H.find((x) => x.id === e.id); if (!hit(h.brief + " " + (h.result ?? ""))) continue;
        // Your own ask is simply your message.
        if (h.kind === "ask") { if (!show.messages) continue; const m = keyed(el("div", "msg user"), e); m.append(markdown(h.brief)); tx.append(m); if (cur?.t.start === h) tx.append(el("div", "msg-tm", clock(h.at))); continue; }
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
  const diffEl = (rows, cls) => { const d = el("div", "diff" + (cls ? " " + cls : "")); rows.forEach(([c, t]) => d.append(el("div", c, t))); return d; };
  // The whole tool call. A phone gets a full-screen sheet and a wider screen a dialog; either way it is a history entry,
  // so the back gesture closes it without leaving the page.
  let viewerEl = null, skipPop = false;
  function openViewer(e, verb, ic, inLabel) {
    if (e.more?.length && e.slot != null && !e.full) { const open = (f) => openViewer({ ...e, ...f, full: true }, verb, ic, inLabel); fullOf(e).then(open, () => open({ fullFailed: true })); return; }
    const d = el("dialog", "viewer"); d.setAttribute("aria-label", verb + " " + e.arg);
    const head = el("div", "vh"); const t = el("div", "vt"); t.append(icon(I[ic]), el("span", null, verb + " " + e.arg));
    const close = el("button", "vclose"); close.type = "button"; close.setAttribute("aria-label", "Close"); close.append(icon(I.x)); close.addEventListener("click", () => d.close());
    head.append(t, close, el("div", "vm" + (e.ok || e.ok === null ? "" : " err"), e.name + " · " + (e.unfinished ? "no result" : e.ok ? e.secs : e.ok === null ? "exit unknown · " + e.secs : "failed · " + e.secs)));
    const body = el("div", "vb");
    const section = (label, text) => { const s = el("div", "vs"); s.append(el("span", null, label));
      if (text) { const c = el("button", "vcopy"); c.type = "button"; c.append(icon(I.copy), el("span", null, "Copy"));
        c.addEventListener("click", () => navigator.clipboard?.writeText(text).then(() => { c.lastChild.textContent = "Copied"; }, () => { c.lastChild.textContent = "Copy failed"; })); s.append(c); }
      body.append(s); };
    const cutNote = (text) => { if (!e.fullFailed && /…(\(truncated\))?\s*$/.test(text ?? "")) body.append(el("p", "vnote", "Cut short in this copy of the logs.")); };
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
      else { section("Output", e.out); if (e.out) { body.append(el("pre", null, e.out)); cutNote(e.out); } else body.append(el("p", "vnote", e.unfinished ? "No result recorded." : "No output.")); }
    }
    if (e.fullFailed) body.append(el("p", "vnote", "Couldn't load the full text: this is the preview."));
    if (e.fullCut?.length) body.append(el("p", "vnote", "Cut at 8 MB: the rest isn't shown."));
    d.append(head, body); document.body.append(d);
    d.addEventListener("click", (ev) => { if (ev.target === d) d.close(); }); // a tap on the backdrop (wide screens)
    d.addEventListener("close", () => { d.remove(); document.documentElement.classList.remove("viewer-open"); if (viewerEl === d) { viewerEl = null; if (history.state?.sheet) { skipPop = true; history.back(); } } if (LIVE.pending) refresh(); });
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
    if (phone.matches && SESS[h.to]) { const nav = siblingNav(SESS[h.to]); if (nav) title.append(nav); }
    block.append(title);
    const brief = markdown(h.brief, "brief"), more = el("button", "more", "Show more"); more.type = "button"; more.hidden = true; more.setAttribute("aria-expanded", "false");
    more.addEventListener("click", () => { const expanded = brief.classList.toggle("open"); more.textContent = expanded ? "Show less" : "Show more"; more.setAttribute("aria-expanded", String(expanded)); });
    new ResizeObserver(() => { if (brief.classList.contains("open") || !brief.clientHeight) return; const clipped = brief.scrollHeight > brief.clientHeight + 1; more.hidden = !clipped; brief.classList.toggle("clipped", clipped); }).observe(brief);
    const parentLink = el("button", "intro-open", "Open in " + parent.name); parentLink.type = "button"; parentLink.addEventListener("click", () => openParentAtHandoff(h)); block.append(brief, more, parentLink); return block;
  }
  function childReturnBlock(s, h) {
    const block = el("div", "child-return"), calls = TXM[s.id]?.calls ?? (TX[s.id] ?? []).filter((e) => e.k === "tool").length, finished = s.state === "done" || s.state === "err" || h.status === "done" || h.status === "err";
    const status = finished ? (s.state === "err" || h.status === "err" ? "err" : "done") : "work";
    const text = finished ? (status === "err" ? "Failed" : "Done") : "Working · " + calls + (calls === 1 ? " tool call" : " tool calls") + " · " + dur(s.start, null);
    const state = el("span", "stat " + status); state.append(status === "work" ? el("span", "spin") : dot(status), el("span", null, finished ? "Returned to " + nameOf(h.from) + " · " + text + " · " + dur(s.start, s.last) : text)); block.append(state);
    if (finished) { const link = el("button", null, "Open in " + nameOf(h.from)); link.type = "button"; link.addEventListener("click", () => openParentAtHandoff(h)); block.append(link); }
    return block;
  }

  function handoffCard(h, viewer, start) {
    const other = h.kind === "move" ? null : viewer === h.from ? h.to : h.from;
    const child = h.kind === "spawn" && viewer === h.from ? SESS[h.to] : null;
    const c = el("div", "hcard " + (child ? "child-card " + hcls(h.to) : h.kind === "toyou" ? "toyou" : h.kind === "move" ? "move" : hcls(other)) + (start ? " start" : "")); c.dataset.h = h.id; c.tabIndex = 0; c.setAttribute("role", "link");
    const [ic, parts] = sentence(h, viewer);
    if (child) { c.append(childKindChip(child)); const ln = el("span", "ln", child.name); ln.append(el("span", "verb", " · " + (child.kind ?? HARNESS[child.harness]))); c.append(ln); }
    else { c.append(icon(ic)); const ln = el("span", "ln"); ln.append(...parts); c.append(ln); }
    const shownState = child?.state ?? h.status, sw = el("span", "stat " + shownState); sw.append(shownState === "work" ? el("span", "spin") : dot(shownState === "done" ? "done" : shownState), child ? STATE[shownState] : statWord(h)); c.append(sw);
    if (child) { const calls = TXM[child.id]?.calls ?? (TX[child.id] ?? []).filter((e) => e.k === "tool").length, meta = el("div", "child-meta"); meta.append(el("span", null, dur(child.start, child.state === "work" ? null : child.last)), el("span", null, calls + (calls === 1 ? " tool call" : " tool calls"))); c.append(meta); }
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
        const group = el("div", "child-work"); group.dataset.e = "cw:" + h.id; group.addEventListener("click", (e) => e.stopPropagation()); const toggle = el("button", "cw-toggle"); toggle.type = "button"; toggle.setAttribute("aria-expanded", "false"); toggle.append(icon(I.chev, "chev"), "What " + child.name + " did");
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
  function render() {
    // The page first, then the bar: the bar's summary (a trace's counts, a search's matches) comes from the page.
    closeAccountMenu();
    tick(); const page = $("#page"), r = route; rendered = r; page.style.paddingBottom = ""; page.replaceChildren(); page.classList.remove("child-page"); page.style.removeProperty("--h");
    if (r.v === "home") { renderHome(page); renderTopbar("Home"); }
    else if (r.v === "analytics") { renderAnalytics(page); renderTopbar("Analytics", null, { analytics: true }); }
    else if (r.v === "sessions") { renderSessions(page); renderTopbar("Sessions"); }
    else if (r.v === "machines") { renderMachines(page); renderTopbar("Machines"); }
    else if (r.v === "machine") { renderMachine(page, r.id); renderTopbar(MACHINE[r.id], { label: "Machines", go: () => go({ v: "machines" }) }, { line2: machineLine(r.id) }); }
    else if (r.v === "trace") { const sum = renderTrace(page, r.turn) ?? ""; renderTopbar("Trace", { label: SESS[r.sid].name, go: () => goSession(r.sid, r.turn) }, { line2: (l2) => l2.append(el("span", "rest", sum)) }); }
    else if (r.v === "session") { const s = SESS[r.id], lineage = lineageOf(r.id).slice(0, -1); renderSession(page, r.id); renderTopbar(s.name, null, { session: s, lineage, line2: sessionLine(s) }); }
    document.documentElement.style.setProperty("--barh", $("#topbar").offsetHeight + "px");
    syncLayoutPrefs(); syncBarLine(); renderNav(); renderLanes(); renderDrawerAccount(); syncJump();
  }

  // Analytics uses epoch milliseconds from the served model. While a session is still working, extend its last
  // recorded busy interval to the model clock so the figures and charts include live work.
  const MIN = 60000, HOUR = 60 * MIN;
  const busyOf = (s) => { const iv = (s.busy ?? []).map(([a, b]) => [a, b]); if (s.state === "work" && iv.length) iv.at(-1)[1] = Math.max(iv.at(-1)[1], NOW); return iv; };

  // Analytics is computed from the served model's sessions, busy intervals, turn index and handoffs.
  // The tx endpoint gives per-session aggregate tool counts, so (as in the approved mockup) those calls are attributed to session start.
  const DAY_MS = 86400000;
  const rangeMs = (days) => days * DAY_MS;
  const inRange = (t, from, to) => Number.isFinite(t) && t >= from && t < to;
  function analyticsSessions() {
    return Object.values(SESS).map((s) => {
      const startedAt = Number(s.start) || 0, calls = ANALYTICS_COUNTS[s.id]?.calls ?? TXM[s.id]?.calls ?? 0;
      const errors = ANALYTICS_COUNTS[s.id]?.errors ?? TXM[s.id]?.errors ?? 0;
      const turns = (TURNS[s.id] ?? []).filter(hasTurn);
      return { s, id: s.id, startedAt, costAt: startedAt, busy: busyOf(s),
        turnEvents: turns.map((t) => Number.isFinite(t.at) ? t.at : Number.isFinite(t.start?.at) ? t.start.at : startedAt),
        toolEvents: Array.from({ length: Math.max(0, calls) }, (_, i) => ({ at: startedAt, error: i < errors })) };
    });
  }
  function analyticsWaits() {
    return H.filter((h) => h.kind === "toyou" && SESS[h.from])
      .map((h) => ({ sid: h.from, startAt: h.at, endAt: h.status === "wait" ? null : Number.isFinite(h.done) ? h.done : null }));
  }
  function busyMsIn(row, from, to) { return row.busy.reduce((sum, [a, b]) => sum + Math.max(0, Math.min(b, to) - Math.max(a, from)), 0); }
  const sessionFacetValue = (s, key) => key === "repo" ? s.repo ?? "__none__" : key === "model" ? s.model ?? s.modelId ?? "Unknown model" : s[key] ?? "";
  function matchesSessionFacets(s) { return Object.keys(sessionFilters).every((key) => !sessionFilters[key] || sessionFacetValue(s, key) === sessionFilters[key]); }
  function renderFacetFilters(onChange) {
    const box = el("div", "facet-filters"); box.setAttribute("aria-label", "Filter sessions");
    const specs = [
      ["repo", "Repo", "All repos", [...new Set(Object.values(SESS).map((s) => sessionFacetValue(s, "repo")))].sort((a, b) => a === "__none__" ? 1 : b === "__none__" ? -1 : a.localeCompare(b)), (v) => v === "__none__" ? "No repo" : v],
      ["machine", "Machine", "All machines", [...new Set(Object.values(SESS).map((s) => s.machine ?? ""))].sort(), (v) => MACHINE[v] ?? v],
      ["harness", "Harness", "All harnesses", [...new Set(Object.values(SESS).map((s) => s.harness ?? ""))].sort(), (v) => HARNESS[v] ?? v],
      ["model", "Model", "All models", [...new Set(Object.values(SESS).map((s) => sessionFacetValue(s, "model")))].sort(), shortModel],
    ];
    for (const [key, label, allLabel, values, showValue] of specs) {
      const field = el("label", "facet-field"); field.append(el("span", null, label)); const select = el("select"); select.setAttribute("aria-label", label);
      const all = el("option", null, allLabel); all.value = ""; select.append(all);
      for (const value of values) { const option = el("option", null, showValue(value)); option.value = value; select.append(option); }
      select.value = sessionFilters[key]; select.addEventListener("change", () => { sessionFilters[key] = select.value; onChange(); }); field.append(select); box.append(field);
    }
    return box;
  }
  function analyticsStats(rows, from, to) {
    const relevant = rows.filter((r) => inRange(r.startedAt, from, to) || r.busy.some(([a, b]) => a < to && b > from));
    const waits = analyticsWaits().filter((w) => rows.some((r) => r.id === w.sid) && w.startAt < to && (w.endAt ?? to) > from);
    const durations = waits.map((w) => Math.max(0, Math.min(w.endAt ?? to, to) - Math.max(w.startAt, from))).filter((x) => x > 0).sort((a, b) => a - b);
    const median = durations.length ? durations.length % 2 ? durations[(durations.length - 1) / 2] : (durations[durations.length / 2 - 1] + durations[durations.length / 2]) / 2 : 0;
    const waitBy = new Map(); for (const w of waits) { const ms = Math.max(0, Math.min(w.endAt ?? to, to) - Math.max(w.startAt, from)); waitBy.set(w.sid, (waitBy.get(w.sid) ?? 0) + ms); }
    const current = analyticsWaits().filter((w) => !w.endAt && rows.some((r) => r.id === w.sid)).map((w) => ({ ...w, ms: Math.max(0, NOW - w.startAt), s: SESS[w.sid] })).sort((a, b) => b.ms - a.ms);
    const agentMs = relevant.reduce((sum, r) => sum + busyMsIn(r, from, to), 0);
    const costRows = rows.filter((r) => inRange(r.costAt, from, to)).map((r) => ({ row: r, cost: costForSession(r.id) }));
    const costUnknown = [...new Set(costRows.flatMap((x) => x.cost.unknown))];
    return { rows: relevant, agentMs, started: rows.filter((r) => inRange(r.startedAt, from, to)).length,
      turns: rows.reduce((sum, r) => sum + r.turnEvents.filter((at) => inRange(at, from, to)).length, 0),
      tools: rows.reduce((sum, r) => sum + r.toolEvents.filter((e) => inRange(e.at, from, to)).length, 0),
      errors: rows.reduce((sum, r) => sum + r.toolEvents.filter((e) => inRange(e.at, from, to) && e.error).length, 0),
      waitsMs: durations.reduce((sum, x) => sum + x, 0), medianWaitMs: median, longestWaitMs: durations.at(-1) ?? 0,
      longestCurrent: current[0] ?? null, waitBy, costUnknown,
      apiCost: costUnknown.length ? null : costRows.reduce((sum, x) => sum + x.cost.knownUsd, 0) };
  }
  const hoursText = (ms) => (ms / HOUR).toFixed(1) + " h", rangeName = () => analyticsRange === 1 ? "24 h" : analyticsRange + " d";
  function deltaNote(value, previous, format) {
    const delta = value - previous, note = el("div", "note");
    if (Math.abs(delta) < 1e-9) { note.textContent = "No change vs previous " + rangeName(); return note; }
    note.append(el("span", delta > 0 ? "up" : "down", (delta > 0 ? "+" : "−") + format(Math.abs(delta))), " vs previous " + rangeName()); return note;
  }
  function peakBusy(rows, from, to) {
    const starts = rows.flatMap((r) => r.busy.filter(([a, b]) => a < to && b > from).map(([a]) => Math.max(a, from)));
    return starts.reduce((best, at) => Math.max(best, rows.filter((r) => r.busy.some(([a, b]) => a <= at && at < b)).length), 0);
  }
  function chartWidth() { const page = $("#page"), style = getComputedStyle(page); return Math.max(280, Math.round(page.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight))); }
  const niceStep = (max) => [.25, .5, 1, 2, 5, 10, 20, 50, 100, 200, 500].find((x) => x * 3 >= max) ?? 1000;
  const svgEl = (tag, attrs, text) => { const node = document.createElementNS(SVGNS, tag); for (const [key, value] of Object.entries(attrs)) node.setAttribute(key, String(value)); if (text != null) node.textContent = text; return node; };
  function timeText(ms) { const mins = Math.max(0, Math.round(ms / MIN)), days = Math.floor(mins / 1440), hours = Math.floor(mins % 1440 / 60), rem = mins % 60; return days ? days + "d " + hours + "h" : hours ? hours + "h " + rem + "m" : mins + "m"; }
  const countText = (n) => Math.round(n).toLocaleString(), hLabel = (n) => n ? +n.toFixed(2) + " h" : "0";
  const agentBuckets = () => analyticsRange === 1 ? [24, "per hour"] : analyticsRange === 7 ? [28, "per 6 hours"] : [30, "per day"];
  function renderAgentsChart(rows, from, to) {
    const [count, unit] = agentBuckets(), span = (to - from) / count, panel = el("section", "analytics-panel");
    panel.append(el("h2", null, "Agents at work"), el("div", "panel-sub", "Agent-hours " + unit + " · stacked by harness"));
    const bins = Array.from({ length: count }, (_, i) => { const a = from + i * span, b = a + span, hrs = (h) => rows.filter((r) => r.s.harness === h).reduce((n, r) => n + busyMsIn(r, a, b), 0) / HOUR; return { a, b, claude: hrs("claude"), codex: hrs("codex") }; });
    const W = chartWidth(), height = 190, left = 40, right = W - 4, top = 12, bottom = 151, most = Math.max(0, ...bins.map((x) => x.claude + x.codex)), stepY = niceStep(most || 1), max = Math.max(stepY, Math.ceil(most / stepY) * stepY);
    const svg = svgEl("svg", { viewBox: "0 0 " + W + " " + height, role: "group", "aria-label": "Agent-hours " + unit + " over the selected range, stacked by harness" }), yOf = (n) => bottom - (bottom - top) * n / max;
    for (let n = 0; n <= max + 1e-9; n += stepY) svg.append(svgEl("line", { x1: left, x2: right, y1: yOf(n), y2: yOf(n), class: "gridline" }), svgEl("text", { x: 0, y: yOf(n) + 4, class: "axis-label" }, hLabel(n)));
    const step = (right - left) / count, w = Math.max(2, step * .64);
    bins.forEach((bin, i) => { const x = left + i * step + (step - w) / 2, ch = (bottom - top) * bin.claude / max, xh = (bottom - top) * bin.codex / max, total = bin.claude + bin.codex;
      if (ch) svg.append(svgEl("rect", { x, y: bottom - ch, width: w, height: ch, class: "cost-claude" })); if (xh) svg.append(svgEl("rect", { x, y: bottom - ch - xh, width: w, height: xh, class: "cost-codex" }));
      const label = clock(bin.a) + "–" + clock(bin.b) + ": " + hLabel(total), hit = svgEl("rect", { x: left + i * step, y: top, width: step, height: bottom - top, class: "chart-hit" }); hit.append(svgEl("title", {}, label));
      if (total > 0) { hit.setAttribute("role", "button"); hit.setAttribute("tabindex", "0"); hit.setAttribute("aria-label", label + ". Open the sessions busy then"); }
      const open = () => { if (total > 0) openAnalyticsSlice(bin.a, bin.b, rows.filter((r) => busyMsIn(r, bin.a, bin.b) > 0)); };
      hit.addEventListener("click", open); hit.addEventListener("keydown", (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); open(); } }); svg.append(hit);
    });
    svg.append(svgEl("text", { x: left, y: 178, class: "axis-label" }, analyticsRange === 1 ? "24 h ago" : analyticsRange + " d ago"), svgEl("text", { x: right, y: 178, "text-anchor": "end", class: "axis-label" }, "Now"));
    const chart = el("div", "analytics-chart"); chart.append(svg); panel.append(chart);
    const legend = el("div", "analytics-legend"); for (const [h, label] of [["claude", "Claude"], ["codex", "Codex"]]) { const item = el("span"), swatch = el("i"); swatch.style.setProperty("--h", "var(--" + h + ")"); item.append(swatch, label); legend.append(item); } panel.append(legend); return panel;
  }
  function renderCostChart(rows, from, to) {
    const panel = el("section", "analytics-panel"), title = el("h2", null, "Cost over time"); title.append(costInfoTip());
    const count = analyticsRange === 1 ? 24 : analyticsRange, unit = analyticsRange === 1 ? "per hour" : "per day"; panel.append(title, el("div", "panel-sub", "API-equivalent cost " + unit + " · stacked by harness"));
    const bins = Array.from({ length: count }, (_, i) => ({ a: from + (to - from) * i / count, b: from + (to - from) * (i + 1) / count, claude: 0, codex: 0, rows: [] })), unknown = new Set();
    for (const row of rows) { if (!inRange(row.costAt, from, to)) continue; const cost = costForSession(row.id); if (cost.usd == null) { cost.unknown.forEach((model) => unknown.add(model)); continue; } const index = Math.min(count - 1, Math.floor((row.costAt - from) / (to - from) * count)); bins[index][row.s.harness] += cost.usd; bins[index].rows.push(row); }
    const W = chartWidth(), svg = svgEl("svg", { viewBox: "0 0 " + W + " 190", role: "img", "aria-label": "API-equivalent cost " + unit + ", stacked by harness" });
    const left = 46, right = W - 4, top = 12, bottom = 151, max = Math.max(.01, ...bins.map((b) => b.claude + b.codex)), step = (right - left) / count;
    for (let n = 0; n <= 2; n++) { const y = bottom - (bottom - top) * n / 2; svg.append(svgEl("line", { x1: left, x2: right, y1: y, y2: y, class: "gridline" }), svgEl("text", { x: 0, y: y + 4, class: "axis-label" }, "$" + (max * n / 2).toFixed(2))); }
    bins.forEach((bin, i) => { const w = Math.max(2, step * .64), x = left + i * step + (step - w) / 2, ch = bin.claude / max * (bottom - top), xh = bin.codex / max * (bottom - top);
      if (ch) svg.append(svgEl("rect", { x, y: bottom - ch, width: w, height: ch, class: "cost-claude" })); if (xh) svg.append(svgEl("rect", { x, y: bottom - ch - xh, width: w, height: xh, class: "cost-codex" }));
      const hit = svgEl("rect", { x: left + i * step, y: top, width: step, height: bottom - top, class: "chart-hit" }); if (bin.rows.length) { hit.setAttribute("role", "button"); hit.setAttribute("tabindex", "0"); hit.setAttribute("aria-label", clock(bin.a) + " to " + clock(bin.b) + ": " + asMoney(bin.claude + bin.codex)); }
      const open = () => { if (bin.rows.length) openAnalyticsSlice(bin.a, bin.b, bin.rows); }; hit.addEventListener("click", open); hit.addEventListener("keydown", (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); open(); } }); svg.append(hit);
    });
    svg.append(svgEl("text", { x: left, y: 178, class: "axis-label" }, analyticsRange === 1 ? "24 h ago" : analyticsRange + " d ago"), svgEl("text", { x: right, y: 178, "text-anchor": "end", class: "axis-label" }, "Now"));
    const chart = el("div", "analytics-chart"); chart.append(svg); panel.append(chart); const legend = el("div", "analytics-legend");
    for (const [h, label] of [["claude", "Claude"], ["codex", "Codex"]]) { const item = el("span"), swatch = el("i"); swatch.style.setProperty("--h", "var(--" + h + ")"); item.append(swatch, label); legend.append(item); } panel.append(legend);
    if (unknown.size) panel.append(el("div", "no-price", "no price for " + [...unknown].join(", ") + "; unpriced usage is omitted from bars.")); return panel;
  }
  function renderCodexAllowance() {
    const latest = Object.values(SESS).filter((s) => s.harness === "codex" && s.rate_limits?.recorded_at != null).sort((a, b) => b.rate_limits.recorded_at - a.rate_limits.recorded_at)[0]; if (!latest) return null;
    const limits = latest.rate_limits, panel = el("section", "analytics-panel"), grid = el("div", "allowance-grid"); panel.append(el("h2", null, "Codex allowance"), el("div", "panel-sub", "Latest recorded rate limits · " + new Date(limits.recorded_at).toLocaleString([], { hour: "numeric", minute: "2-digit" })));
    for (const limit of limits.windows ?? []) { const label = limit.minutes === 300 ? "5-hour window" : limit.minutes === 10080 ? "Weekly window" : limit.minutes + "-minute window", box = el("div", "allowance-window");
      box.append(el("div", "window-name", label), el("div", "window-used", limit.used_percent + "% used"), el("div", "window-reset", "Resets " + new Date(limit.resets_at).toLocaleString([], { weekday: "short", hour: "numeric", minute: "2-digit" }))); grid.append(box); }
    if (!grid.childElementCount) return null; panel.append(grid); return panel;
  }
  function openAnalyticsSlice(a, b, active) {
    const d = el("dialog", "viewer analytics-slice"), head = el("div", "vh"), title = el("div", "vt"), close = el("button", "vclose"), when = new Date(a).toLocaleString([], { weekday: "short", hour: "numeric", minute: "2-digit" }) + "–" + new Date(b).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
    d.setAttribute("aria-label", "Sessions busy " + when); title.append(el("span", null, "Sessions busy · " + when)); close.type = "button"; close.setAttribute("aria-label", "Close sessions list"); close.append(icon(I.x)); close.addEventListener("click", () => d.close()); head.append(title, close);
    const body = el("div", "vb"), list = el("div", "analytics-list"); if (!active.length) body.append(el("p", "empty", "No sessions were busy then."));
    for (const row of active.map((r) => ({ r, ms: busyMsIn(r, a, b) })).sort((x, y) => y.ms - x.ms || x.r.s.name.localeCompare(y.r.s.name))) { const s = row.r.s, item = el("button", "analytics-session analytics-slice"); item.type = "button"; item.append(harnessMark(s.harness), el("span", "session-name", s.name), el("span", "session-value", timeText(row.ms) + " busy")); item.addEventListener("click", () => { pendingSessionOpen = s.id; d.close(); }); list.append(item); }
    if (active.length) body.append(list); d.append(head, body); document.body.append(d); d.addEventListener("click", (e) => { if (e.target === d) d.close(); });
    d.addEventListener("close", () => { d.remove(); document.documentElement.classList.remove("viewer-open"); if (viewerEl === d) { viewerEl = null; if (history.state?.sheet) { skipPop = true; history.back(); } else if (pendingSessionOpen) { const id = pendingSessionOpen; pendingSessionOpen = null; goSession(id); } } });
    viewerEl = d; document.documentElement.classList.add("viewer-open"); d.showModal(); close.focus({ focusVisible: false }); try { history.pushState({ ...route, sheet: 1, scrollTop: currentScroll() }, ""); } catch {}
  }
  function analyticsBreakdown(title, rows, from, to, groupKey) {
    const groups = new Map(), keyFor = (s) => groupKey === "repo" ? s.repo ?? "__none__" : groupKey === "machine" ? s.machine : s.harness + "\u0000" + (s.model ?? s.modelId ?? "Unknown model");
    const labelFor = (key) => groupKey === "repo" ? key === "__none__" ? "No repo (roles)" : key : groupKey === "machine" ? MACHINE[key] ?? key : (HARNESS[key.split("\u0000")[0]] ?? key.split("\u0000")[0]) + " · " + shortModel(key.split("\u0000")[1]);
    for (const row of rows) { const ms = busyMsIn(row, from, to); if (!ms && !inRange(row.startedAt, from, to)) continue; const key = keyFor(row.s), g = groups.get(key) ?? { key, ms: 0, cost: 0, unknown: new Set(), sessions: new Set() }; g.ms += ms; g.sessions.add(row.id);
      if (inRange(row.costAt, from, to)) { const c = costForSession(row.id); g.cost += c.knownUsd; c.unknown.forEach((x) => g.unknown.add(x)); } groups.set(key, g); }
    const selected = (x) => analyticsMeasure === "cost" ? x.cost : x.ms, items = [...groups.values()].sort((a, b) => selected(b) - selected(a) || labelFor(a.key).localeCompare(labelFor(b.key))), max = Math.max(1, ...items.map(selected));
    const panel = el("section", "analytics-panel"); panel.append(el("h3", null, title)); const list = el("div", "analytics-list");
    for (const item of items) { const b = el("button", "analytics-row"); b.type = "button"; b.append(el("span", "row-title", labelFor(item.key)), el("span", "row-count", item.sessions.size + (item.sessions.size === 1 ? " session" : " sessions")));
      const measure = selected(item), track = el("span", "row-track"), bar = el("i", "row-bar"); bar.style.width = Math.max(measure ? 2 : 0, measure / max * 100) + "%"; if (groupKey === "harness") bar.style.background = item.key.startsWith("claude") ? "var(--claude)" : "var(--codex)"; track.append(bar);
      b.append(track, el("span", "row-hours" + (analyticsMeasure === "hours" ? " on" : ""), hoursText(item.ms)), el("span", "row-cost" + (analyticsMeasure === "cost" ? " on" : ""), item.unknown.size ? "—" : asMoney(item.cost))); if (item.unknown.size) b.append(el("span", "no-price", "no price for " + [...item.unknown].join(", ")));
      b.dataset.breakdown = groupKey; b.dataset.key = item.key;
      b.addEventListener("click", () => { if (groupKey === "repo") sessionFilters.repo = item.key; else if (groupKey === "machine") sessionFilters.machine = item.key; else { const [harness, model] = item.key.split("\u0000"); sessionFilters.harness = harness; sessionFilters.model = model; } query = ""; groupBy = "recent"; go({ v: "sessions" }); }); list.append(b); }
    if (!items.length) list.append(el("p", "empty", "No activity in this range.")); panel.append(list); return panel;
  }
  function analyticsList(title, items, value) {
    const panel = el("section", "analytics-panel"); panel.append(el("h2", null, title)); const list = el("div", "analytics-list"); if (!items.length) list.append(el("p", "empty", "No sessions in this range."));
    for (const item of items) { const s = item.s, b = el("button", "analytics-session"); b.type = "button"; b.append(harnessMark(s.harness), el("span", "session-name", s.name), el("span", "session-value", value(item))); if (item.cost?.unknown.length) b.append(el("span", "no-price", "no price for " + item.cost.unknown.join(", "))); b.addEventListener("click", () => goSession(s.id)); list.append(b); } panel.append(list); return panel;
  }
  function renderAnalytics(page) {
    const all = analyticsSessions().filter((row) => matchesSessionFacets(row.s)), to = NOW, from = to - rangeMs(analyticsRange), now = analyticsStats(all, from, to), previous = analyticsStats(all, from - rangeMs(analyticsRange), from);
    const head = el("div", "ph"), h1 = el("h1", null, "Analytics"); head.append(h1, el("div", "sub", "Measured activity · Last " + (analyticsRange === 1 ? "24 hours" : analyticsRange + " days"))); page.append(head); observeTitle(h1); page.append(renderFacetFilters(() => render()));
    const measure = el("div", "analytics-measure"); measure.setAttribute("role", "group"); measure.setAttribute("aria-label", "Breakdown bar measure");
    for (const [key, label] of [["hours", "Agent-hours"], ["cost", "API-equivalent cost"]]) { const b = el("button", null, label); b.type = "button"; b.setAttribute("aria-pressed", String(analyticsMeasure === key)); b.addEventListener("click", () => { if (analyticsMeasure === key) return; const top = currentScroll(); analyticsMeasure = key; render(); restoreScroll(top); }); measure.append(b); }
    const metrics = el("div", "analytics-metrics"), addMetric = (label, value, note, more, tip = false) => { const m = el("div", "analytics-metric"), l = el("div", "label"); l.append(el("span", null, label)); if (tip) l.append(costInfoTip()); if (more) { m.dataset.more = ""; m.title = more; } m.append(l, el("div", "value", value), note); metrics.append(m); }, pct = (errors, tools) => tools ? Math.round(errors / tools * 100) + "%" : "0%";
    addMetric("Agent-hours", hoursText(now.agentMs), deltaNote(now.agentMs, previous.agentMs, hoursText), "Busy time summed across sessions; two sessions busy for an hour count two hours.");
    const costNote = now.apiCost == null || previous.apiCost == null ? el("div", "note", "no price for " + [...new Set([...now.costUnknown, ...previous.costUnknown])].join(", ")) : deltaNote(now.apiCost, previous.apiCost, asMoney);
    addMetric("API-equivalent cost", now.apiCost == null ? "—" : asMoney(now.apiCost), costNote, null, true);
    addMetric("Sessions started", countText(now.started), deltaNote(now.started, previous.started, countText)); addMetric("Turns", countText(now.turns), deltaNote(now.turns, previous.turns, countText));
    addMetric("Tool calls", countText(now.tools), deltaNote(now.tools, previous.tools, countText), countText(now.errors) + " failed (" + pct(now.errors, now.tools) + ") · previous " + rangeName() + ": " + countText(previous.errors) + " failed (" + pct(previous.errors, previous.tools) + ")");
    addMetric("Peak concurrency", countText(peakBusy(all, from, to)), deltaNote(peakBusy(all, from, to), peakBusy(all, from - rangeMs(analyticsRange), from), countText), "The most sessions busy at the same moment.");
    addMetric("Waited on you", timeText(now.waitsMs), deltaNote(now.waitsMs, previous.waitsMs, timeText), "Median wait " + timeText(now.medianWaitMs) + " · previous " + rangeName() + ": " + timeText(previous.medianWaitMs));
    const currentWait = now.longestCurrent; addMetric("Longest current wait", currentWait ? timeText(currentWait.ms) : "—", deltaNote(currentWait ? currentWait.ms : 0, previous.longestWaitMs, timeText), currentWait ? currentWait.s.name + " has waited on you for " + timeText(currentWait.ms) : "No session is waiting on you"); page.append(metrics);
    const breakdowns = el("div", "analytics-breakdowns"); breakdowns.append(analyticsBreakdown("By repo", all, from, to, "repo"), analyticsBreakdown("By machine", all, from, to, "machine"), analyticsBreakdown("By harness and model", all, from, to, "harness"));
    const bdHead = el("div", "analytics-bd-head"), bdTitle = el("div"); bdTitle.append(el("h2", null, "Breakdown"), el("div", "panel-sub", "Agent-hours and API-equivalent cost; bars follow the toggle")); bdHead.append(bdTitle, measure);
    const busyTop = [...all].map((r) => ({ ...r, value: busyMsIn(r, from, to) })).filter((r) => r.value > 0).sort((a, b) => b.value - a.value).slice(0, 5), waitTop = [...now.waitBy].map(([id, ms]) => ({ s: SESS[id], id, value: ms })).filter((r) => r.s && r.value > 0).sort((a, b) => b.value - a.value).slice(0, 5);
    const costTop = all.filter((r) => inRange(r.costAt, from, to)).map((r) => { const cost = costForSession(r.id); return { ...r, cost, value: cost.usd }; }).sort((a, b) => (b.value ?? -1) - (a.value ?? -1)).slice(0, 5), bottom = el("div", "analytics-split");
    bottom.append(analyticsList("Top sessions · busy time", busyTop, (x) => timeText(x.value)), analyticsList("Top sessions · waited on", waitTop, (x) => timeText(x.value)), analyticsList("Most expensive sessions · API-equivalent cost", costTop, (x) => x.value == null ? "—" : asMoney(x.value)));
    page.append(renderAgentsChart(all, from, to), renderCostChart(all, from, to), bdHead, breakdowns, bottom); const allowance = renderCodexAllowance(); if (allowance) page.append(allowance);
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
    head.append(sub); page.append(head); observeTitle(h1);
    const fr = el("label", "find"); const fi = el("input"); fi.id = "sq"; fi.type = "search"; fi.placeholder = "Search sessions"; fi.setAttribute("aria-label", "Search sessions"); fi.value = query; fr.append(icon(I.search), fi);
    const gb = el("div", "groupby"); gb.setAttribute("role", "group"); gb.setAttribute("aria-label", "Group by");
    for (const [g, label] of [["recent", "Recent"], ["project", "Project"], ["machine", "Machine"], ["harness", "Harness"]]) { const b = el("button", null, label); b.type = "button"; b.dataset.g = g; b.addEventListener("click", () => { groupBy = g; draw(); }); gb.append(b); }
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
    fi.addEventListener("input", () => { query = fi.value.trim(); draw(); });
    page.append(renderFacetFilters(() => draw()), fr, gb, out); draw();
  }

  // ---- Drawer (phone) ---------------------------------------------------------------------------------------------------------
  const sidebar = $("#sidebar");
  function openDrawer() { if (!phone.matches) return; document.body.classList.add("drawer-open"); $("#lead-btn")?.setAttribute("aria-expanded", "true"); }
  function closeDrawer(quiet) { if (!document.body.classList.contains("drawer-open")) return; document.body.classList.remove("drawer-open"); closeAccountMenu(); const b = $("#lead-btn"); b?.setAttribute("aria-expanded", "false"); if (!quiet) b?.focus(); }
  $("#drawer-close").addEventListener("click", () => closeDrawer());
  $("#scrim").addEventListener("click", () => closeDrawer());
    document.addEventListener("keydown", (e) => { if (e.key === "Escape") { closeDrawer(); closeAccountMenu(); $(".session-menu")?.remove(); $("#more-btn")?.setAttribute("aria-expanded", "false"); closeFilter(); } if (e.key === "/" && !/INPUT/.test(document.activeElement?.tagName ?? "")) { e.preventDefault(); openDrawer(); $("#q").focus(); } });
  let sx = null;
  sidebar.addEventListener("touchstart", (e) => { sx = e.touches[0].clientX; }, { passive: true });
  sidebar.addEventListener("touchmove", (e) => { if (sx !== null && e.touches[0].clientX - sx < -50) { sx = null; closeDrawer(); } }, { passive: true });
  // The sidebar search narrows the Recent list as you type; Enter opens the Sessions page with the same query.
  $("#q").addEventListener("input", (e) => { query = e.target.value.trim(); renderLanes(); });
  $("#q").addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); query = e.target.value.trim(); go({ v: "sessions" }); } });
  phone.addEventListener("change", () => {
    closeDrawer(true); syncLayoutPrefs();
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
  const LIVE = { version: null, timer: null, busy: false, delay: 2000, ended: false, pending: false, fresh: 0, turns: new Map(), missing: new Set() };
  // The turn index as last drawn, to tell which turns an update changed.
  const remember = (m) => { LIVE.turns = new Map(m.turns.map((x) => [x.id, turnKey(x)])); };
  // What an update can change in a turn record or a handoff, cheaply (not the text, which a record never rewrites).
  const turnKey = (x) => [x.start, x.end?.st, x.end?.why, x.end?.h, x.sent.join(","), x.last ? 1 : 0].join("|");
  const handKey = (h) => [h.status, h.to, h.done, h.result?.length, h.answer?.length, h.answers?.length, h.declined ? 1 : 0].join("|");
  let rendered = null; // the route the page shows
  const visible = () => document.visibilityState === "visible";
  function schedule(ms) { clearTimeout(LIVE.timer); LIVE.timer = null; if (!LIVE.ended && visible()) LIVE.timer = setTimeout(poll, ms); }
  document.addEventListener("visibilitychange", () => {
    if (!visible()) { clearTimeout(LIVE.timer); LIVE.timer = null; } else if (LIVE.version && !LIVE.busy && !LIVE.timer) schedule(LIVE.delay > 2000 ? LIVE.delay : 0); }); // a backoff in progress holds
  function poll() {
    LIVE.timer = null; if (LIVE.busy || LIVE.ended || !visible()) return; LIVE.busy = true;
    fetch("/api/model?since=" + enc(LIVE.version ?? ""), { credentials: "same-origin" })
      .then((r) => r.status === 304 ? null : r.ok ? r.json() : Promise.reject(Object.assign(new Error(r.status + " " + r.statusText), { status: r.status })), (e) => Promise.reject(Object.assign(e, { status: 0 })))
      .then((m) => (m ? update(m) : null))
      .then(() => { LIVE.delay = 2000; }, (err) => {
        if (err?.status === 403) return ended();
        LIVE.delay = Math.min(30000, LIVE.delay * 2);
        if (err?.status == null) setTimeout(() => { throw err; }); // not the network: a fault on the page, reported as one
      })
      .finally(() => { LIVE.busy = false; schedule(LIVE.delay); });
  }
  function ended() {
    LIVE.ended = true; clearTimeout(LIVE.timer); LIVE.timer = null; if ($(".livenote")) return;
    const n = el("p", "livenote", "Session ended: reload with the printed URL"); n.setAttribute("role", "status"); document.body.append(n);
  }
  // A 403 or a dropped connection fails the update (and backs off); anything else skips that one transcript.
  const soft = (p) => p.catch((e) => { if (e?.status === 403 || e?.status === 0) throw e; });
  // The transcripts on screen: a session page's own, and its child runs'. Any other loaded transcript is dropped, so opening
  // it again loads it fresh.
  const viewed = () => { const v = new Set(); if (route.v !== "session") return v; v.add(route.id); for (const h of H) if (h.kind === "spawn" && h.from === route.id && h.to) v.add(h.to); return v; };
  function update(m) {
    const oldH = new Map(H.map((h) => [h.id, handKey(h)])), oldT = LIVE.turns, names = new Map(Object.values(SESS).map((x) => [x.id, x.name]));
    adopt(m); remember(m);
    const changedH = new Set(H.filter((h) => oldH.get(h.id) !== handKey(h)).map((h) => h.id));
    const view = viewed(), grown = new Set(), cuts = new Map();
    let full = Object.values(SESS).some((x) => names.has(x.id) && names.get(x.id) !== x.name); // a new name shows in every turn
    for (const sid of Object.keys(TX)) { if (view.has(sid) && SESS[sid]) spread(sid); else { delete TX[sid]; delete TXM[sid]; } }
    // Only transcripts whose mark in the model moved are asked for, one at a time.
    let chain = Promise.resolve();
    if (route.v === "analytics") chain = chain.then(loadAnalyticsCounts);
    // A mark with fewer entries or bytes than the one loaded means the file was cut or rewritten: load it again.
    const shrank = (a, b) => { const [s0, b0] = String(a).split(".").map(Number), [s1, b1] = String(b).split(".").map(Number); return s1 < s0 || b1 < b0; };
    for (const sid of view) if (TX[sid] && TXM[sid].tok != null && TOK[sid] != null && shrank(TXM[sid].tok, TOK[sid])) chain = chain.then(() => soft(reload(sid).then(() => { grown.add(sid); full = true; })));
    else if (TX[sid] && TXM[sid].to >= TXM[sid].total && TXM[sid].tok !== TOK[sid]) chain = chain.then(() => soft(tail(sid).then((r) => { grown.add(sid); if (r.cut != null) cuts.set(sid, r.cut); if (r.reload) full = true; })));
    if (route.v === "session" && TX[route.id]) chain = chain.then(() => newKids(route.id, grown));
    return chain.then(() => { LIVE.version = m.version; refresh(full ? null : dirtyTurns(cuts, grown, changedH, oldT)); });
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
    if (viewerEl) { LIVE.pending = true; return; } // drawn whole when the sheet closes
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
  const ANCHORS = "[data-e], .turn, .hop, .ib, .nrow, .sec-h, .ph, .divider, .analytics-metric, .analytics-panel, .facet-filters, .groupby, .find, .empty";
  const HOSTS = "[data-e], [data-h], [data-id], [data-sid], [data-go], [data-turn], [data-g], [data-m]";
  const FOCUSABLE = "button, input, [tabindex]";
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
    const keys = () => new Set([...$("#page").querySelectorAll(".turns :is(.msg, .step, .hcard, .think, .think-masked, .think-pending)[data-e]")].filter((n) => !n.closest(".cw-body")).map((n) => n.dataset.e));
    const before = keys();
    // Only the changed turns are drawn again, unless the turns shown no longer match the index or nothing was shown.
    const whole = !dirty || box.querySelector(":scope > p.empty") || [...box.querySelectorAll(":scope > .turn")].some((b) => !TURN.has(b.dataset.turn));
    if (whole) morph(box, transcript(route.id).querySelector(".turns"));
    else if (dirty.size) morphTurns(box, transcript(route.id, { only: dirty }).querySelector(".turns"), dirty);
    const h1 = $("#page .ph h1"); if (h1) h1.textContent = s.name;
    const t = $("#topbar .t"); if (t) { t.textContent = s.name; t.title = s.name; }
    const l2 = $("#topbar .l2"); if (l2) { l2.replaceChildren(); const hit = el("button", "meta-hit"); hit.type = "button"; hit.setAttribute("aria-label", s.name + ": open Session details"); hit.addEventListener("click", () => openSessionDetails(s)); l2.append(hit); sessionLine(s)(l2); requestAnimationFrame(() => { if (l2.isConnected) fitSessionLine(l2); }); }
    const fc = $("#topbar .fcount"); if (fc) { const n = find ? $("#page").querySelectorAll(".turns .msg, .turns .step, .turns .hcard").length : 0; fc.textContent = find ? (n ? n + (n === 1 ? " match" : " matches") : "No matches") : ""; }
    renderNav(); renderLanes(); ticker();
    let n = 0; for (const k of keys()) if (!before.has(k)) n++;
    return n;
  }
  // What a block shows, without what the reader toggled (open, hidden, measured clipping) or what opening fills in.
  const VIEW = new Set(["open", "clipped", "flash"]);
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

  const jumpButton = el("button", "jump-bottom"); jumpButton.type = "button"; jumpButton.id = "jump-bottom"; jumpButton.setAttribute("aria-label", "Jump to bottom of transcript"); jumpButton.hidden = true; document.body.append(jumpButton);
  function scrollMetrics() {
    if (phone.matches) return { top: window.scrollY, height: document.documentElement.scrollHeight, viewport: window.innerHeight, gap: Math.max(0, document.documentElement.scrollHeight - window.innerHeight - window.scrollY) };
    const m = $("#main"); return { top: m.scrollTop, height: m.scrollHeight, viewport: m.clientHeight, gap: Math.max(0, m.scrollHeight - m.clientHeight - m.scrollTop) };
  }
  function scrollToEnd(behavior = "smooth") { if (phone.matches) window.scrollTo({ top: document.documentElement.scrollHeight, behavior }); else { const m = $("#main"); m.scrollTo({ top: m.scrollHeight, behavior }); } }
  function syncJump() {
    if (route.v !== "session") { LIVE.fresh = 0; jumpButton.hidden = true; return; }
    const { gap } = scrollMetrics(); if (gap <= 80) LIVE.fresh = 0;
    jumpButton.hidden = gap <= 80; jumpButton.replaceChildren();
    if (LIVE.fresh) jumpButton.append(el("span", "new-count", LIVE.fresh + " new"));
    jumpButton.append(icon(I.down));
    jumpButton.setAttribute("aria-label", LIVE.fresh ? "Jump to bottom; " + LIVE.fresh + " new entries" : "Jump to bottom of transcript");
  }
  function clearNewEntries() { LIVE.fresh = 0; jumpButton.hidden = true; }
  jumpButton.addEventListener("click", () => scrollToEnd("smooth"));
  window.addEventListener("scroll", syncJump, { passive: true });
  $("#main").addEventListener("scroll", syncJump, { passive: true });

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
