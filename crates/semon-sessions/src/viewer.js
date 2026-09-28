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
  const HARNESS = { claude: "Claude Code", codex: "Codex" };
  const SESS = {};
  const H = [];
  const TX = {};

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
  };
  const clock = (t) => { const d = new Date(t), n = new Date(NOW); const hm = String(d.getHours()).padStart(2, "0") + ":" + String(d.getMinutes()).padStart(2, "0"); return d.toDateString() === n.toDateString() ? hm : d.toLocaleDateString(undefined, { weekday: "short" }) + " " + hm; };
  const ago = (t) => { const d = Math.floor((NOW - t) / 60000); return d < 1 ? "now" : d < 60 ? d + "m" : d < 2880 ? Math.floor(d / 60) + "h" : Math.floor(d / 1440) + "d"; };
  const dur = (a, b) => { const d = Math.max(0, Math.floor(((b ?? NOW) - a) / 60000)); return d >= 1440 ? Math.floor(d / 1440) + "d " + Math.floor((d % 1440) / 60) + "h" : d >= 60 ? Math.floor(d / 60) + "h " + (d % 60) + "m" : d + "m"; };
  const tok = (m) => m >= 1 ? m.toFixed(1) + "M" : Math.round(m * 1000) + "k";
  const dot = (st) => { const d = el("span", "dot " + st); d.title = STATE[st] ?? st; d.setAttribute("role", "img"); d.setAttribute("aria-label", d.title); return d; };
  const STATE = { work: "Working", wait: "Needs you", idle: "Idle", done: "Done", err: "Failed" };
  const nameOf = (id) => id === "you" ? "You" : SESS[id].name;
  const hcls = (id) => id === "you" ? "h-you" : "h-" + SESS[id].harness;
  const where = (s) => s.repo ? s.repo + (s.branch && s.branch !== "main" && s.branch !== s.name ? " · " + s.branch : "") : "No repo";
  const facetLine = (s) => [s.kind ?? HARNESS[s.harness], MACHINE[s.machine], where(s)].join(" · ");
  const parentOf = (sid) => H.find((h) => h.kind === "spawn" && h.to === sid)?.from;
  const RANK = { wait: 0, work: 1, err: 2, done: 3 };
  const inbox = () => H.filter((h) => h.kind === "toyou" && h.status === "wait").sort((a, b) => b.at - a.at);
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
  const statWord = (h) => ({ work: "working", wait: "waiting on you", err: "failed", done: h.kind === "toyou" ? "answered" : h.result ? "returned" : "delivered" })[h.status];

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
  let serverNow = 0, fetchedAt = 0;
  let TOK = {}; // per session: its transcript's growth mark in the model; a loaded transcript is tailed only when it moved
  const enc = encodeURIComponent;
  // An error carries the HTTP status (0: no response), so live polling can tell a 403 from a dropped connection.
  const api = (path) => fetch(path, { credentials: "same-origin" }).then((r) => { if (!r.ok) throw Object.assign(new Error(r.status + " " + r.statusText), { status: r.status }); return r.json(); }, (e) => { throw Object.assign(e, { status: 0 }); });
  // NOW follows the client clock from the model's `now`, so every "ago" keeps moving; a running tool's age follows NOW.
  function tick() {
    NOW = serverNow + (Date.now() - fetchedAt);
    for (const s of Object.values(SESS)) if (s.activity && s.activity[3] != null) s.activity[2] = Math.floor((NOW - s.activity[3]) / 1000);
  }
  function adopt(m) {
    serverNow = m.now; fetchedAt = Date.now(); TOK = m.tx ?? {};
    for (const k of Object.keys(MACHINE)) { delete MACHINE[k]; delete MACHINE_UP[k]; delete MACHINE_LAST[k]; }
    // Several machines come as `machines`; one comes as `machine` alone.
    for (const x of m.machines ?? [m.machine]) { MACHINE[x.id] = x.name; MACHINE_UP[x.id] = x.up; if (x.last != null) MACHINE_LAST[x.id] = x.last; }
    ADMIN = m.admin && typeof m.admin.href === "string" && m.admin.href.startsWith("/") && !m.admin.href.startsWith("//") ? m.admin : null;
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
      const t = { id: x.id, sid: x.sid, start: x.start ? HID.get(x.start) ?? null : null, u: x.u ? { k: "u", text: x.text ?? "" } : null, entries: [], out: [], sent: [], end: x.end };
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
    if (r.v !== "session" || !SESS[r.id]) return null;
    const t = r.turn ? TURN.get(r.turn) : null, deep = t && t.sid === r.id && !t.entries.length;
    if (TX[r.id] && !deep) return kids(r.id);
    return fetchTx(r.id, deep ? "turn=" + enc(t.id) : "").then(() => kids(r.id));
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
    return r.v === "home" ? "/" : r.v === "timeline" ? "/timeline" : r.v === "sessions" ? "/sessions" : r.v === "machines" ? "/machines"
      : r.v === "machine" ? "/machines/" + enc(r.id)
      : r.v === "session" ? "/s/" + hs(r.id) + "/" + enc(r.id) + (r.turn ? "?turn=" + enc(r.turn) : "")
      : "/trace/" + hs(r.sid) + "/" + enc(r.sid) + "/" + enc(r.turn);
  }
  function routeOf(loc) {
    const p = loc.pathname.split("/").filter(Boolean).map((x) => { try { return decodeURIComponent(x); } catch { return x; } }), turn = new URLSearchParams(loc.search).get("turn");
    if (p[0] === "timeline") return { v: "timeline" };
    if (p[0] === "sessions") return { v: "sessions" };
    if (p[0] === "machines") return p[1] && MACHINE[p[1]] ? { v: "machine", id: p[1] } : { v: "machines" };
    if (p[0] === "s" && SESS[p[2]]) return turn ? { v: "session", id: p[2], turn } : { v: "session", id: p[2] };
    if (p[0] === "trace" && SESS[p[2]] && p[3]) return { v: "trace", sid: p[2], turn: p[3] };
    return { v: "home" };
  }
  function boot() {
    api("/api/model").then((m) => {
      adopt(m); route = routeOf(location); LIVE.version = m.version; remember(m);
      try { history.replaceState(route, "", urlOf(route)); } catch {}
      const done = () => { render(); if (route.v === "session" && route.turn) revealTurn(route.turn, true); schedule(2000); setInterval(ticker, 1000); };
      const p = load(route); if (p) p.then(done, done); else done();
    }, (err) => { $("#page").replaceChildren(el("p", "empty", "Couldn't load the sessions: " + err.message)); });
  }

  // ---- State & navigation ---------------------------------------------------------------
  const phone = window.matchMedia("(max-width: 760px)");
  let route = { v: "home" }; let groupBy = "recent"; let query = "";
  let show = { messages: true, tools: true, thinking: true }; let find = ""; let findOpen = false; let filterOpen = false;
  const quietTop = () => { if (phone.matches) window.scrollTo(0, 0); else $("#main").scrollTop = 0; };
  function go(r, fromHistory) {
    route = r; find = ""; findOpen = false; filterOpen = false; closeDrawer(true); $(".menu")?.remove(); clearPill();
    if (!fromHistory) { try { history.pushState(r, "", urlOf(r)); } catch {} }
    if (r.v === "timeline" && !fromHistory) tlView.left = null;
    const done = () => { if (route !== r) return; render(); if (r.v === "session" && r.turn) revealTurn(r.turn, !fromHistory); else quietTop(); };
    const p = load(r); if (p) p.then(done, done); else done();
  }
  window.addEventListener("popstate", (e) => {
    if (skipPop) { skipPop = false; return; }                        // our own history.back() after closing the viewer
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
      if (phone.matches) window.scrollTo(0, Math.max(0, window.scrollY + b.getBoundingClientRect().top - gap)); else { const m = $("#main"); m.scrollTop += b.getBoundingClientRect().top - m.getBoundingClientRect().top - gap; } };
    place(); requestAnimationFrame(() => requestAnimationFrame(place));
    if (flash) { b.classList.add("flash"); setTimeout(() => b.classList.remove("flash"), 1500); }
  }

  // ---- Sidebar ----------------------------------------------------------------------------------
  function renderNav() {
    const nav = $("#nav"); nav.replaceChildren();
    // A session or a trace sits under Sessions, a machine under Machines.
    const under = { home: ["home"], timeline: ["timeline"], sessions: ["sessions", "session", "trace"], machines: ["machines", "machine"] };
    const item = (v, label, ic, count, hot) => { const b = el("button", "nav-item"); b.type = "button"; b.dataset.go = v; if (under[v].includes(route.v)) b.setAttribute("aria-current", "page"); b.append(icon(ic, "icon"), el("span", null, label)); if (count) b.append(el("span", "cnt" + (hot ? " hot" : ""), String(count))); b.addEventListener("click", () => go({ v })); nav.append(b); };
    item("home", "Home", I.home, inbox().length, true);
    item("timeline", "Timeline", I.now);
    item("sessions", "Sessions", I.sessions, Object.values(SESS).filter((s) => s.lane).length);
    item("machines", "Machines", I.machine, Object.keys(MACHINE).filter((m) => !MACHINE_UP[m]).length, true);
  }
  // Sessions match by name, repo, branch, machine, harness and the messages that started their turns.
  const sessMatch = (s, q) => !q || [s.name, s.repo, s.branch, MACHINE[s.machine], s.movedFrom ? MACHINE[s.movedFrom] : "", HARNESS[s.harness], s.role ? "role no repo" : "", ...(TURNS[s.id] ?? []).map((t) => t.start?.brief ?? t.u?.text ?? "")].join(" ").toLowerCase().includes(q.toLowerCase());
  // The sidebar keeps a short list for quick switching: the 8 most recently active sessions (those matching the search
  // box while something is typed there), then a link to the Sessions page. Enter in the search box opens that page.
  function renderLanes() {
    const lanes = Object.values(SESS).filter((s) => s.lane && sessMatch(s, query)).sort((a, b) => b.last - a.last);
    const box = $("#lanes"); box.replaceChildren();
    for (const s of lanes.slice(0, 8)) {
      const r = el("button", "srow"); r.type = "button"; r.dataset.id = s.id;
      if ((route.v === "session" && (route.id === s.id || parentOf(route.id) === s.id)) || (route.v === "trace" && route.sid === s.id)) r.setAttribute("aria-current", "page");
      r.append(dot(s.state), el("span", "nm", s.name), el("span", "ag", ago(s.last)), el("span", "fx", [HARNESS[s.harness], MACHINE[s.machine], s.repo ?? "no repo"].join(" · ")));
      r.addEventListener("click", () => goSession(s.id)); box.append(r);
    }
    if (!lanes.length) box.append(el("p", "ghead", "No sessions match"));
    const all = el("button", "side-all", query ? "All matching sessions ›" : "All sessions ›"); all.type = "button"; all.id = "all-sessions"; all.addEventListener("click", () => go({ v: "sessions" })); box.append(all);
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
    if (s && findOpen) { searchBar(bar); return; }
    const m = el("button", "ibtn lead"); m.id = "lead-btn"; m.type = "button"; m.setAttribute("aria-label", "Open navigation"); m.setAttribute("aria-controls", "sidebar"); m.setAttribute("aria-expanded", "false"); m.append(icon(I.menu)); m.addEventListener("click", openDrawer); bar.append(m);
    const t = el("div", "ttl"), l1 = el("div", "l1");
    if (s) { const hit = el("button", "hit"); hit.type = "button"; hit.setAttribute("aria-label", s.name + ": details"); hit.addEventListener("click", (e) => { e.stopPropagation(); toggleMenu(s, $("#more-btn")); }); t.append(hit); }
    if (crumb) { const c = el("button", "crumb", crumb.label); c.type = "button"; c.setAttribute("aria-label", "Back to " + crumb.label); c.addEventListener("click", crumb.go); l1.append(c, el("span", "sep", "›")); }
    const tt = el("span", "t", title); tt.title = title; l1.append(tt); t.append(l1);
    if (opts.line2) { const l2 = el("div", "l2"); opts.line2(l2); t.append(l2); }
    bar.append(t);
    if (!s) return;
    const fb = el("button", "ibtn"); fb.type = "button"; fb.setAttribute("aria-label", "Find in transcript"); fb.append(icon(I.search));
    fb.addEventListener("click", () => { findOpen = true; filterOpen = false; render(); $("#find")?.focus(); });
    const pop = el("div", "filters pop"); pop.hidden = !filterOpen;
    for (const [key, label] of [["messages", "Messages"], ["tools", "Tool steps"], ["thinking", "Thinking"]]) { const l = el("label"); const cb = el("input"); cb.type = "checkbox"; cb.checked = show[key]; cb.id = "f-" + key; cb.addEventListener("change", () => { show[key] = cb.checked; render(); }); l.append(cb, label); pop.append(l); }
    const tb = el("button", "ibtn" + (show.messages && show.tools && show.thinking ? "" : " on")); tb.id = "filter-btn"; tb.type = "button"; tb.setAttribute("aria-label", "Filter transcript"); tb.setAttribute("aria-expanded", String(filterOpen)); tb.append(icon(I.filter));
    tb.addEventListener("click", () => { $(".menu")?.remove(); $("#more-btn")?.setAttribute("aria-expanded", "false"); filterOpen = pop.hidden; pop.hidden = !filterOpen; tb.setAttribute("aria-expanded", String(filterOpen)); });
    const more = el("button", "ibtn"); more.id = "more-btn"; more.type = "button"; more.setAttribute("aria-label", "Session details and actions"); more.setAttribute("aria-expanded", "false"); more.append(icon(I.more)); more.addEventListener("click", (e) => { e.stopPropagation(); toggleMenu(s, more); });
    // The dropdown hangs from the filter button's right edge, wherever the bar's padding and the buttons after it put it.
    const place = () => { pop.style.right = Math.max(0, bar.getBoundingClientRect().right - tb.getBoundingClientRect().right) + "px"; };
    tb.addEventListener("click", place);
    bar.append(fb, tb, more, pop); if (filterOpen) place();
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
  // A session's summary line: state, errors (a jump to the first failed step), turns and calls, harness and model, machine, repo.
  const sessionLine = (s) => (l2) => {
    const es = TX[s.id] ?? [], m = TXM[s.id], calls = m ? m.calls : es.filter((e) => e.k === "tool").length, errors = m ? m.errors : es.filter((e) => e.k === "tool" && e.ok === false).length, nT = (TURNS[s.id] ?? []).filter(hasTurn).length;
    const st = el("span", "stat " + s.state); st.append(dot(s.state), STATE[s.state]); l2.append(st);
    if (errors) { const j = el("button", "errs", errors + (errors === 1 ? " error" : " errors")); j.type = "button"; j.setAttribute("aria-label", j.textContent + ": jump to the first failed step");
      j.addEventListener("click", (ev) => { ev.stopPropagation(); const e = $(".step.err"); const gs = e?.closest(".tgroup")?.querySelector(".tsum"); if (gs?.getAttribute("aria-expanded") === "false") gs.click(); if (e) { e.scrollIntoView({ behavior: "smooth", block: "center" }); const t = e.querySelector("button"); if (t?.getAttribute("aria-expanded") === "false") t.click(); } });
      l2.append(el("span", "sep", " · "), j); }
    l2.append(el("span", "sep", " · "), el("span", "rest", [nT + (nT === 1 ? " turn" : " turns"), calls + (calls === 1 ? " call" : " calls"), (s.kind ? s.kind + " · " : "") + HARNESS[s.harness] + " · " + s.model, MACHINE[s.machine] + (s.movedFrom ? " (moved from " + MACHINE[s.movedFrom] + ")" : ""), s.repo ? where(s) : "No repo"].join(" · ")));
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
  function toggleMenu(s, btn) {
    const ex = $(".menu"); if (ex) { ex.remove(); btn.setAttribute("aria-expanded", "false"); return; }
    const m = el("div", "menu"); m.setAttribute("role", "menu");
    const copy = el("button"); copy.type = "button"; copy.append(icon(I.copy, "icon"), el("span", null, "Copy resume command"));
    const cmd = s.harness === "codex" ? "codex resume " + s.id : "claude --resume " + s.id;
    copy.addEventListener("click", () => { navigator.clipboard?.writeText(cmd).then(() => { copy.lastChild.textContent = "Copied"; }, () => { copy.lastChild.textContent = cmd; }); });
    m.append(copy);
    if (s.harness === "claude") { const a = el("button"); a.type = "button"; a.append(icon(I.ext, "icon"), el("span", null, "Open in claude.ai")); m.append(a); }
    const dl = el("dl");
    for (const [k, v] of [["Model", s.model], ["Machine", MACHINE[s.machine] + (s.movedFrom ? " (moved from " + MACHINE[s.movedFrom] + ")" : "")], ["Started", clock(s.start)], ["Duration", dur(s.start, s.state === "work" ? null : s.last)], ["Tokens in / out", tok(s.tokens[0]) + " / " + tok(s.tokens[2])], ["Cached context", tok(s.tokens[1])], ["Session id", s.id]]) dl.append(el("dt", null, k), el("dd", "mono", v));
    m.append(dl); closeFilter(); $("#topbar").append(m); btn.setAttribute("aria-expanded", "true");
  }
  document.addEventListener("click", (e) => { const m = $(".menu"); if (m && !m.contains(e.target)) { m.remove(); $("#more-btn")?.setAttribute("aria-expanded", "false"); }
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
    r.append(icon(quiet && h.kind === "toyou" ? I.done : ic)); const ln = el("span", "ln"); ln.append(...parts); r.append(ln, el("span", "tm", ago(h.at)));
    r.append(rich("span", "q", preview(h.brief)));
    const an = quiet ? answersOf(h) : null; if (an) r.append(el("span", "ans", an.length ? "You answered: " + an.join(" · ") : "Answered · reply not in these logs"));
    if (t) r.append(originLine(t));
    const ctx = el("span", "ctx"); const s = SESS[sid];
    ctx.append(el("span", null, [HARNESS[s.harness], MACHINE[s.machine]].join(" · "))); if (t?.out.length) ctx.append(traceBtn(t));
    r.append(ctx);
    const open = () => goSession(sid, t?.id);
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
    const flow = el("div", "flow"), seen = new Set([root.id]), sess = new Set([root.sid]); let n = 0;
    const hop = (cls, ic, hc, parts, at) => { const x = el("div", "hop " + cls); const node = el("div", "node " + hc); node.append(icon(ic)); const body = el("div", "body"); const sent = el("div", "sent"); sent.append(...parts); if (at != null) sent.append(el("span", "tm", clock(at))); body.append(sent); x.append(node, body); flow.append(x); return [x, body]; };
    const s0 = root.start, text = s0 ? s0.brief : root.u?.text;
    const [ic0, parts0] = s0 ? sentence(s0, null) : root.u ? sentence({ kind: "ask", to: root.sid }, null) : [I.more, [el("span", "who", SESS[root.sid].name), el("span", "verb", " · a turn whose start isn't in these logs")]];
    const [r0, b0] = hop("k-root", ic0, hcls(s0 ? s0.from : root.u ? "you" : root.sid), parts0, s0?.at); r0.dataset.turn = root.id; if (s0) r0.dataset.h = s0.id;
    if (text) clampBrief(b0, text);
    const e0 = turnEnd(root); traceMeta(b0, e0?.st ?? "idle", e0?.text ?? "Nothing recorded", root.sid, root);
    const walk = (t) => {
      for (const h of t.sent) {
        const c = h.kind === "spawn" || h.kind === "relay" ? STARTS.get(h.id) : null, tgt = h.kind === "toyou" ? h.from : h.to, [ic, parts] = sentence(h, null);
        const [x, b] = hop("child k-" + h.kind + " s-" + h.status + (c || h.kind === "toyou" || h.kind === "move" ? "" : " stub"), ic, hcls(tgt), parts, h.at); x.dataset.h = h.id; if (c) x.dataset.turn = c.id;
        clampBrief(b, h.brief); const an = answerEl(h, "result"); if (an) b.append(an);
        if (h.result) { const r = el("div", "result"); r.append(el("span", "rl", "Result:")); const s = el("span"); inline(s, h.result); r.append(s); b.append(r); }
        if (h.kind === "move") { traceMeta(b, "done", "Moved", h.to, t); continue; }
        n++;
        if (h.kind === "toyou") { traceMeta(b, h.status === "done" ? "done" : h.status, statWord(h), h.from, t); continue; }
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
    const head = el("div", "ph sr"); const h1 = el("h1", null, SESS[sid].name); head.append(h1); page.append(head); observeTitle(h1);
    page.append(transcript(sid));
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
    const sec = el("section", opts.nested ? "nested" : null); sec.setAttribute("aria-label", opts.nested ? SESS[sid].name + " transcript" : "Transcript"); Object.assign(sec.style, { display: "grid", gap: "10px", gridTemplateColumns: "minmax(0, 1fr)" });
    const entries = opts.entries ?? TX[sid] ?? [];
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
    const firsts = turnMode ? new Map((TURNS[sid] ?? []).map((t) => [t.entries[0], t])) : new Map(); let cur = null;
    // A live update draws only the turns that changed (opts.only, by turn id).
    const owner = opts.only ? new Map((TURNS[sid] ?? []).flatMap((t) => t.entries.map((e) => [e, t.id]))) : null;
    const closeTurn = () => { flush(); if (!cur) return; const { t, blk } = cur; cur = null; tx = box;
      if ((find || !show.messages || !show.tools || !show.thinking) && !blk.querySelector(".msg, .step, .hcard")) { blk.remove(); return; }
      const end = turnEnd(t); if (!end) return;
      const d = el("div", "turn-end"); const st = el("span", "stat " + end.st); st.append(end.st === "work" ? el("span", "spin") : dot(end.st), end.text); d.append(st);
      if (t.out.length) d.append(traceBtn(t)); blk.append(d); };
    const openTurn = (t) => { closeTurn(); const blk = el("section", "turn"); blk.dataset.turn = t.id;
      // Your own message needs no header: the bubble is yours and its time sits under it. A relay or brief says who sent it.
      const h = t.start;
      if (t.u || h?.kind === "ask") blk.setAttribute("aria-label", "Your message" + (h ? " at " + clock(h.at) : ""));
      else if (h) { const hd = el("h3", "turn-h " + hcls(h.from)); const l = el("span", "lbl"); const b = el("button", "from", nameOf(h.from)); b.type = "button"; b.setAttribute("aria-label", "Open " + nameOf(h.from) + " where it sent this"); b.addEventListener("click", () => openSender(h)); l.append(el("span", "verb", h.kind === "relay" ? "Relay from " : "Brief from "), b); hd.append(icon(I.in), l, el("span", "tm", clock(h.at))); blk.append(hd); }
      tx = el("div", "tx"); blk.append(tx); box.append(blk); cur = { t, blk }; };
    for (const e of entries) {
      if (owner && !opts.only.has(owner.get(e))) continue;
      if (turnMode && isGap(e)) { closeTurn(); if (!find) box.append(el("div", "divider", e.text)); continue; }
      if (firsts.has(e)) openTurn(firsts.get(e));
      // Entries that render nothing (empty thinking, hidden kinds) must not split a run of tool calls.
      if (e.k === "think" && (!show.thinking || find || (!e.secs && !e.text))) continue;
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
      else if (e.k === "think") { if (!show.thinking || find || (!e.secs && !e.text)) continue; /* Claude Code stores most thinking empty: no marker without content or a duration */ const t = el("button", "think"); t.type = "button"; t.append(el("span", null, e.secs != null ? "Thought for " + e.secs + "s" : "Thought"), icon(I.chev, "chev")); tx.append(keyed(t, e)); }
      else if (e.k === "harness") { if (!show.messages || find) continue; tx.append(keyed(el("div", "harness-note", "Harness text added before the prompt (" + e.label + ")"), e)); }
      else if (e.k === "end") { if (find) continue; tx.append(keyed(el("div", "divider", e.text), e)); }
      else if (e.k === "h") {
        const h = H.find((x) => x.id === e.id); if (!hit(h.brief + " " + (h.result ?? ""))) continue;
        // Your own ask is simply your message.
        if (h.kind === "ask") { if (!show.messages) continue; const m = keyed(el("div", "msg user"), e); m.append(markdown(h.brief)); tx.append(m); if (cur?.t.start === h) tx.append(el("div", "msg-tm", clock(h.at))); continue; }
        // A relay or brief that starts a turn is that turn's message.
        if (cur && cur.t.start === h && e === cur.t.entries[0]) { if (!show.messages) continue; tx.append(keyed(handoffCard(h, sid, true), e)); continue; }
        if (h.kind === "move") { if (find) continue; tx.append(keyed(handoffCard(h, sid), e)); continue; }
        if (!show.tools && h.kind !== "toyou") continue;
        if (opts.nested && h.kind === "spawn" && h.to === sid) continue; // the parent's card already shows this brief
        tx.append(keyed(handoffCard(h, sid), e));
        // A subagent's or Codex run's own work opens inline under its handoff: the turn that brief started there.
        const cw = turnMode && h.kind === "spawn" && h.from === sid ? STARTS.get(h.id)?.entries ?? TX[h.to] ?? [] : [];
        if (cw.length) {
          const n = cw.filter((x) => x.k === "tool").length;
          const w = el("div", "childwork"); if (e.key) w.dataset.e = "cw:" + e.key; const lastCw = cw.at(-1);
          // What the child did, as far as loaded (a change redraws its turn): how many entries, the last one and its state, and how
          // many calls are running or failed, so a parallel call that finishes earlier in the child counts too.
          w.dataset.sum = [cw.length, lastCw.key, lastCw.live ? "live" : lastCw.unfinished ? "open" : String(lastCw.ok), cw.filter((x) => x.live).length, cw.filter((x) => x.ok === false).length].join(" ");
          const b = el("button", "cw-toggle"); b.type = "button"; b.setAttribute("aria-expanded", "false");
          b.append(icon(I.chev, "chev"), el("span", null, "What " + SESS[h.to].name + " did" + (n ? " · " + n + (n === 1 ? " tool call" : " tool calls") : "")));
          const inner = el("div", "cw-body"); inner.hidden = true;
          b.addEventListener("click", () => { if (!inner.childElementCount) inner.append(transcript(h.to, { nested: true, entries: cw })); inner.hidden = !inner.hidden; b.setAttribute("aria-expanded", String(!inner.hidden)); });
          w.append(b, inner); tx.append(w);
        }
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

  function handoffCard(h, viewer, start) {
    const other = h.kind === "move" ? null : viewer === h.from ? h.to : h.from;
    const c = el("div", "hcard " + (h.kind === "toyou" ? "toyou" : h.kind === "move" ? "move" : hcls(other)) + (start ? " start" : "")); c.dataset.h = h.id; c.tabIndex = 0; c.setAttribute("role", "link");
    const [ic, parts] = sentence(h, viewer);
    c.append(icon(ic)); const ln = el("span", "ln"); ln.append(...parts); c.append(ln);
    const sw = el("span", "stat " + h.status); sw.append(h.status === "work" ? el("span", "spin") : dot(h.status === "done" ? "done" : h.status), statWord(h)); c.append(sw);
    const br = markdown(h.brief, "brief"); c.append(br);
    // Long messages open in place; the rest of the card still goes to the other session.
    const more = el("button", "more", "Show more"); more.type = "button"; more.hidden = true; more.setAttribute("aria-expanded", "false");
    more.addEventListener("click", (ev) => { ev.stopPropagation(); const open = c.classList.toggle("open"); more.textContent = open ? "Show less" : "Show more"; more.setAttribute("aria-expanded", String(open)); });
    new ResizeObserver(() => { if (c.classList.contains("open") || !br.clientHeight) return; const x = br.scrollHeight > br.clientHeight + 1; more.hidden = !x; br.classList.toggle("clipped", x); }).observe(br);
    c.append(more);
    if (h.result) { const r = el("span", "result"); r.append(el("b", null, (h.status === "err" ? "Failed" : "Returned") + ": ")); inline(r, h.result); c.append(r); }
    const an = answerEl(h, "result"); if (an) c.append(an);
    // Received: the sender's turn that sent it. Sent on: the turn it started there. To you: this turn's trace. A move: the machine it left.
    const open = () => { if (h.kind === "move") go({ v: "machine", id: h.fromMachine }); else if (h.kind === "toyou") { const t = HOLDS.get(h.id); if (t) goTrace(t.id); } else if (viewer === h.to) openSender(h); else if (SESS[other]) goSession(other, STARTS.get(h.id)?.id); };
    c.addEventListener("click", (ev) => { if (!getSelection().isCollapsed) return; open(); });
    c.addEventListener("keydown", (ev) => { if (ev.target === c && (ev.key === "Enter" || ev.key === " ")) { ev.preventDefault(); open(); } });
    return c;
  }

  // ---- Render --------------------------------------------------------------------------------------------------------
  function render() {
    // The page first, then the bar: the bar's summary (a trace's counts, a search's matches) comes from the page.
    tick(); const page = $("#page"), r = route; rendered = r; page.style.paddingBottom = ""; page.replaceChildren(); page.classList.toggle("wide", r.v === "timeline");
    if (r.v === "home") { renderHome(page); renderTopbar("Home"); }
    else if (r.v === "timeline") { renderTimeline(page); renderTopbar("Timeline"); }
    else if (r.v === "sessions") { renderSessions(page); renderTopbar("Sessions"); }
    else if (r.v === "machines") { renderMachines(page); renderTopbar("Machines"); }
    else if (r.v === "machine") { renderMachine(page, r.id); renderTopbar(MACHINE[r.id], { label: "Machines", go: () => go({ v: "machines" }) }, { line2: machineLine(r.id) }); }
    else if (r.v === "trace") { const sum = renderTrace(page, r.turn) ?? ""; renderTopbar("Trace", { label: SESS[r.sid].name, go: () => goSession(r.sid, r.turn) }, { line2: (l2) => l2.append(el("span", "rest", sum)) }); }
    else if (r.v === "session") { const p = parentOf(r.id), s = SESS[r.id]; renderSession(page, r.id); renderTopbar(s.name, p ? { label: SESS[p].name, go: () => goSession(p) } : null, { session: s, line2: sessionLine(s) }); }
    document.documentElement.style.setProperty("--barh", $("#topbar").offsetHeight + "px");
    syncBarLine(); renderNav(); renderLanes();
  }

  // ---- Timeline: sessions as rows, time across ------------------------------------------------------------------------
  // Each top-level session is a row with its subagents and Codex runs indented beneath it. Bars are the session's busy
  // intervals (log activity clustered at 5 minutes); marks are handoffs. Time runs left to right, but a stretch over 2
  // hours with nothing anywhere collapses to a 28 px break, so days of logs fit. The labels stay put and only the chart
  // scrolls sideways; its axis follows it. Every position comes from one time-to-x map, so rows, marks and axis agree.
  const MIN = NOW > 1e11 ? 60000 : 1; // the real data counts epoch milliseconds, the sample minutes
  const HOUR = 60 * MIN;
  const two = (x) => String(x).padStart(2, "0");
  const hhmm = (t) => MIN === 1 ? clock(t) : two(new Date(t).getHours()) + ":" + two(new Date(t).getMinutes());
  const span = (m) => m >= 1440 ? Math.floor(m / 1440) + "d" + (Math.round((m % 1440) / 60) ? " " + Math.round((m % 1440) / 60) + "h" : "") : m >= 60 ? Math.round(m / 60) + "h" : Math.round(m) + "m";
  // A relay's gist: its brief on one line, less a leading "<Sender>:" or "<Sender> here:" that the label already says.
  const escRe = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const unsigned = (h) => { const name = SESS[h.from]?.name; return name ? h.brief.replace(new RegExp("^\\s*" + escRe(name) + "( here)?:\\s*", "i"), "") : h.brief; };
  // One sender relaying the same text to several sessions within 2 minutes is a broadcast: drawn once, not per
  // recipient. Same text means the same first 80 characters once whitespace is normalised and that prefix dropped.
  function groupBroadcasts(evs) {
    const out = [], open = new Map();
    for (const e of evs) {
      if (e.k !== "relay") { out.push(e); continue; }
      const key = e.h.from + "\u0000" + unsigned(e.h).replace(/\s+/g, " ").trim().slice(0, 80), g = open.get(key);
      if (g && e.t - g.t <= 2 * MIN && !g.hs.some((x) => x.to === e.h.to)) { g.hs.push(e.h); continue; }
      const ng = { ...e, hs: [e.h] }; open.set(key, ng); out.push(ng);
    }
    return out;
  }
  const ZOOM = [15, 30, 60, 120, 240]; // px per hour
  const tlView = { pph: 60, left: null, centre: null, rel: true, open: new Map() };
  // A session's bars: its busy intervals; while it works, the last one runs to now.
  const busyOf = (s) => { const iv = (s.busy ?? []).map(([a, b]) => [a, b]); if (s.state === "work" && iv.length) iv.at(-1)[1] = Math.max(iv.at(-1)[1], NOW); return iv; };
  const turnAt = (sid, t) => (TURNS[sid] ?? []).filter((x) => x.start?.at != null && x.start.at <= t).at(-1);
  // The time-to-x map: busy intervals and handoff times, merged; gaps over 2 hours become 28 px breaks.
  function tlMap(pph) {
    const pts = []; for (const s of Object.values(SESS)) pts.push(...busyOf(s));
    for (const h of H) { if (h.at != null) pts.push([h.at, h.at]); if (h.done != null) pts.push([h.done, h.done]); }
    pts.push([NOW, NOW]); pts.sort((p, q) => p[0] - q[0]);
    const segs = []; for (const [a, b] of pts) { const l = segs.at(-1); if (l && a - l[1] <= 2 * HOUR) l[1] = Math.max(l[1], b); else segs.push([a, b]); }
    const PAD = 10 * MIN, pieces = []; let x = 16;
    segs.forEach(([a0, b0], i) => { const a = a0 - PAD, b = b0 + PAD; if (i) { pieces.push({ a: pieces.at(-1).b, b: a, x0: x, w: 28, gap: true }); x += 28; } const w = (b - a) / HOUR * pph; pieces.push({ a, b, x0: x, w }); x += w; });
    const X = (t) => { for (const p of pieces) if (t <= p.b) return p.x0 + Math.max(0, t - p.a) / ((p.b - p.a) || 1) * p.w; const q = pieces.at(-1); return q.x0 + q.w; };
    const xT = (v) => { for (const p of pieces) if (v <= p.x0 + p.w) return p.a + Math.max(0, v - p.x0) / (p.w || 1) * (p.b - p.a); return pieces.at(-1).b; };
    return { pieces, X, xT, width: x + 24 };
  }
  // Rows: top-level sessions by latest activity, each followed by its child runs in start order; a parent with children
  // gets a disclosure row, open by default for 3 or fewer.
  function tlRows() {
    const kids = new Map(); for (const s of Object.values(SESS)) { const p = parentOf(s.id); if (p && SESS[p]) { if (!kids.has(p)) kids.set(p, []); kids.get(p).push(s); } }
    const desc = (sid) => (kids.get(sid) ?? []).flatMap((c) => [c, ...desc(c.id)]);
    const tops = Object.values(SESS).filter((s) => !parentOf(s.id) || !SESS[parentOf(s.id)]).sort((a, b) => b.last - a.last);
    const rows = [];
    const add = (s, depth, hidden) => {
      if (!hidden) rows.push({ k: "s", s, depth, h: depth ? 36 : 40 });
      const ks = (kids.get(s.id) ?? []).sort((a, b) => a.start - b.start); if (!ks.length) return;
      const all = desc(s.id), open = tlView.open.get(s.id) ?? all.length <= 3;
      if (!hidden) rows.push({ k: "d", s, depth, h: 36, open, all });
      for (const c of ks) add(c, depth + 1, hidden || !open);
    };
    tops.forEach((s) => add(s, 0, false));
    return rows;
  }
  const kidsText = (all) => { const sub = all.filter((x) => x.kind === "Subagent").length, cdx = all.filter((x) => x.kind === "Codex run").length, other = all.length - sub - cdx;
    return [sub ? sub + (sub === 1 ? " subagent" : " subagents") : "", cdx ? cdx + (cdx === 1 ? " Codex run" : " Codex runs") : "", other ? other + (other === 1 ? " other run" : " other runs") : ""].filter(Boolean).join(" · "); };
  function renderTimeline(page) {
    const map = tlMap(tlView.pph), X = map.X, rows = tlRows(), at = new Map(); let y = 0;
    for (const r of rows) { r.y = y; y += r.h; if (r.k === "s") at.set(r.s.id, r); }
    // Where a session's marks go: its own row, or its nearest visible ancestor's while it is collapsed.
    const rowOf = (sid) => { let x = sid; const seen = new Set(); while (x && !at.has(x) && !seen.has(x)) { seen.add(x); x = parentOf(x); } return at.get(x); };
    const cy = (r) => r.y + r.h / 2, nm = (x) => x === "you" ? "You" : SESS[x]?.name ?? x;
    const busyMin = Object.values(SESS).reduce((a, s) => a + busyOf(s).reduce((u, [p, q]) => u + (q - p), 0), 0) / MIN;
    const head = el("div", "ph"); const h1 = el("h1", null, "Timeline"); head.append(h1);
    const sub = el("div", "sub"); for (const [v, l] of [[Object.keys(SESS).length, "sessions"], [span(busyMin), "busy"]]) { const x = el("span"); x.append(el("b", null, String(v)), l); sub.append(x); }
    const first = map.pieces[0].a + 10 * MIN; sub.append(el("span", null, clock(first) + " – " + clock(NOW)));
    head.append(sub, el("div", "tl-legend", "Bars: busy · ● your message · ◆ to you · ┗ started · ┆ message")); page.append(head); observeTitle(h1);
    // Controls: zoom (the time under the middle of the view stays put) and the messages toggle.
    const ctl = el("div", "tl-ctl"), zi = ZOOM.indexOf(tlView.pph);
    const zb = (d, label, path) => { const b = el("button", "ibtn"); b.type = "button"; b.setAttribute("aria-label", label); b.append(icon(path)); b.disabled = !ZOOM[zi + d]; b.addEventListener("click", () => zoom(d)); return b; };
    const tog = el("button", "tl-tog", "Messages"); tog.type = "button"; tog.setAttribute("aria-pressed", String(tlView.rel));
    ctl.append(zb(-1, "Zoom out", "M5 12h14"), el("span", "zl", tlView.pph + " px/h"), zb(1, "Zoom in", "M12 5v14M5 12h14"), tog); page.append(ctl);
    const tl = el("div", "tl" + (tlView.rel ? "" : " norel"));
    tog.addEventListener("click", () => { tlView.rel = !tlView.rel; tog.setAttribute("aria-pressed", String(tlView.rel)); tl.classList.toggle("norel", !tlView.rel); });
    // The axis: a sticky row whose inside follows the chart's horizontal scroll.
    const axRow = el("div", "tl-axisrow"), clip = el("div", "tl-axisclip"), axis = el("div", "tl-axis"); axis.style.width = map.width + "px";
    const step = tlView.pph >= 44 ? 1 : tlView.pph >= 22 ? 2 : tlView.pph >= 15 ? 3 : 6; let lastDay = null;
    for (const p of map.pieces) {
      if (p.gap) { const g = el("div", "tl-axgap", "≈" + span((p.b - p.a + 20 * MIN) / MIN)); g.style.left = p.x0 + "px"; axis.append(g); lastDay = null; continue; } // after a break, say the date again
      // Whole local hours inside this stretch (the sample's clock is minutes since midnight).
      let t0 = Math.ceil(p.a / HOUR) * HOUR; if (MIN !== 1) { const d0 = new Date(p.a); d0.setMinutes(0, 0, 0); t0 = +d0 < p.a ? +d0 + HOUR : +d0; }
      for (let t = t0; t <= p.b; t += HOUR) {
        const d = MIN === 1 ? null : new Date(t), hr = MIN === 1 ? Math.round(t / HOUR) % 24 : d.getHours(), x = X(t);
        const tk = el("span", "tl-tick"); tk.style.left = x + "px"; axis.append(tk);
        if (hr % step === 0) { const l = el("span", "tl-hr", two(hr) + ":00"); l.style.left = x + "px"; axis.append(l); }
        if (d && d.toDateString() !== lastDay) { lastDay = d.toDateString(); const dl = el("span", "tl-day", d.toLocaleDateString(undefined, { weekday: "short", day: "numeric" })); dl.style.left = x + "px"; axis.append(dl); }
      }
    }
    clip.append(axis); axRow.append(el("span"), clip);
    clip.addEventListener("wheel", (e) => { if (Math.abs(e.deltaY) > Math.abs(e.deltaX)) { scroller.scrollLeft += e.deltaY; e.preventDefault(); } }, { passive: false });
    // Labels (fixed) and the chart (scrolls sideways).
    const body = el("div", "tl-body"), labels = el("div", "tl-labels"), scroller = el("div", "tl-scroll"), canvas = el("div", "tl-canvas");
    canvas.style.width = map.width + "px"; canvas.style.height = y + "px"; canvas.dataset.map = JSON.stringify(map.pieces);
    for (const r of rows) {
      const tr = el("div", "tl-track" + (r.k === "d" ? " disc" : "")); tr.style.top = r.y + "px"; tr.style.height = r.h + "px"; tr.dataset.sid = r.s.id; tr.dataset.k = r.k; canvas.append(tr);
      const gd = el("span", "gd"); gd.style.width = (r.depth * 12 + (r.k === "d" ? 22 : 4)) + "px";
      if (r.k === "s") {
        const s = r.s, b = el("button", "tl-lab" + (r.depth ? " child" : " top")); b.type = "button"; b.style.height = r.h + "px"; Object.assign(b.dataset, { sid: s.id, depth: r.depth, parent: parentOf(s.id) ?? "" });
        const d = dot(s.state); if (s.stub) d.classList.add("hollow"); b.append(gd, d, el("span", "nm", s.name)); b.title = s.name; b.setAttribute("aria-label", "Open " + s.name);
        b.addEventListener("click", () => goSession(s.id)); labels.append(b);
      } else {
        const b = el("button", "tl-disc"); b.type = "button"; b.style.height = r.h + "px"; b.dataset.sid = r.s.id; b.setAttribute("aria-expanded", String(r.open));
        b.append(gd, el("span", null, (r.open ? "▾ " : "▸ ") + kidsText(r.all))); b.title = kidsText(r.all); b.addEventListener("click", () => { tlView.open.set(r.s.id, !r.open); render(); }); labels.append(b);
        // Collapsed: a faint summary of the hidden runs' bars on the disclosure row.
        if (!r.open) for (const k of r.all) for (const [p, q] of busyOf(k)) { const v = el("span", "tl-sum"); v.style.left = X(p) + "px"; v.style.width = Math.max(3, X(q) - X(p)) + "px"; tr.append(v); }
      }
    }
    for (const p of map.pieces) if (p.gap) { const g = el("div", "tl-gap"); g.style.left = p.x0 + "px"; canvas.append(g); }
    const nowL = el("div", "tl-now"); nowL.style.left = X(NOW) + "px"; canvas.append(nowL);
    const open = (sid, t) => () => { if (SESS[sid]) goSession(sid, t?.id); };
    const hit = (cls, box, say, go, data) => { const b = el("button", "tl-hit " + cls); b.type = "button"; Object.assign(b.style, box); b.title = say; b.setAttribute("aria-label", say); b.addEventListener("click", go); Object.assign(b.dataset, data); canvas.append(b); return b; };
    // Bars: one per busy interval, at least 3 px wide, with at least a 36 px hit area.
    const failed = new Set(H.filter((h) => h.kind === "spawn" && h.status === "err").map((h) => h.to));
    for (const r of rows) {
      if (r.k !== "s") continue; const s = r.s, iv = busyOf(s);
      iv.forEach(([a, b], i) => {
        const x1 = X(a), w = Math.max(3, X(b) - x1), ext = Math.max(0, (36 - w) / 2), last = i === iv.length - 1, live = last && s.state === "work";
        const say = s.name + " busy " + hhmm(a) + "–" + hhmm(b) + (live ? ", still working" : "");
        const bt = hit("tl-barhit", { left: x1 - ext + "px", top: r.y + "px", width: w + 2 * ext + "px", height: r.h + "px" }, say, (ev) => {
          const t = ev.detail ? Math.min(b, Math.max(a, map.xT(ev.clientX - canvas.getBoundingClientRect().left))) : (a + b) / 2; open(s.id, turnAt(s.id, t))(); }, { sid: s.id, i: String(i), a: String(a), b: String(b) });
        const v = el("span", "tl-bar " + hcls(s.id) + (live ? " live" : "") + (last && failed.has(s.id) ? " fail" : "")); v.style.left = ext + "px"; v.style.width = w + "px"; bt.append(v);
      });
    }
    // Marks and connectors, one per handoff (a broadcast once), each opening its turn as in a trace.
    const evs = groupBroadcasts(H.filter((h) => h.at != null).map((h, i) => ({ t: h.at, h, k: h.kind, i })).sort((p, q) => p.t - q.t || p.i - q.i));
    for (const e of evs) {
      const h = e.h, x = X(e.t), tm = hhmm(e.t);
      if (e.k === "ask") { const r = rowOf(h.to); if (!r) continue; const t = STARTS.get(h.id);
        hit("tl-mark ask", { left: x - 18 + "px", top: cy(r) - 18 + "px" }, tm + " You asked " + nm(h.to) + ": " + oneLine(h.brief), open(h.to, t), { k: "ask", hs: h.id, to: h.to, turn: t?.id ?? "", row: r.s.id }).append(el("span", "vis")); }
      else if (e.k === "toyou") { const r = rowOf(h.from); if (!r) continue; const t = HOLDS.get(h.id), w = h.status === "wait", an = answersOf(h);
        const say = tm + " " + nm(h.from) + " " + (TOYOU[h.ask] ?? "Sent you a message").toLowerCase() + (w ? ", waiting on you" : an?.length ? ", answered: " + an.join(" · ") : an ? ", answered" : "");
        hit("tl-mark ty" + (w ? "" : " done"), { left: x - 18 + "px", top: cy(r) - 18 + "px" }, say, open(h.from, t), { k: "toyou", hs: h.id, to: h.from, turn: t?.id ?? "", row: r.s.id }).append(el("span", "vis")); }
      else if (e.k === "spawn" || e.k === "relay") {
        const hs = e.hs ?? [h], from = rowOf(h.from), tos = hs.map((x) => rowOf(x.to)); if (!from || tos.some((r) => !r)) continue;
        const y0 = cy(from), ends = tos.map((r) => ({ r, y: cy(r) }));
        let top = Math.min(y0, ...ends.map((q) => q.y)), bot = Math.max(y0, ...ends.map((q) => q.y));
        if (bot === top) top = y0 - 14; // to its own row (a collapsed child attaches to its parent): a short stub from above
        const hb = Math.max(36, bot - top), ht = (top + bot) / 2 - hb / 2;
        let sid = h.to, t = null, say;
        if (e.k === "spawn") { t = STARTS.get(h.id); say = tm + " " + nm(h.from) + " started " + nm(h.to); }
        else if (hs.length > 1) { t = HOLDS.get(h.id); sid = h.from; say = tm + " " + nm(h.from) + " messaged " + hs.map((x) => nm(x.to)).join(", ") + ": " + oneLine(unsigned(h)); }
        else { t = STARTS.get(h.id); if (!t) { t = HOLDS.get(h.id); if (t) sid = h.from; } say = tm + " " + nm(h.from) + " messaged " + nm(h.to) + ": " + oneLine(unsigned(h)); }
        const c = hit("tl-conn " + (e.k === "spawn" ? "sp" : "rel"), { left: x - 18.75 + "px", top: ht + "px", width: "36px", height: hb + "px" }, say, open(sid, t), { k: e.k, hs: hs.map((q) => q.id).join(","), to: sid, turn: t?.id ?? "", x: String(x) });
        const ln = el("span", "ln"); ln.style.top = top - ht + "px"; ln.style.height = bot - top + "px"; c.append(ln);
        for (const q of ends) {
          if (e.k === "spawn") { const f = el("span", "ft"); f.style.top = q.y - ht + "px"; f.dataset.row = q.r.s.id; f.dataset.y = String(q.y); c.append(f); }
          else { const up = q.y < y0, hd = el("span", "hd " + (up ? "up" : "dn")); hd.style.top = q.y - ht + "px"; hd.dataset.row = q.r.s.id; hd.dataset.y = String(q.y); c.append(hd); }
        }
      }
    }
    scroller.append(canvas); body.append(labels, scroller); tl.append(axRow, body); page.append(tl);
    const sync = () => { axis.style.transform = "translateX(" + -scroller.scrollLeft + "px)"; tlView.left = scroller.scrollLeft; };
    scroller.addEventListener("scroll", sync, { passive: true });
    scroller.addEventListener("wheel", (e) => { if (e.shiftKey && Math.abs(e.deltaY) > Math.abs(e.deltaX)) { scroller.scrollLeft += e.deltaY; e.preventDefault(); } }, { passive: false });
    // Opening position: the latest activity (the right edge); after a zoom, the same time in the middle; after Back, as it was.
    if (tlView.centre != null) { scroller.scrollLeft = X(tlView.centre) - scroller.clientWidth / 2; tlView.centre = null; }
    else scroller.scrollLeft = tlView.left ?? scroller.scrollWidth;
    sync();
    function zoom(d) { const i = ZOOM.indexOf(tlView.pph) + d; if (!ZOOM[i]) return; tlView.centre = map.xT(scroller.scrollLeft + scroller.clientWidth / 2); tlView.pph = ZOOM[i]; render(); }
  }

  // ---- Sessions: every top-level session and its child runs ---------------------------------------------------------------
  const laneOf = (sid) => { const seen = new Set(); while (parentOf(sid) && !seen.has(sid)) { seen.add(sid); sid = parentOf(sid); } return sid; };
  function childRuns(sid) {
    const kids = Object.values(SESS).filter((x) => x.id !== sid && laneOf(x.id) === sid), sub = kids.filter((x) => x.kind === "Subagent").length, cdx = kids.filter((x) => x.kind === "Codex run").length, other = kids.length - sub - cdx;
    return [sub ? sub + (sub === 1 ? " subagent" : " subagents") : null, cdx ? cdx + (cdx === 1 ? " Codex run" : " Codex runs") : null, other ? other + (other === 1 ? " other run" : " other runs") : null].filter(Boolean).join(" · ");
  }
  function renderSessions(page) {
    const all = Object.values(SESS).filter((s) => s.lane);
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
    page.append(fr, gb, out); draw();
  }

  // ---- Drawer (phone) ---------------------------------------------------------------------------------------------------------
  const sidebar = $("#sidebar");
  function openDrawer() { if (!phone.matches) return; document.body.classList.add("drawer-open"); $("#lead-btn")?.setAttribute("aria-expanded", "true"); }
  function closeDrawer(quiet) { if (!document.body.classList.contains("drawer-open")) return; document.body.classList.remove("drawer-open"); const b = $("#lead-btn"); b?.setAttribute("aria-expanded", "false"); if (!quiet) b?.focus(); }
  $("#drawer-close").addEventListener("click", () => closeDrawer());
  $("#scrim").addEventListener("click", () => closeDrawer());
  document.addEventListener("keydown", (e) => { if (e.key === "Escape") { closeDrawer(); $(".menu")?.remove(); closeFilter(); } if (e.key === "/" && !/INPUT/.test(document.activeElement?.tagName ?? "")) { e.preventDefault(); openDrawer(); $("#q").focus(); } });
  let sx = null;
  sidebar.addEventListener("touchstart", (e) => { sx = e.touches[0].clientX; }, { passive: true });
  sidebar.addEventListener("touchmove", (e) => { if (sx !== null && e.touches[0].clientX - sx < -50) { sx = null; closeDrawer(); } }, { passive: true });
  // The sidebar search narrows the Recent list as you type; Enter opens the Sessions page with the same query.
  $("#q").addEventListener("input", (e) => { query = e.target.value.trim(); renderLanes(); });
  $("#q").addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); query = e.target.value.trim(); go({ v: "sessions" }); } });
  phone.addEventListener("change", () => { closeDrawer(true); if (route.v === "timeline") render(); });

  // ---- Live updates (deliberate difference 3) --------------------------------------------------------------------------------
  // Every screen polls /api/model?since= every 2 s while the tab is visible, one request at a time: it backs off up to 30 s
  // on errors and stops on 403 (the server restarted with a new token). A new model swaps the globals and draws the screen
  // again with its view state kept: the scroll position, anchored to the first visible block; what is open, by stable keys;
  // the Timeline's zoom, scroll and rows; focus, find and filters; the drawer. A session page follows its transcript's tail
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
    if (!st.bottom && n) pill(n);
  }

  // View state. A block's identity: its class and keys, or its own text when it has no key (a section heading).
  const scroller = () => (phone.matches ? document.scrollingElement : $("#main"));
  const edge = () => $("#topbar").getBoundingClientRect().bottom;
  const ANCHORS = "[data-e], .turn, .hop, .ib, .nrow, .sec-h, .ph, .divider, .tl-lab, .tl-disc, .tl-ctl, .groupby, .find, .empty";
  const HOSTS = "[data-e], [data-h], [data-id], [data-sid], [data-go], [data-turn], [data-g], [data-m]";
  const FOCUSABLE = "button, input, [tabindex]";
  const identOf = (n) => { const d = n.dataset, keys = [d.e, d.turn, d.h, d.id, d.m, d.sid, d.go, d.g];
    return [n.classList[0], ...keys, keys.some((x) => x != null) ? "" : n.firstChild?.nodeType === 3 ? n.firstChild.data : ""].map((x) => x ?? "").join("|"); };
  // Each anchor candidate on the page, in order, with its identity made unique by how many came before it.
  function anchors(fn) { const seen = new Map(); for (const n of $("#page").querySelectorAll(ANCHORS)) { const id = identOf(n), k = seen.get(id) ?? 0; seen.set(id, k + 1); if (fn(n, id + "#" + k)) return; } }
  const opener = (n) => n.classList.contains("step") ? n.querySelector(":scope > button") : n.classList.contains("tgroup") ? n.querySelector(":scope > .tsum") : n.classList.contains("childwork") ? n.querySelector(":scope > .cw-toggle") : null;
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
    // The Timeline's child rows keep how they are shown, even where that is the default a new child run would change.
    for (const b of $("#page").querySelectorAll(".tl-disc")) tlView.open.set(b.dataset.sid, b.getAttribute("aria-expanded") === "true");
    // At the chart's right edge (within 8 px) it follows new activity, as a session page at its end does; elsewhere it keeps
    // its position in pixels (tlView.left).
    const tl = $("#page .tl-scroll"); if (tl && tl.scrollWidth - tl.scrollLeft - tl.clientWidth <= 8) tlView.left = null;
    // Child work opened and then closed keeps what was open inside it, for when it opens again.
    st.shut = new Set([...$("#page").querySelectorAll(".childwork[data-e]")].filter((n) => opener(n).getAttribute("aria-expanded") === "false" && n.querySelector(":scope > .cw-body").childElementCount).map((n) => n.dataset.e));
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
    const shut = all(".childwork[data-e]").filter((n) => st.shut.has(n.dataset.e) && opener(n).getAttribute("aria-expanded") === "false");
    for (const n of all(".childwork[data-e]")) if ((st.open.has(n.dataset.e) || shut.includes(n)) && opener(n).getAttribute("aria-expanded") === "false") opener(n).click();
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
    const keys = () => new Set([...$("#page").querySelectorAll(".turns :is(.msg, .step, .hcard)[data-e]")].filter((n) => !n.closest(".cw-body")).map((n) => n.dataset.e));
    const before = keys();
    // Only the changed turns are drawn again, unless the turns shown no longer match the index or nothing was shown.
    const whole = !dirty || box.querySelector(":scope > p.empty") || [...box.querySelectorAll(":scope > .turn")].some((b) => !TURN.has(b.dataset.turn));
    if (whole) morph(box, transcript(route.id).querySelector(".turns"));
    else if (dirty.size) morphTurns(box, transcript(route.id, { only: dirty }).querySelector(".turns"), dirty);
    const h1 = $("#page .ph h1"); if (h1) h1.textContent = s.name;
    const t = $("#topbar .t"); if (t) { t.textContent = s.name; t.title = s.name; }
    const l2 = $("#topbar .l2"); if (l2) { l2.replaceChildren(); sessionLine(s)(l2); }
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

  // "N new ↓": entries that arrived below a reader who had scrolled up. Tapping it goes to the end; reaching the end clears it.
  function pill(n) {
    LIVE.fresh += n; let p = $(".newpill");
    if (!p) { p = el("button", "newpill"); p.type = "button"; p.addEventListener("click", () => { const sc = scroller(); sc.scrollTop = sc.scrollHeight; clearPill(); }); document.body.append(p); }
    p.textContent = LIVE.fresh + " new ↓"; p.setAttribute("aria-label", LIVE.fresh + (LIVE.fresh === 1 ? " new entry" : " new entries") + ": go to the latest");
  }
  function clearPill() { LIVE.fresh = 0; $(".newpill")?.remove(); }
  const atEnd = () => { if (!LIVE.fresh) return; const sc = scroller(); if (sc.scrollHeight - sc.scrollTop - sc.clientHeight <= 80) clearPill(); };
  window.addEventListener("scroll", atEnd, { passive: true });
  $("#main").addEventListener("scroll", atEnd, { passive: true });

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
