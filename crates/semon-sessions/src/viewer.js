import { createErrorNavigation } from "../../../ui/src/navigation/errors";
import { NavigationController } from "../../../ui/src/navigation/routes";
import { createScrollTransactions } from "../../../ui/src/navigation/scroll";
import { ViewerModelStore } from "../../../ui/src/state/model";
import { TranscriptStore } from "../../../ui/src/state/transcript";
import { createTraceCalculations } from "../../../ui/src/domain/trace";
import { createDomain } from "../../../ui/src/domain/calculations";
import { clock as formatClock, ago as formatAgo, dur as formatDuration, tok, shortName, machineShorts, shortModel, clean, liveUrl, preview, compactCount, niceStep, timeText, countText, hLabel } from "../../../ui/src/domain/format";
import { measureSessionScreen, measureTraceScreen, measureViewerBar, createOrdering, orderRows, createPagerController, routeUrl, parseRoute, TranscriptCache, setGeometry, revealMeasuredTurn, createLiveRegion, requestJson, ModelStore, createLiveController, createPagingStore, renderPlaceholder, createStatusNote, createSelect, createViewerBar, renderSessionScreen, updateSessionPager, updateSessionJump, updateSessionClock, renderTraceScreen, renderSliceBody, renderModelItems, renderAnalyticsScreen, createKidsSheet, createNativeSheet, renderSessionMenu, renderFullTool, createImageViewer, parseAccount, createAccountChrome, createShellChrome, renderShellNavigation, createRecentRenderer, createPanelChrome, createFacetChrome, renderSessionsScreen, renderMachinesScreen, renderHomeScreen, renderMachineScreen, ownsScreen, screenKind, releaseScreen } from "../../../ui/src/account-adapter";
import { getViewerHost } from "../../../ui/src/viewer-host";
queueMicrotask(() => {
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
  const modelStore = new ViewerModelStore();
  const MACHINE = modelStore.machines;
  const MACHINE_UP = modelStore.machineUp;
  // An offline machine's last-seen time (epoch ms), and the embedding server's machine-management link, when served.
  const MACHINE_LAST = modelStore.machineLast;
  let ADMIN = null;
  let ACCOUNT = null;
  const viewerHost = getViewerHost();
  const NATIVE_PAGE = viewerHost?.nativePage;
  let NAV_MACHINES = viewerHost?.machinesPath ?? null;
  // An embedding page that shows the viewer's sidebar beside its own content marks its .app data-viewer="sidebar" (docs/shell.md):
  // only the sidebar is drawn there, and every destination opens the viewer's own page.
  const SIDEBAR_ONLY = document.querySelector(".app")?.dataset.viewer === "sidebar";
  // The harnesses Semon can name. Mirrors crates/semon-sessions/src/harness.rs (a Rust test keeps them equal). Icons identify the source only; the artwork is served unmodified.
  const HARNESSES = { claude: { name: "Claude Code", short: "Claude", icon: { light: "/harness/claude-code.svg", dark: "/harness/claude-code.svg" } }, codex: { name: "Codex", short: "Codex", icon: { light: "/harness/codex-black.svg", dark: "/harness/codex.svg" } }, opencode: { name: "OpenCode", short: "OpenCode", icon: { light: "/harness/opencode-light.svg", dark: "/harness/opencode-dark.svg" } } };
  const HARNESS = Object.fromEntries(Object.entries(HARNESSES).map(([id, h]) => [id, h.name]));
  const HARNESS_SHORT = Object.fromEntries(Object.entries(HARNESSES).map(([id, h]) => [id, h.short]));
  const SESS = modelStore.sessions;
  const H = modelStore.handoffs;
  const transcripts = new TranscriptStore({ request: (path, signal) => api(path, signal), turns: sid => modelStore.turns[sid] ?? [], turn: id => modelStore.turn.get(id), entry: e => txEntry(e), cleared(sid) { if (navigation.route.v === "session" && navigation.route.id === sid) resetPagerInput(); } });
  const TX = transcripts.entries;
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
  const HID = modelStore.handoff;
  const TURNS = modelStore.turns, TURN = modelStore.turn, STARTS = modelStore.starts, HOLDS = modelStore.holds;
  const TXM = transcripts.meta; // per session: the loaded range of its transcript { from, to, total } and its totals { calls, errors }
  const domain = createDomain({ sessions: SESS, machines: MACHINE, handoffs: H, turns: TURNS, turn: TURN, starts: STARTS, holds: HOLDS, handoff: HID, transcriptMeta: TXM }, () => NOW, SEEN_RESULTS);
  const { nameOf, hcls, where, hostOf, machineLabels, machineLabel, branchOf, shortHost, parentOf, originHandoff, RANK, isResult, inbox, working, answersOf, statWord, hasTurn, oneLine, TOYOU, turnEnd, traceRoot, countOf, callsText, sessionChildren, childSessions, descendantsOf, TOTAL_TOKEN_KINDS, TOKEN_KINDS, asMoney, usageTotal, costForSessions, costForSession, costText, costMissing, TREE_RANK, urgentDescendant, childParts, defaultTreeOpen, kidRank, lineageOf, byState, onMachine, movedOff, movesOf, shortMoney } = domain;
  const clock = t => formatClock(t, NOW), ago = t => formatAgo(t, NOW), dur = (a, b) => formatDuration(a, b, NOW);
  const $ = (s, r = document) => r.querySelector(s);
  // Instrument Sans sets the middle dot with little side bearing. Thin spaces keep separators readable without changing code.
  const spaced = (t) => String(t).replace(/ · /g, "\u2009 · \u2009").replace(/^· /, "·\u2009 ");
  const I = {
    menu: "M4 7h16M4 12h16M4 17h16", more: "M5 12h.01M12 12h.01M19 12h.01", back: "M15 6l-6 6 6 6", chev: "M9 6l6 6-6 6", search: "M11 18a7 7 0 1 1 0-14 7 7 0 0 1 0 14zM20 20l-4-4", filter: "M4 6h16M7 12h10M10 18h4",
    home: "M4 11l8-7 8 7M6 9.5V20h12V9.5M10 20v-5h4v5", inbox: "M4 13l2.5-8h11L20 13v6H4zM4 13h5l1 2h4l1-2h5", now: "M3 12h4l2.5-6 5 12 2.5-6h4", trace: "M6 4v10a4 4 0 0 0 4 4h8M6 10h12M15 7l3 3-3 3M15 15l3 3-3 3", sessions: "M9 6h11M9 12h11M9 18h11M4.5 6h.01M4.5 12h.01M4.5 18h.01",
    run: "M4 17l5-5-5-5M12 19h8", stack: "M12 3l9 5-9 5-9-5zM3 13l9 5 9-5", read: "M6 3h8l4 4v14H6zM14 3v4h4", edit: "M4 20h4L19 9l-4-4L4 16zM13 7l4 4", find: "M11 18a7 7 0 1 1 0-14 7 7 0 0 1 0 14zM20 20l-4-4",
    machine: "M3 5h18v11H3zM8 20h8M12 16v4", repo: "M6 3v12M6 15a3 3 0 1 0 0 6 3 3 0 0 0 0-6zM18 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM18 9c0 6-12 3-12 6", duration: "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM12 7v5l3 2", delegate: "M4 3h7v5H4zM7.5 8v9H13M13 14h7v6h-7z",
    out: "M7 17L17 7M9 7h8v8", in: "M17 7L7 17M15 17H7V9", move: "M4 8h13l-3-3M20 16H7l3 3", ask: "M5 18l-1 3 3-1 11-11-2-2zM14 6l4 4", you: "M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM4 21c1-4 4-6 8-6s7 2 8 6", q: "M9 9a3 3 0 1 1 4 2.8c-.7.3-1 .9-1 1.7V14M12 18h.01", check: "M5 12l4 4 10-10", qc: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM9.6 9.4a2.5 2.5 0 1 1 3.4 2.4c-.6.3-1 .8-1 1.5v.4M12 17h.01", decide: "M12 21v-6M12 15L6 9M12 15l6-6M6 9V4M18 9V4M4 6l2-2 2 2M16 6l2-2 2 2", result: "M14 3H6v18h12V7zM14 3v4h4M9 12h6M9 16h6", done: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM8 12.5l2.7 2.7L16 9.8", x: "M6 6l12 12M18 6L6 18", expand: "M15 3h6v6M9 21H3v-6M21 3l-7 7M3 21l7-7", copy: "M9 9h11v11H9zM5 15H4V4h11v1", ext: "M14 4h6v6M20 4l-9 9M18 14v6H4V6h6",
    down: "M12 4v15M5 12l7 7 7-7", up: "M6 15l6-6 6 6", dn: "M6 9l6 6 6-6", branch: "M6 3v12M6 15a3 3 0 1 0 0 6 3 3 0 0 0 0-6zM18 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM18 9c0 6-12 3-12 6",
    wrench: "M14.5 6.5a5 5 0 0 0-6.9 6.9l-4.8 4.8a2 2 0 0 0 2.8 2.8l4.8-4.8a5 5 0 0 0 6.9-6.9l-3 3-2.8-2.8z", wide: "M4 9V4h5M15 4h5v5M20 15v5h-5M9 20H4v-5", sidebar: "M4 5h16v14H4zM9 5v14", tokens: "M5 5h14M12 5v14M9 19h6", chart: "M4 19V5M4 19h17M8 15l3-4 3 2 5-7", coin: "M12 3v18M17 7.5C17 6.1 14.8 5 12 5S7 6.1 7 7.5 9.2 10 12 10s5 1.1 5 2.5-2.2 2.5-5 2.5-5-1.1-5-2.5", relay: "M4 7h13l-3-3M20 17H7l3 3",
  };
  // A dot is the state's only sign where nothing beside it says the state, and then it carries a tooltip; `tip = false` where a word does.
  const STATE = { work: "Working", wait: "Needs you", idle: "Idle", done: "Done", err: "Failed", new: "New result", read: "Read result" };
  // The name of a machine that has none is drawn whole ("Unknown machine"); a real one is cut to 14 characters.
  // Each machine the sessions in `scope` (sessions or their ids; every session by default) run on, by its short display name, only where
  // they span several machines. With one machine every line would say the same thing, so the Map is empty. One entry per machine id.
  // A harness is named in plain muted text (.hname), never in a vendor colour; "short" gives "Claude" where the line is tight. The label itself
  // holds text only: its official mark (harnessIcon) is drawn beside it, outside the span, so the word still tells Claude and Codex apart.
  // The short name ("Claude") gets the long one ("Claude Code") as its tooltip; the long one repeats itself, so it has none.
  // A harness's official mark, drawn unmodified from HARNESSES and served at /harness/*.svg. It says which harness a session belongs to (identity or
  // source) and nothing about its state: the state dot is a separate control that a mark never replaces or merges with. Consistency comes from the
  // container's size (`size`, px) and padding alone; the artwork is never recoloured or cropped. A mark with a light and a dark file draws both and CSS
  // shows one, by the theme (prefers-color-scheme or an explicit data-theme), so the mark follows a theme switch with no script.
  // `label` says whether the mark stands alone: when it does (true, or a string to name it), its container carries the accessible name and a tip, since
  // nothing beside it names the harness. With text beside it the mark is decorative (aria-hidden), and the text is the name.
  // `lead` puts a small gap after the mark, where it sits in running text. An id the registry doesn't know has no mark: null.
  const darkTheme = () => { const t = document.documentElement.getAttribute("data-theme"); return t === "dark" || (t !== "light" && !!window.matchMedia?.("(prefers-color-scheme: dark)").matches); };
  const facetLine = (s) => [s.kind ?? HARNESS[s.harness], MACHINE[s.machine], where(s)].join(" · ");
  function markSeenResults(handoffs) {
    let changed = false;
    for (const h of handoffs) if (isResult(h) && typeof h.id === "string" && !SEEN_RESULTS.has(h.id)) {
      SEEN_RESULTS.add(h.id); changed = true;
    }
    while (SEEN_RESULTS.size > SEEN_LIMIT) SEEN_RESULTS.delete(SEEN_RESULTS.values().next().value);
    if (changed) try { window.localStorage.setItem(SEEN_KEY, JSON.stringify([...SEEN_RESULTS])); } catch {}
  }
  // Inline marks: `code`, **bold**, *italic*, ~~strike~~, [text](url) and bare URLs. Everything goes in as text nodes;
  // only http(s) links are live, and they open in a new tab. Anything else stays literal text, <tags> included.
  // What you answered, when the logs kept it: h.answer holds one string per question, in the brief's order.
  function sentenceSnapshot(h, viewer, links = false) {
    const parts = [], text = (className, text) => parts.push({ className, text });
    const who = (id, action, label) => { const name = nameOf(id), linked = links && action && id !== "you" && id !== viewer && SESS[id]; parts.push({ text: name, className: linked ? "who-link" : "who", action: linked ? action : undefined, label: linked ? label(name) : undefined }); };
    const sender = { kind: "sender", id: h.id }, recipient = { kind: "session", id: h.to, turn: STARTS.get(h.id)?.id };
    const senderLabel = name => "Open " + name + " where it sent this", recipientLabel = name => "Open " + name + " at the turn this started";
    let path;
    if (h.kind === "ask") { path = I.ask; who("you"); text("verb", " asked "); who(h.to, recipient, recipientLabel); }
    else if (h.kind === "spawn" || h.kind === "relay") {
      if (viewer === h.to) { path = I.in; text("verb", h.kind === "spawn" ? "Brief from " : "Relay from "); who(h.from, sender, senderLabel); }
      else { path = I.out; who(h.from, sender, senderLabel); text("verb", h.kind === "spawn" ? " handed off to " + (SESS[h.to]?.kind === "Subagent" ? "subagent" : SESS[h.to]?.kind ?? "") + " " : " relayed to "); who(h.to, recipient, recipientLabel); }
    } else if (h.kind === "move") {
      path = I.move; const short = machineShorts([h.fromMachine, h.toMachine].map(id => [id, MACHINE[id] ?? id]));
      const machine = id => parts.push({ className: "verb mach", text: short.get(id), tip: "Machine: " + (MACHINE[id] ?? id), action: links ? { kind: "machine", id } : undefined, label: links ? "Open machine " + (MACHINE[id] ?? id) : undefined });
      text("verb", "Semon moved "); who(h.to, { kind: "session", id: h.to }, name => "Open " + name); text("verb", " from "); machine(h.fromMachine); text("verb", " to "); machine(h.toMachine);
    } else { path = h.status === "done" && (h.ask === "question" || h.ask === "decision") ? I.done : h.ask === "question" ? I.qc : h.ask === "decision" ? I.decide : I.result; who(h.from, sender, senderLabel); text("verb", { question: " asked you", result: " sent you a result", decision: " needs your decision" }[h.ask]); }
    return { icon: path, parts };
  }
  const sentenceHost = { session(id, turn) { goSession(id, turn); }, machine(id) { go({ v: "machine", id }); }, sender(id) { const h = HID.get(id); if (h) openSender(h); } };
  // ---- Turns and traces, computed from the transcripts ---------------------------------------------------
  // A turn starts at each incoming entry: your message, a relay from another session, or the brief that starts a
  // subagent or Codex run. It runs to the next incoming entry. What it sends on (spawns, relays, messages to you)
  // and machine moves are content inside it. Entries before the first incoming one form a leading turn without a
  // header. A turn's id is its start handoff's id, else session:index. STARTS maps a handoff to the turn it started;
  // HOLDS maps it to the turn that sent it.
  // A gap marker ("Earlier entries not included in this copy") is where this copy skips part of the log. It ends the
  // turn before it, and what follows starts a turn of its own, so the marker is drawn between turns, never inside one.
  const isGap = (e) => e.k === "end" && /entries (not included|omitted)|^No activity/.test(e.text ?? "");
  // TURNS, TURN, STARTS and HOLDS keep the shapes above, filled from the server's turn index (adopt), since the page no longer
  // holds every transcript. A turn's entries are the loaded ones (spread).
  // ---- Loading: the model from /api/model, transcripts a page at a time from /api/tx --------------------------
  // A session's tool calls and errors, from the model (`calls` and `errors` on each session; absent from an older server or
  // cache: null, shown as "—"). Nothing fetches a transcript only to count it. A transcript loaded to its end is tailed by
  // every update, so its own totals agree with its entries; a range that stops short (a deep link, a child's start turn)
  // keeps the totals from when it was fetched, so the model's win there.
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
  const api = requestJson;
  // NOW follows the client clock from the model's `now`, so every "ago" keeps moving; a running tool's age follows NOW.
  function tick() {
    NOW = serverNow + (Date.now() - fetchedAt);
    for (const s of Object.values(SESS)) if (s.activity && s.activity[3] != null) s.activity[2] = Math.floor((NOW - s.activity[3]) / 1000);
  }
  function adopt(m) {
    m = modelStore.adopt(m);
    domain.invalidate();
    serverNow = m.now; fetchedAt = Date.now(); TOK = m.tx ?? {}; transcripts.marks = TOK;
    ADMIN = m.admin && safePath(m.admin.href) ? m.admin : null;
    // A server-provided menu wins; otherwise an embedding page may set `window.semonEmbed.account`, held to the same rules.
    ACCOUNT = accountOf(m.account) ?? accountOf(viewerHost?.account) ?? embeddedAccount();
    viewerHost?.modelAccount?.(ACCOUNT);
    NAV_MACHINES = viewerHost?.machinesPath ?? (m.nav && safePath(m.nav.machines) ? m.nav.machines : null);
    tick();
  }
  // Each loaded entry goes to its turn: an entry that starts a turn (or a page) names it. Its key, its turn and place in
  // it, stays the same while the transcript only grows: live updates find what was open and where the reader was by it.
  const spread = sid => transcripts.spread(sid);
  // A return line arrives as data; it reads as the mockup's "Returned to … · HH:MM".
  const txEntry = (e) => e.k === "end" && e.ret ? { k: "end", text: "Returned to " + nameOf(e.ret.to) + (e.ret.failed ? " · failed" : "") + (e.ret.at != null ? " · " + clock(e.ret.at) : ""), turn: e.turn } : e;
  // where: "before" and "after" extend the loaded range; otherwise the page replaces it.
  const fetchTx = (sid, q, where, signal, onPage) => transcripts.fetch(sid, q, where, signal, onPage);
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
  const STALE_BRIEFS = transcripts.staleBriefs;
  const TXCACHE = transcripts.cache;
  const cacheTx = (sid, entries, meta) => transcripts.keep(sid, entries, meta, !!originHandoff(sid));
  const adoptCached = r => transcripts.adoptCached(r.id, r.turn);
  // A mark with fewer entries or bytes than the one loaded means the file was cut or rewritten: load it again.
  function shrank(a, b) { const [s0, b0] = String(a).split(".").map(Number), [s1, b1] = String(b).split(".").map(Number); return s1 < s0 || b1 < b0; }
  // A transcript drawn from the cache is brought up to date the way a live update does it: when the model's mark for it moved
  // since it was kept, its tail is fetched (or the whole page, if the file shrank), and its child work loads. Nothing is asked
  // for when the mark is the same. The page is drawn again, keeping the reader's place, once something arrived.
  function revalidate(r) {
    const sid = r.id, m = TXM[sid], moved = m && m.to >= m.total && m.tok != null && TOK[sid] != null && m.tok !== TOK[sid];
    const job = moved ? (shrank(m.tok, TOK[sid]) ? reload(sid) : tail(sid)) : null, work = job;
    if (work) work.then(() => { if (navigation.route === r && navigation.rendered === r) refresh(null); }, () => {});
  }
  // Paging state survives redraws: a click and an observer share one request per session and direction, and a failed
  // page stays manual until Retry succeeds. Observers belong only to the buttons currently drawn.
  const pagingStore = transcripts.paging, PAGING = pagingStore.states;
  let pagerArmed = false, automaticLoads = 0, scrollRevision = 0, programmaticScrollPending = false, programmaticScrollTimer = null;
  const clearPaging = sid => transcripts.clearPaging(sid);
  const dropTx = sid => transcripts.drop(sid);
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
    if (navigation.route.v !== "session" || navigation.rendered !== navigation.route || $("#page").hasAttribute("aria-busy")) return;
    clearTimeout(programmaticScrollTimer); programmaticScrollTimer = null; programmaticScrollPending = false;
    stopOpeningEndPin(); pagerArmed = true; automaticLoads = 0; scrollRevision++; queuePagerObservers();
  }
  const automaticPagingAllowed = () => pagerArmed && automaticLoads < 3 && !openingEndUntil && !findOpen && !find && show.messages && show.tools && show.thinking;
  const pagingState = (sid, where) => pagingStore.get(sid, where);
  function pagerSnapshot(sid, where) {
    const state = pagingState(sid, where), direction = where === "before" ? "earlier" : "later";
    return { sid, where, disabled: state.busy, busy: state.busy, text: state.busy ? "Loading " + direction + "…" : state.failed ? "Couldn't load " + direction + " entries · Retry" : "Load " + direction + (where === "after" && TXM[sid]?.newer ? " · " + TXM[sid].newer + " new" : "") };
  }
  function paintPager(b) { updateSessionPager($("#page"), pagerSnapshot(b.dataset.pagerSid, b.dataset.pagerWhere)); }
  const pagerController = createPagerController({
    route(sid) { return navigation.route.v === "session" && navigation.route.id === sid && navigation.rendered === navigation.route ? navigation.route : null; }, range(sid) { return TXM[sid]; }, state: pagingState, automatic: automaticPagingAllowed,
    current(sid, where, state, r) { return PAGING.get(sid)?.[where] === state; }, beginManual: stopOpeningEndPin, countAutomatic() { automaticLoads++; }, paint: paintPager,
    load(sid, where, boundary, signal, applied) { return fetchTx(sid, where + "=" + boundary, where, signal, applied); },
    commit(r, where, manual) {
      const box = scroller(), top = phone.matches ? 0 : box.getBoundingClientRect().top, st = capture();
      const entry = [...$("#page").querySelectorAll(".turns [data-e][data-entry-key]:not(.tgroup)")].find(n => { const rect = n.getBoundingClientRect(); return rect.height && rect.top >= top; });
      st.paging = { anchor: entry ? { key: entry.dataset.entryKey, off: entry.getBoundingClientRect().top - top } : null, height: box.scrollHeight, before: where === "before" };
      const armed = pagerArmed, used = automaticLoads; render(); restore(st); if (!manual) { pagerArmed = armed; automaticLoads = used; } syncJump(); saveHistoryScroll();
    }, queue: queuePagerObservers,
  });
  function disconnectPagerObservers() { pagerController.disconnect(); }
  function queuePagerObservers() {
    if (SIDEBAR_ONLY) return;
    pagerController.queue($("#page"), phone.matches ? null : $("#main"), () => navigation.route.v === "session" && navigation.rendered === navigation.route && automaticPagingAllowed() && !$("#page").hasAttribute("aria-busy"));
  }
  function loadPager(button, manual) { return pagerController.load(button, manual); }
  // "Load earlier" at the top of a transcript, and "Load later" at its end when a deep link loaded a middle page.

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
  const routeModel = { session(id) { return SESS[id]; }, machine(id) { return !!MACHINE[id]; }, turn(id) { return TURN.get(id); }, get machinesPath() { return viewerHost?.machinesPath; } };
  const urlOf = r => routeUrl(r, routeModel), routeOf = location => parseRoute(location, routeModel);
  function boot() {
    api("/api/model?delta=1").then((m) => {
      adopt(m); LIVE.version = m.version; remember(m); if (SIDEBAR_ONLY || NATIVE_PAGE) { if (NATIVE_PAGE) navigation.route = { v: NATIVE_PAGE.nav }; render(); schedule(2000); return; } navigation.route = routeOf(location);
      if (navigation.route.v === "sessions") query = (new URLSearchParams(location.search).get("q") ?? "").trim(); // direct Sessions links can prefill its search field
      if (navigation.route.v === "machines" && NAV_MACHINES && !viewerHost) { location.assign(NAV_MACHINES); return; }
      try { history.replaceState({ ...navigation.route, scrollTop: 0 }, "", urlOf(navigation.route) + (navigation.route.v === "session" ? location.hash : navigation.route.v === "sessions" && query ? "?q=" + enc(query) : "")); } catch {}
      const done = () => {
        render();
        if (navigation.route.v === "session" && navigation.route.turn) { revealTurn(navigation.route.turn, true); if (location.hash) requestAnimationFrame(() => requestAnimationFrame(revealEntryHash)); }
        else if (navigation.route.v === "session" && location.hash) revealEntryHash();
        else if (navigation.route.v === "session") { openSessionAtEnd(); syncJump(); }
        else quietTop();
        schedule(2000); setInterval(ticker, 1000);
      };
      const initialRoute = navigation.route, p = load(initialRoute);
      if (p) p.then(() => { done();  }, done);
      else { done();  }
    }, (err) => { if (viewerHost?.modelFailed?.(err?.status ?? 0)) return; if (viewerHost) { console.warn("semon: model unavailable", err.status); return; } renderPlaceholder($(SIDEBAR_ONLY ? "#lanes" : "#page"), "Couldn't load the sessions: " + err.message); });
  }

  // ---- State & navigation ---------------------------------------------------------------
  const phone = window.matchMedia("(max-width: 760px)");
  const navigation = new NavigationController({ loadMachines: viewerHost ? signal => viewerHost.loadMachines(signal) : undefined }, viewerHost?.initialMachines ?? null); // (before the layout preferences, which read it)
  let wideMode = false, railMode = false, treePrefs = {};
  try { wideMode = localStorage.getItem("semon.wide") === "1"; } catch {}
  try { railMode = !SIDEBAR_ONLY && localStorage.getItem("semon.rail") === "1"; } catch {} // the rail is the viewer's own layout: an embedding page keeps its sidebar whole
  try { const saved = JSON.parse(localStorage.getItem("semon.tree") ?? "{}"); if (saved && typeof saved === "object" && !Array.isArray(saved)) treePrefs = pruneTreePrefs(saved); } catch {}
  const app = $(".app");
  const syncLayoutPrefs = () => { if (SIDEBAR_ONLY) return; app.classList.toggle("rail", railMode && !phone.matches); $("#page").classList.toggle("wide-mode", wideMode && !phone.matches && navigation.route.v === "session"); };
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
  const railToggle = SIDEBAR_ONLY ? ($("#rail-toggle") ?? document.createElement("button")) : document.createElement("button");  railToggle.setAttribute("aria-expanded", String(!railMode)); railToggle.setAttribute("data-tip", railMode ? "Expand sidebar" : "Collapse sidebar"); railToggle.setAttribute("aria-label", railMode ? "Expand sidebar" : "Collapse sidebar"); railToggle.addEventListener("click", () => setRailMode(!railMode)); syncLayoutPrefs();
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
  let hostFocus = null;
  if (viewerHost) document.addEventListener("focusin", event => {
    const node = event.target;
    if (!(node instanceof HTMLElement) || !$("#page").contains(node)) return;
    hostFocus = node.id ? { id: node.id } : node.dataset.id ? { row: node.dataset.id } : node.getAttribute("aria-label") ? { label: node.getAttribute("aria-label") } : null;
  });
  function restoreHostFocus(saved) {
    if (!viewerHost || !saved) return;
    const selector = saved.id ? "#" + CSS.escape(saved.id) : saved.row ? '[data-id="' + CSS.escape(saved.row) + '"]' : saved.label ? '[aria-label="' + CSS.escape(saved.label) + '"]' : null;
    if (selector) $("#page")?.querySelector(selector)?.focus({ preventScroll: true });
  }
  const saveHistoryScroll = () => { if (viewerEl) return; try { if (history.state?.v) history.replaceState({ ...history.state, scrollTop: currentScroll(), ...(viewerHost ? { hostFocus } : {}) }, ""); } catch {} };
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
  let skeletonTimer = null, lanesFor = null;
  // The turn shapes the skeleton cycles through: a bubble (yours), text lines, a step row. Widths are classes, sk-w1..sk-w5, in percent.
  function paintPending(r) {
    const page = $("#page"), hadFocus = $("#sidebar").contains(document.activeElement);
    renderNav(); renderLanes(); drawSessionBar();
    lanesFor = { r, version: LIVE.version };
    if (hadFocus) $("#lanes .srow[data-id='" + CSS.escape(r.id) + "']")?.focus({ preventScroll: true });
    page.setAttribute("aria-busy", "true"); page.inert = true; page.classList.add("loading");
    clearTimeout(skeletonTimer);
    skeletonTimer = setTimeout(() => {
      if (navigation.route !== r || !page.classList.contains("loading")) return;
      page.classList.remove("loading"); page.classList.remove("child-page"); setGeometry(page, "paddingBottom", null);
      renderPlaceholder(page); quietTop(); syncBarLine();
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
    if (navigation.route !== r || err?.name === "AbortError") return;
    endLoading();
    const page = $("#page"); page.classList.remove("child-page"); setGeometry(page, "paddingBottom", null); renderPlaceholder(page, "Couldn't load this session: " + (err?.message ?? "no response"), () => go({ ...r }, true));
  }
  // A deep link to a turn the loaded transcript doesn't hold yet.
  const isDeep = (r) => { const t = r.turn ? TURN.get(r.turn) : null; return !!t && t.sid === r.id && !t.entries.length; };
  function go(r, fromHistory, prepared = false, nextContent = null) {
    navigation.cancelNative();
    if (NATIVE_PAGE) { if (!fromHistory) location.assign(r.v === "machines" ? NAV_MACHINES : urlOf(r)); return; }
    if (r.v === "machines" && viewerHost && !prepared) {
      closeDrawer(true); closeAccountMenu(true, true);
      navigation.loadNative(r, content => go(r, fromHistory, true, content), () => location.assign(viewerHost.machinesPath));
      return;
    }
    if (r.v !== "sessions" || r !== focusSessionsSearchOnRender) focusSessionsSearchOnRender = null;
    if (SIDEBAR_ONLY) { if (!fromHistory) { closeDrawer(true); location.assign(r.v === "machines" && NAV_MACHINES ? NAV_MACHINES : urlOf(r)); } return; } // an embedding page's sidebar leads to the viewer's pages
    if (navigation.route.v === "session") clearPaging(navigation.route.id);
    resetPagerInput(); stopOpeningEndPin(); navigation.cancelRoute();
    if (r.v === "timeline") { r = { ...r, v: "analytics" }; try { history.replaceState({ ...r, scrollTop: r.scrollTop ?? currentScroll() }, "", urlOf(r)); } catch {} }
    if (r.v === "machines" && NAV_MACHINES && !viewerHost) { location.assign(NAV_MACHINES); return; }
    if (!fromHistory) saveHistoryScroll();
    // Capture outgoing history before removing host content: removal can clamp its scroll offset.
    navigation.replaceContent(r, nextContent);
    closeAccountMenu(true, true);
    dropErrors(true); // (first: it drops a range the error stepper moved, and that is not kept)
    // Keep the session left after the cached destination has had a frame to draw; weighing it must not delay that draw.
    if (navigation.route.v === "session" && (r.v !== "session" || r.id !== navigation.route.id) && TX[navigation.route.id] && TXM[navigation.route.id]) { const sid = navigation.route.id, entries = TX[sid], meta = { ...TXM[sid], origin: !!originHandoff(sid) }; requestAnimationFrame(() => requestAnimationFrame(() => setTimeout(() => cacheTx(sid, entries, meta), 0))); }
    if (r.v !== "session" || r.id !== navigation.route.id) show = { ...SHOW_ALL };
    if (r.v !== "sessions") ORD.delete("page");
    navigation.route = r; find = ""; findOpen = false; closeDrawer(true); clearNewEntries();
    if (!fromHistory) { const state = { ...r }; delete state.scrollTop; try { history.pushState(state, "", urlOf(r)); } catch {} }
    const done = () => {
      if (navigation.route !== r) return;
      endLoading(); render(); if (r.v === "session" || (viewerHost && r.v === "machines")) focusTitle();
      if (fromHistory && Number.isFinite(r.scrollTop)) {
        // A fresh offscreen turn has only its intrinsic estimate. Measure once on history navigation
        // before setting the saved offset, so the browser cannot clamp it to the estimated height.
        const turns = [...$("#page").querySelectorAll(".turn")];
        for (const turn of turns) revealMeasuredTurn(turn, true);
        const heights = turns.map((turn) => turn.getBoundingClientRect().height);
        turns.forEach((turn, i) => { setGeometry(turn, "intrinsicHeight", Math.ceil(heights[i])); revealMeasuredTurn(turn, false); });
        restoreScroll(r.scrollTop); restoreHostFocus(r.hostFocus);
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
        requestAnimationFrame(() => setTimeout(() => { if (navigation.route !== r) return; if (kept && !r.turn) spread(r.id); done(); revalidate(r); }, 0));
        return;
      }
    }
    if (r.v === "session") paintPending(r);
    navigation.load(r, signal => load(r, signal), done, error => failLoad(r, error));
  }
  if (!SIDEBAR_ONLY) window.addEventListener("popstate", (e) => {
    if (skipPop) { skipPop = false; if (afterPop) { const leave = afterPop; afterPop = null; leave(); return; } if (pendingSessionOpen) { const id = pendingSessionOpen; pendingSessionOpen = null; goSession(id); } return; } // close a sheet before opening its session
    if (accountSheet) { accountSheet = false; closeAccountMenu(true); return; } // back gesture closes the phone's account menu
    if (viewerEl) { const d = viewerEl; viewerEl = null; d.close(); return; } // back gesture closes the viewer, page stays
    if (e.state?.v) go(navigation.historyRoute(e.state, routeOf(location)), true); });
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
  const accountHost = {
    place(widget, trigger) {
      const at = trigger.getBoundingClientRect();
      setGeometry(widget, "accountLeft", at.left);
      setGeometry(widget, "accountWidth", at.width);
      setGeometry(widget, "accountBottom", Math.max(0, innerHeight - at.top + 6));
    },
    opened(compact) {
      if (compact) try { history.pushState({ ...navigation.route, sheet: 1, scrollTop: currentScroll() }, ""); accountSheet = true; } catch {}
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
  };
  const shellChrome = SIDEBAR_ONLY ? null : createShellChrome({
    account: accountHost,
    navigate(destination) { go({ v: destination.key }); return true; },
    drawerOpened() { orderApply("side"); },
    drawerClosed() { setTimeout(() => { if (phone.matches && !document.body.classList.contains("drawer-open")) orderApply("side"); }, ORD_DRAWER_MS); },
    railChanged() { setRailMode(!railMode); renderNav(); },
  });
  if (shellChrome) shellChrome.mount(app);
  const accountChrome = shellChrome?.account ?? createAccountChrome(accountHost);
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
    if (shellChrome) { shellChrome.drawerAccount(ACCOUNT ? { account: ACCOUNT, compact: true, wide: wideMode, onWideChange: () => setWideMode(!wideMode) } : null); return; }
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
  const ordering = createOrdering(), ORD = ordering.scopes, ordTouch = { down: false }, byLast = (a, b) => b.last - a.last;
  const pageSig = () => JSON.stringify([query, groupBy, sessionFilters]);
  document.addEventListener("pointerdown", () => { ordTouch.down = true; }, true);
  for (const t of ["pointerup", "pointercancel"]) document.addEventListener(t, () => { ordTouch.down = false; }, true);
  window.addEventListener("blur", () => { ordTouch.down = false; });
  // A tab that comes back from the background applies what was held, once, before it paints, from the data it has. What the catch-up
  // poll brings after that is held like any other update.
  document.addEventListener("visibilitychange", () => { ordTouch.down = false; if (visible()) { orderApply("page"); orderApply("side"); } });
  // How many rows must move to put `rows` in `cmp` order: all of them but the longest run already in it.
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
  const orderScope = (name, sig, tie, state) => ordering.begin(name, sig, tie, state), orderList = orderRows;
  const sideRegion = () => $("#side-list") ?? $("#sidebar");
  // How long the wide screen's sidebar is left alone before it applies what it holds. Read once, at load, with 10 s as the default; a
  // browser check may set window.__semonOrderIdleMs to a finite number from 200 to 60000 ms before load; other values use the default.
  const ordIdleMs = window.__semonOrderIdleMs;
  const ORD_IDLE_MS = Number.isFinite(ordIdleMs) && ordIdleMs >= 200 && ordIdleMs <= 60000 ? ordIdleMs : 10000;
  const ORD_DRAWER_MS = 320; // the drawer's slide (0.24 s) and a little
  // Applies what a screen holds: the list is drawn sorted, from scratch.
  function orderApply(name) {
    const sc = ORD.get(name); if (!sc?.n || (name === "page" && (sc.tie !== navigation.route || navigation.rendered !== navigation.route || viewerEl || $("#page").hasAttribute("aria-busy")))) return;
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
    const nav = $("#nav"), destinations = [];
    // A session or a trace sits under Sessions, a machine under Machines.
    const under = { home: ["home"], analytics: ["analytics"], sessions: ["sessions", "session", "trace"], machines: ["machines", "machine"] };
    const item = (v, label, ic, count, hot) => { destinations.push({ key: v, label, icon: ic, href: v === "machines" && NAV_MACHINES ? NAV_MACHINES : urlOf({ v }), current: under[v].includes(navigation.route.v), count, hot }); };
    item("home", "Home", I.home, inbox().length, true);
    item("sessions", "Sessions", I.sessions);
    item("analytics", "Analytics", I.chart);
    item("machines", "Machines", I.machine, Object.keys(MACHINE).filter((m) => !MACHINE_UP[m]).length, true);
    if (shellChrome) shellChrome.update(destinations, railMode);
    else renderShellNavigation(nav, destinations, (destination) => { go({ v: destination.key }); return true; });
  }
  // Sessions match by name, repo, branch, machine, harness and the messages that started their turns.
  const sessMatch = (s, q) => !q || [s.name, s.repo, s.branch, MACHINE[s.machine], s.movedFrom ? MACHINE[s.movedFrom] : "", HARNESS[s.harness], s.role ? "role no repo" : "", ...(TURNS[s.id] ?? []).map((t) => t.start?.brief ?? t.u?.text ?? "")].join(" ").toLowerCase().includes(q.toLowerCase());
  // Built once per model and per render (both drop it) and shared: callers copy an array before reordering it.
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
  const COST_TIP = "What these tokens would cost at API rates. Subscriptions (Claude Max, ChatGPT plans) aren't billed this way.";
  // The icon is the only place this text is; it takes keyboard focus so the tip is reachable without a pointer.
  // An open parent lists its waiting children, then its running ones (at most 8), then the newest finished ones until three rows are
  // listed. "All N" opens the rest: a sheet on a phone, the whole list in the tree on a wide screen.
  const TREE_ACTIVE = 8, TREE_ROWS = 3;
  // The open session and the sessions above it. Only the open one is marked current; its ancestors are opened in the tree for this render
  // (nothing is saved) and are always listed, so the current row can always be found.
  function routedPath() {
    const current = navigation.route.v === "session" ? navigation.route.id : navigation.route.v === "trace" ? navigation.route.sid : null, ancestors = new Set();
    for (let id = current && SESS[current] ? parentOf(current) : null; id && SESS[id] && id !== current && !ancestors.has(id); id = parentOf(id)) ancestors.add(id);
    return { current, ancestors };
  }
  // Those ancestors open once per navigation, held in memory: a parent collapsed after that stays collapsed until the next one.
  let forcedOpen = { route: null, ids: new Set() };
  function forcedOpenIds() { if (forcedOpen.route !== navigation.route) forcedOpen = { route: navigation.route, ids: routedPath().ancestors }; return forcedOpen.ids; }
  // Waiting is 0, running 1, finished 2. A finished child with a waiting or running session below it ranks as that session does.
  // The one parent whose whole list is open in the tree (wide screens only). Nothing saves it: a reload starts with the short lists.
  // The parents above it stay listed and open (`expandedPath`) and everything below it is listed in full and open (`expandedUnder`), so
  // its "All N" and the rows it reveals agree.
  let expandedAll = null, revealedFor = null, expandedPath = new Set(), expandedUnder = new Set(), sideOrder = null;
  const ancestorsOf = (id) => { const out = new Set(); for (let p = id && SESS[id] ? parentOf(id) : null; p && SESS[p] && p !== id && !out.has(p); p = parentOf(p)) out.add(p); return out; };
  // Fills a parent's group and says whether it holds the parent's whole list, which sticks the parent's row (stickRow).
  function treeGroupSnapshot(parent, kids, children, depth, rail, open) {
    const { current, ancestors } = routedPath(), rank = new Map(kids.map((c) => [c.id, kidRank(c, children)]));
    const byRank = (a, b) => rank.get(a.id) - rank.get(b.id) || b.last - a.last, sorted = [...kids].sort(byRank), keep = new Set();
    for (const c of sorted) if (rank.get(c.id) < 2 && keep.size < TREE_ACTIVE) keep.add(c.id);
    for (const c of sorted) if (c.id === current || ancestors.has(c.id) || expandedPath.has(c.id)) keep.add(c.id);
    for (const c of sorted) if (keep.size < TREE_ROWS) keep.add(c.id);
    const listed = sorted.filter((c) => keep.has(c.id)), hidden = kids.length - listed.length;
    if (!hidden && expandedAll === parent.id) expandedAll = null; // nothing is left to open: "Show fewer" would have nothing to fold
    const full = hidden > 0 && !rail && (expandedAll === parent.id || expandedUnder.has(parent.id));
    // The rows keep the order they had (see "Stable order"); a child new to the list is held, unless it is the open session or leads to it.
    // A collapsed parent's children are nobody's to see: drawn sorted, not counted. A list new to a screen that has a snapshot is held
    // whole, with no "All N" row either, so that nothing appears in the tree.
    const key = (full ? "a:" : "k:") + parent.id, unseen = !!sideOrder?.keep && open && !full && !sideOrder.reseed && sideOrder.seen.has(parent.id) && !sideOrder.prev.has(key);
    const shown = sideOrder ? orderList(sideOrder, key, full ? sorted : listed, byRank, { must: new Set([current, ...ancestors, ...expandedPath]), quiet: !open, seed: full || sideOrder.reseed || !sideOrder.seen.has(parent.id) }) : full ? sorted : listed;
    const items = shown.map(child => buildLaneSnapshot(child, depth + 1, children, rail));
    return { items, all: kids.length === shown.length || full || unseen ? undefined : descendantsOf(parent.id, children).length, full: full && expandedAll === parent.id };
  }
  let recentSnapshot = { items: [], empty: false };
  const recentRenderer = createRecentRenderer($("#lanes"), {
    open: id => goSession(id),
    toggle(id, value) {
      if (!value) forcedOpenIds().delete(id);
      saveTreePref(id, value);
      if (!value && (expandedAll === id || expandedPath.has(id))) {
        expandedAll = null; renderLanes();
        $('#lanes .treeitem[data-id="' + CSS.escape(id) + '"] > .tree-row .tree-toggle')?.focus();
      } else {
        const change = items => items.map(item => ({ ...item, open: item.id === id ? value : item.open, children: item.children ? change(item.children) : undefined }));
        recentSnapshot = { ...recentSnapshot, items: change(recentSnapshot.items) }; recentRenderer.update(recentSnapshot);
      }
    },
    all(id, trigger) {
      if (!SESS[id]) return;
      if (phone.matches) { openKidsSheet(SESS[id], trigger); return; }
      expandedAll = id; renderLanes();
      $('#lanes .treeitem[data-id="' + CSS.escape(id) + '"] > .tree-row .tree-fewer')?.focus();
    },
    fewer(id) {
      expandedAll = null; renderLanes();
      const row = $('#lanes .srow[data-id="' + CSS.escape(id) + '"]'); scrollProgrammatically(() => row?.scrollIntoView({ block: "nearest" })); row?.focus({ preventScroll: true });
    },
  });
  // A phone's "All N": every session below the parent in one sheet, waiting first, then running, then finished, newest first in each.
  function openKidsSheet(parent, trigger) {
    const all = descendantsOf(parent.id, navigationTree().children), bucket = s => s.state === "wait" ? 0 : s.state === "work" ? 1 : 2;
    let picked = null;
    const rows = [...all].sort((a, b) => b.last - a.last).map(s => { const above = parentOf(s.id); return { id: s.id, name: s.name, state: s.state, stateLabel: STATE[s.state] ?? s.state, age: ago(s.last), bucket: bucket(s), under: above && above !== parent.id && SESS[above] ? SESS[above].name : undefined }; });
    const sheet = createKidsSheet(parent.name, rows, {
      matches(id, query) { return sessMatch(SESS[id], query); },
      select(id) { picked = id; pendingSessionOpen = id; sheet.dialog.close(); },
      opened(d) { viewerEl = d; document.documentElement.classList.add("viewer-open"); if (!SIDEBAR_ONLY) try { history.pushState({ ...navigation.route, sheet: 1, scrollTop: currentScroll() }, ""); } catch {} },
      closed(d) {
        document.documentElement.classList.remove("viewer-open");
        if (viewerEl === d) { viewerEl = null; if (!SIDEBAR_ONLY && history.state?.sheet) { skipPop = true; history.back(); } else if (pendingSessionOpen) { const id = pendingSessionOpen; pendingSessionOpen = null; goSession(id); } }
        if (!picked) ($('#lanes .tree-all[data-id="' + CSS.escape(parent.id) + '"]') ?? trigger).focus();
        if (LIVE.pending) refresh();
      },
    });
    sheet.show();
  }
  function buildLaneSnapshot(s, depth, children, rail) {
    const kids = children.get(s.id) ?? [], allKids = descendantsOf(s.id, children);
    const { current, ancestors } = routedPath(), saved = treePrefs[s.id];
    const open = forcedOpenIds().has(s.id) || expandedPath.has(s.id) || (typeof saved?.open === "boolean" ? saved.open : defaultTreeOpen(s.id, children) || expandedUnder.has(s.id));
    const group = kids.length && !rail ? treeGroupSnapshot(s, kids, children, depth, rail, open) : null;
    const parts = allKids.length ? childParts(allKids) : [], harness = Object.hasOwn(HARNESSES, s.harness) ? HARNESSES[s.harness] : null;
    const flag = kids.length && !rail ? (allKids.some(x => x.state === "wait") ? "wait" : allKids.some(x => x.state === "err") ? "err" : null) : null;
    const fields = [{ className: "row-duration", text: dur(s.start, s.state === "work" || s.state === "wait" ? null : s.last), priority: 1, tip: "Duration", icon: I.duration }];
    if (Object.keys(MACHINE).length > 1) fields.push({ className: "row-machine host", text: shortHost(s), priority: 2, tip: "Machine: " + hostOf(s), icon: I.machine });
    if (s.repo) fields.push({ className: "repo-short", text: s.repo, priority: 3, tip: "Repo: " + s.repo, icon: I.repo });
    return {
      id: s.id, name: s.name, label: [s.name, STATE[s.state] ?? s.state, HARNESS[s.harness] ?? s.harness, shortHost(s), ...parts].join(", "),
      state: s.state, stateLabel: STATE[s.state] ?? s.state, age: ago(s.last), model: shortModel(s.model ?? s.modelId), modelTip: "Model: " + modelIdOf(s),
      harness: harness ? { id: s.harness, name: harness.name, light: harness.icon.light, dark: harness.icon.dark, darkTheme: darkTheme() } : undefined,
      fields, current: current === s.id ? "page" : rail && ancestors.has(s.id) ? "true" : undefined, rail,
      childState: rail && allKids.some(x => x.state === "work" || x.state === "wait") ? urgentDescendant(s.id, children) ?? "work" : undefined,
      flag: flag ? { state: flag, tip: allKids.filter(x => x.state === "wait").length + " needs you · " + allKids.filter(x => x.state === "err").length + " failed" } : undefined,
      open, depth, children: group?.items, all: group?.all, stuck: !!group?.full && open,
    };
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
    recentSnapshot = { items: orderList(sideOrder, "lanes", lanes, byLast, { limit: 8, must: new Set([current, ...ancestors]) }).slice(0, 8).map(s => buildLaneSnapshot(s, 0, children, rail)), empty: !lanes.length };
    recentRenderer.update(recentSnapshot);
    if (!sideOrder.n) { clearTimeout(ordIdle); ordIdle = null; } else if (!ordIdle) ordIdleArm(); // (counted from when something was first held)
    const q = $("#q"); if (q && document.activeElement !== q) q.value = query;
    restoreLaneFocus(focus);
    // A stuck row covers the top of the sidebar: what is scrolled into view (the open session, after a navigation) stays clear of it.
    const stuck = box.querySelector(".tree-row.stuck"), navigated = revealedFor !== navigation.route; revealedFor = navigation.route;
    setGeometry($("#side-list") ?? $("#sidebar"), "scrollPaddingTop", stuck ? stuck.offsetHeight + 8 : null);
    if (stuck && navigated) scrollProgrammatically(() => box.querySelector('.srow[aria-current="page"]')?.scrollIntoView({ block: "nearest" }));
  }

  // ---- Top bar ---------------------------------------------------------------------------------------
  // The same on every page: the menu button (phones), the title, and at most two actions. A detail page adds a crumb up a
  // level and a second line of labels. Labels are information, never a control: their full values are in the tooltip and in
  // the session menu. On a session, Find takes over the bar and the filters sit under it as chips.
  const kindText = (s) => s.kind ?? HARNESS[s.harness];
  // The model the session is on (what the line's label abbreviates), so the label, its tip and the Details row name the same one; a session that used more than one is priced per model in the cost section.
  const modelIdOf = (s) => s.model ?? Object.keys(s.tokens_by_model ?? {})[0];
  const viewerBar = createViewerBar();
  function renderTopbar(title, crumb, opts = {}) {
    const s = opts.session ?? opts.traceSession;
    const account = ACCOUNT ? { account: ACCOUNT, compact: false, wide: wideMode, onWideChange: () => setWideMode(!wideMode) } : null;
    const mode = s && errOn(s.id) ? "errors" : s && findOpen ? "find" : "normal";
    const content = viewerBar.update({ mode, name: title, session: s?.id, state: s?.state, stateLabel: s ? STATE[s.state] : undefined, stateTip: s ? "Status: " + STATE[s.state] + " · " + turnsLabel(s) : undefined,
      showState: !!s && !opts.traceSession && !(phone.matches && opts.lineage?.length), ancestors: !phone.matches ? opts.lineage ?? [] : [], crumb: opts.lineage?.length ? undefined : crumb?.label,
      labels: opts.line2 ?? [], trace: !!opts.traceSession, analytics: !!opts.analytics, days: analyticsRange, query: find, count: matchText(matchCount()), filter: show.messages && show.tools ? "all" : show.messages ? "messages" : "steps", failed: s ? countOf(s, "errors") ?? 0 : 0, signals: s ? signalCount(s) : 0,
      errorMode: ERR.mode, errorText: ERR.notice ?? errText(), errorDisabled: ERR.listed && !ERR.slots.length, icons: I,
    }, {
      ancestor: goSession, crumb() { crumb?.go(); }, find() { findOpen = true; render(); $("#find")?.focus(); }, closeFind() { findOpen = false; find = ""; show = { ...SHOW_ALL }; render(); }, query(value) { find = value.toLowerCase(); render(); },
      filter(key) { if (key === "failures" || key === "signals") { openErrors(s.id, key === "signals" ? "signals" : "errors"); return; } show = key === "messages" ? { messages: true, tools: false, thinking: false } : key === "steps" ? { messages: false, tools: true, thinking: false } : { ...SHOW_ALL }; render(); },
      menu(trigger, runs) { openSessionMenu(SESS[s.id] ?? s, trigger, runs ? ".runs" : undefined); }, errors() { openErrors(s.id); }, closeErrors, step: stepErrors,
      range(days) { if (analyticsRange === days) return; const top = currentScroll(); analyticsRange = days; render(); restoreScroll(top); refreshAnalytics(true); },
    });
    const lead = { label: opts.traceSession ? "Back to " + s.name : "Open navigation", icon: opts.traceSession ? I.chev : I.menu, back: opts.traceSession ? () => goSession(s.id, navigation.route.turn) : undefined };
    shellChrome.topbar(mode === "normal" ? { titleSlot: content.titleSlot, actions: [content.actions], session: !!s, lead, account } : { mode: [content.mode], session: true, account, accountTarget: content.accountTarget });
    if (s && mode === "normal") requestAnimationFrame(() => { const line = $("#topbar .meta-line"); if (line) measureViewerBar($("#topbar")); });
  }
  // The label and buttons in place, so focus stays where it is.
  // ---- Errors mode: "N errors" steps through the session's failed steps ---------------------------------------------------------
  // The bar reads "Error k of N" with previous and next, and a close button (Escape). /api/tx?errors=1 says where every failed
  // step is (its slot), so a step on a page not loaded yet is reachable: a page next to the loaded range is added to it, one
  // further away replaces it with the page around the step. Each step is scrolled to the middle and marked, never opened;
  // its tool group opens so it shows. Closing puts back the pages, what was open and the scroll position from before. The
  // mode is its own controller, apart from find, so the two can become one mode later.
  const drawSessionBar = () => {
    const s = SESS[navigation.route.id]; if (navigation.route.v !== "session" || !s) return;
    renderTopbar(s.name, null, { session: s, lineage: lineageOf(navigation.route.id).slice(0, -1), line2: sessionLine(s) });
    setGeometry(document.documentElement, "barHeight", $("#topbar").offsetHeight); syncBarLine();
  };
  // A redraw keeps focus on the bar's control that had it.
  const keepFocus = (fn) => { const id = document.activeElement?.id; fn(); const n = id && document.getElementById(id); if (n && n !== document.activeElement) n.focus({ preventScroll: true }); };
  function centre(node) {
    if (!node.isConnected) return;
    const sc = scroller(), r = (node.querySelector(":scope > button") ?? node).getBoundingClientRect(), bottom = phone.matches ? window.innerHeight : $("#main").getBoundingClientRect().bottom;
    const d = (r.top + r.bottom) / 2 - (edge() + bottom) / 2; scrollProgrammatically(() => { if (Math.abs(d) >= 1) sc.scrollTop += d; });
    syncBarLine(); syncJump(); saveHistoryScroll();
  }
  const errorNavigation = createErrorNavigation({ navigation, TX, TXM, TOK, SESS, show, sidebarOnly: SIDEBAR_ONLY, page: () => $("#page"), drawSessionBar, render, keepFocus, countOf, capture, restore, opener, resetPagerInput, stopOpeningEndPin, clearFind() { find = ""; }, centre, fetchTx, dropTx, spread, tail });
  const { state: ERR, text: errText, on: errOn, open: openErrors, step: stepErrors, mark: markError, drop: dropErrors, close: closeErrors, live: errorsLive } = errorNavigation;
  const signalCount = (s) => Object.values(s?.signals ?? {}).reduce((n, x) => n + x, 0);
  // A label is information; one that leads somewhere (`act`) is a button that looks the same, with its hit area padded to the tap size.
  const turnsLabel = (s) => { const n = (TURNS[s.id] ?? []).filter(hasTurn).length; return n + (n === 1 ? " turn" : " turns"); };
  // On a phone the line of labels leaves the bar and the state is the small dot before the title. The dot names the state for a screen reader, and
  // its tip (a tap on a phone) adds the turn count. A desktop hides it, since the line shows the state there.
  // "Started 21:57 on <machine>" stays on one line: the machine name ellipsises (its tip, only while cut off, has the whole name).
  // A session's line: its state, then kind, model, failed steps, machine, branch and API-equivalent cost, each a plain label.
  const sessionLine = s => {
    const failed = countOf(s, "errors") ?? 0, runs = descendantsOf(s.id, sessionChildren()), cost = runs.length ? costForSessions([s, ...runs]) : costForSession(s.id), labels = [];
    const add = (key, text, tip, drop, extra = {}) => labels.push({ key, text, tip: tip ?? undefined, drop, ...extra });
    add("state", STATE[s.state], undefined, 0, { className: "state " + s.state, state: s.state, stateLabel: STATE[s.state] });
    add("kind", kindText(s), s.kind ? s.kind + " · " + HARNESS[s.harness] : undefined, 2);
    add("model", shortModel(s.model), "Model: " + modelIdOf(s) + (s.effort ? ". Reasoning effort: " + s.effort : ""), 3, { className: "meta-model", effort: s.effort });
    if (failed) add("errors", failed + " failed", failed + (failed === 1 ? " failed step" : " failed steps") + ": step through them", 0, { className: "lab-errs", action: "errors" });
    if (runs.length) add("runs", runs.length + (runs.length === 1 ? " run" : " runs"), runs.length + (runs.length === 1 ? " run" : " runs") + " under this session: open the list with their cost", 1, { className: "lab-runs", action: "runs" });
    add("machine", MACHINE[s.machine], "Machine: " + MACHINE[s.machine] + " · " + hostOf(s), 5);
    if (s.branch) add("branch", s.branch, "Branch: " + s.branch, 6);
    add("cost", costText(cost), "API-equivalent cost" + (runs.length ? ", with " + runs.length + (runs.length === 1 ? " run" : " runs") : "") + ". Details in the session menu.", 7); return labels;
  };
  const machineLine = m => { const here = onMachine(m), w = here.filter(s => s.state === "work").length, up = MACHINE_UP[m]; const sessions = here.length + (here.length === 1 ? " session" : " sessions");
    return [{ key: "state", text: up ? "Up" : "Not responding", drop: 0, className: "state " + (up ? "done" : "err"), state: up ? w ? "work" : "idle" : "err", stateLabel: STATE[up ? w ? "work" : "idle" : "err"] },
      { key: "activity", text: up ? w + " working · " + sessions : movedOff(m).length ? movedOff(m).length + " moved off" : [MACHINE_LAST[m] != null ? "Last seen " + clock(MACHINE_LAST[m]) : null, sessions].filter(Boolean).join(" · "), drop: 1 }]; };
  // What Find counts: the matches on the page when there is a search or a filter, else nothing.
  const matchCount = () => find || !show.messages || !show.tools || !show.thinking ? $("#page").querySelectorAll(".turns .msg, .turns .bubble, .turns .step:not(.bgend), .turns .event, .turns .child-card").length : null;
  const matchText = (n) => n == null ? "" : n ? n + (n === 1 ? " match" : " matches") : "No matches";
  // Find and filter: one mode. Search takes over the bar and the filters sit under it as chips, one choice at a time.
  // One observer for the current page title; the previous page's is disconnected so it can't flip the new bar.
  let titleObs = null;
  function observeTitle() { syncBarLine(); }
  // The bar's divider shows only once the page has scrolled.
  function syncBarLine() { const y = phone.matches ? window.scrollY : $("#main").scrollTop; $("#topbar").classList.toggle("scrolled", y > 4); }
  if (!SIDEBAR_ONLY) { window.addEventListener("scroll", syncBarLine, { passive: true }); $("#main").addEventListener("scroll", syncBarLine, { passive: true }); }
  if (!SIDEBAR_ONLY) window.addEventListener("resize", () => { const l2 = $("#topbar .meta-line"); if (l2 && navigation.route.v === "session") measureViewerBar($("#topbar")); syncLayoutPrefs(); syncJump(); }, { passive: true });

  // ---- Panels: one builder for the sheets and menus opened from the top bar --------------------------------------------
  // A phone gets a bottom sheet; a desktop a dialog, or for the session menu a panel that hangs from its button. Each is a
  // history entry, so back closes it without leaving the page, and a live update waits until it closes.
  function panel(title, opts = {}) {
    const chrome = createPanelChrome({ title, className: opts.cls, label: opts.label, sub: opts.sub }, {
      opened() { viewerEl = chrome.dialog; document.documentElement.classList.add("panel-open"); try { history.pushState({ ...navigation.route, sheet: 1, scrollTop: currentScroll() }, ""); } catch {} },
      closed() { const d = chrome.dialog; document.documentElement.classList.remove("panel-open"); opts.onClose?.(); if (viewerEl === d) { viewerEl = null; if (history.state?.sheet) { skipPop = true; history.back(); } else if (pendingSessionOpen) { const id = pendingSessionOpen; pendingSessionOpen = null; goSession(id); } } if (LIVE.pending) refresh(); },
    });
    return { d: chrome.dialog, body: chrome.body, show: () => chrome.show() };
  }

  // The session menu: actions, then details, then cost. It is the one place for all three.
  const RUNS_CAP = 5;
  // Shown once in the UI, at the foot of the session menu; NOTICE.md and the README carry it too. The harness marks are the property of their owners.
  const TRADEMARK_NOTICE = "Third-party trademarks are the property of their respective owners. Semon is not affiliated with or endorsed by these companies.";
  function openSessionMenu(s, anchor, scrollTo) {
    const kids = descendantsOf(s.id, sessionChildren());
    const { d, body, show: open } = panel(s.name, { cls: "anchored session-menu", label: "Session menu for " + s.name, sub: [STATE[s.state], kindText(s), shortModel(s.model)].join(" · "), onClose: () => { anchor?.setAttribute("aria-expanded", "false"); anchor?.focus({ focusVisible: false }); } });
    const traceTurn = navigation.route.v === "trace" ? TURN.get(navigation.route.turn) : (() => { const top = $("#topbar").getBoundingClientRect().bottom; const nodes = [...document.querySelectorAll("#page .turn[data-turn]")].filter(n => !n.closest(".cw-body")); const visible = nodes.find(n => n.getBoundingClientRect().bottom > top); return TURN.get(visible?.dataset.turn) ?? (TURNS[s.id] ?? []).at(-1); })();
    const actions = [], addAction = (key, text, icon, className, note, checked, dot) => actions.push({ key, text, icon, className, note, checked, dot });
    if (traceTurn?.out.length) addAction("trace", "Trace this turn", I.trace, "menu-trace");
    const command = s.harness === "codex" ? "codex resume " + s.id : "claude --resume " + (s.sessionId ?? s.id);
    addAction("copy", "Copy resume command", I.copy);
    if (s.harness === "claude") addAction("external", "Open in claude.ai", I.ext);
    if (!phone.matches) addAction("wide", "Wide transcript", I.wide, undefined, undefined, wideMode);
    const signals = signalCount(s); if (signals) addAction("signals", signals + " signals", undefined, "menu-signals", "Step through");
    const errorCount = countOf(s, "errors") ?? 0;
    if (phone.matches) { if (errorCount) addAction("errors", errorCount + " failed", undefined, "menu-errors", "Step through", undefined, "err"); if (kids.length) addAction("runs", "Runs · " + kids.length, I.stack, "menu-runs"); }
    const machine = MACHINE[s.machine] ?? s.machine ?? "Unknown machine", calls = countOf(s, "calls");
    const details = [["Status", STATE[s.state] + " · " + turnsLabel(s)], ...(s.kind ? [["Kind", s.kind]] : []), ["Harness", HARNESS[s.harness] ?? s.harness, false, harnessSnapshot(s.harness)], ["Model", modelIdOf(s), true], ...(s.effort ? [["Effort", s.effort]] : []), ["Machine", machine + (hostOf(s) !== machine ? " · " + hostOf(s) : "") + (s.movedFrom ? " (moved from " + (MACHINE[s.movedFrom] ?? s.movedFrom) + ")" : "")], ["Directory", s.cwd ?? s.dir ?? s.directory, true], [s.worktree ? "Worktree" : "Branch", branchOf(s), true], ["Tool calls", calls == null ? "—" : String(calls)], ...(errorCount ? [["Errors", String(errorCount)]] : []), ["Started", clock(s.start)], ["Duration", dur(s.start, s.state === "work" ? null : s.last)], ["Process id", s.pid, true], ["Session id", s.sessionId ?? s.id, true], ...Object.entries(s.signals ?? {}).map(([kind, n]) => ["Signals · " + kind, String(n)])].filter(([, value]) => value != null && value !== "").map(([label, value, mono, harness]) => ({ label, value: String(value), mono, harness }));
    renderSessionMenu(body, { actions, command, path: phone.matches ? lineageOf(s.id).map(a => ({ id: a.id, name: a.name, harness: a.harness, harnessName: HARNESS[a.harness] ?? a.harness })) : [],
      status: STATE[s.state] + " · " + turnsLabel(s), state: s.state, stateLabel: STATE[s.state], details, cost: costSnapshot(s, kids), notice: TRADEMARK_NOTICE, icons: { chevron: I.chev, copy: I.copy } }, {
      wide: () => wideMode,
      session(id) { pendingSessionOpen = id; d.close(); },
      action(key) {
        if (key === "trace") { afterPop = () => goTrace(traceTurn.id); d.close(); }
        else if (key === "wide") setWideMode(!wideMode);
        else if (key === "signals" || key === "errors") { d.close(); openErrors(s.id, key === "signals" ? "signals" : "errors"); }
        else if (key === "runs") body.querySelector(".runs")?.scrollIntoView({ block: "start" });
      },
    });
    anchor?.setAttribute("aria-expanded", "true");
    open(); if (scrollTo) body.querySelector(scrollTo)?.scrollIntoView({ block: "nearest" }); return d;
  }
  // 739,682 reads "740k" and 12,422,228 "12.4M"; the exact figure is the cell's tooltip.
  const MENU_KINDS = [["Input", ["input"]], ["Output", ["output"]], ["Cache write", ["cache_write_5m", "cache_write_1h"]], ["Cache read", ["cache_read"]], ["Web search", ["web_search"]]];
  const COST_NOTE = "What these tokens would cost at API rates. Subscriptions aren't billed this way.";
  function costSnapshot(s, kids) {
    const own = costForSession(s.id), all = costForSession(s.id, true), missing = costMissing(all), details = [];
    if (kids.length) details.push({ label: "This session", value: costText(own) }, { label: kids.length === 1 ? "Its run" : "Its " + kids.length + " runs", value: costText(costForSessions(kids)) });
    const reports = s.reported_runs ?? [], reported = reports.filter(r => Number.isFinite(r.cost_usd));
    if (reports.length) details.push({ label: HARNESS[s.harness] + "'s own figure", value: reported.length ? asMoney(reported.reduce((n, r) => n + r.cost_usd, 0)) + (reported.length === 1 ? ", last run" : ", last " + reported.length + " runs") : "not reported" });
    const check = [...(s.cost_check ?? [])].reverse().find(c => c.ok === false && Number.isFinite(c.computed_usd) && Number.isFinite(c.reported_usd));
    const mismatch = check ? "Semon's estimate for that run is " + (check.reported_usd === 0 ? 100 : Math.round(Math.abs(check.computed_usd - check.reported_usd) / Math.abs(check.reported_usd) * 100)) + "% " + (check.computed_usd > check.reported_usd ? "above" : "below") + " " + HARNESS[s.harness] + "'s figure: API rates differ from what a plan is charged." : undefined;
    const children = sessionChildren(), runs = [], walk = (id, depth) => { for (const c of [...(children.get(id) ?? [])].sort((a, b) => b.last - a.last)) { const cost = costText(costForSession(c.id)); runs.push({ id: c.id, depth, label: "Open " + c.name + ", " + kindText(c) + ", " + STATE[c.state] + ", " + cost, name: c.name, kind: kindText(c), state: c.state, stateLabel: STATE[c.state], cost }); walk(c.id, depth + 1); } }; if (kids.length) walk(s.id, 0);
    const models = Object.entries(all.by_model ?? {}).map(([id, model]) => {
      const priced = model.usd != null && !missing.includes(id), rows = [];
      for (const [label, keys] of MENU_KINDS) { const tokens = keys.reduce((n, k) => n + (Number(model.tokens?.[k]) || 0), 0), usd = keys.reduce((n, k) => n + (Number(model.usd_by_kind?.[k]) || 0), 0); if (tokens === 0 && (!priced || usd < 0.005)) continue; rows.push({ label, count: tokens ? compactCount(tokens) : "", exact: tokens ? tokens.toLocaleString() : undefined, cost: priced ? asMoney(usd) : "—" }); }
      return { id, rows };
    });
    return { figure: costText(kids.length ? all : own), caption: kids.length ? "this session and its " + kids.length + (kids.length === 1 ? " run" : " runs") : "this session",
      note: COST_NOTE + (missing.length ? " No price for " + missing.join(", ") + "." : ""), details, mismatch, runs, models, includesRuns: !!kids.length };
  }

  // ---- Home: what needs you, then what is running ------------------------------------------------------
  const upCount = () => Object.keys(MACHINE).filter((m) => MACHINE_UP[m]).length;
  let allAnswered = false;
  function harnessSnapshot(id) {
    const h = Object.hasOwn(HARNESSES, id) ? HARNESSES[id] : null;
    return h ? { id, name: h.name, light: h.icon.light, dark: h.icon.dark, darkTheme: darkTheme() } : undefined;
  }
  function liveSnapshot(s, showMachine) {
    const cur = (TURNS[s.id] ?? []).at(-1), msg = cur?.start?.brief ?? cur?.u?.text;
    const inb = cur?.start ?? (cur?.u ? { from: "you" } : H.find((h) => h.to === s.id && h.kind !== "move"));
    return { id: s.id, name: s.name, state: s.state, stateLabel: STATE[s.state] ?? s.state,
      status: s.state === "work" ? HARNESS[s.harness] : ago(s.last), harness: s.state === "work" ? harnessSnapshot(s.harness) : undefined,
      detail: [showMachine ? MACHINE[s.machine] : null, inb ? (inb.from === "you" ? "for you" : "for " + nameOf(inb.from)) : null, msg ? oneLine(msg) : null].filter(Boolean).join(" · "),
      activity: s.state === "work" ? s.activity : undefined };
  }
  function inboxSnapshot(h, quiet) {
    const sid = h.kind === "move" ? h.to : h.from, t = HOLDS.get(h.id), s = SESS[sid];
    const parts = [], part = (className, text, tip) => parts.push({ className, text, tip });
    let path;
    if (h.kind === "ask") { path = I.ask; part("who", nameOf("you")); part("verb", " asked "); part("who", nameOf(h.to)); }
    else if (h.kind === "spawn" || h.kind === "relay") { path = I.out; part("who", nameOf(h.from)); part("verb", h.kind === "spawn" ? " handed off to " + (SESS[h.to]?.kind === "Subagent" ? "subagent" : SESS[h.to]?.kind ?? "") + " " : " relayed to "); part("who", nameOf(h.to)); }
    else if (h.kind === "move") {
      path = I.move; const short = machineShorts([h.fromMachine, h.toMachine].map(id => [id, MACHINE[id] ?? id]));
      part("verb", "Semon moved "); part("who", nameOf(h.to)); part("verb", " from "); part("verb mach", short.get(h.fromMachine), "Machine: " + (MACHINE[h.fromMachine] ?? h.fromMachine)); part("verb", " to "); part("verb mach", short.get(h.toMachine), "Machine: " + (MACHINE[h.toMachine] ?? h.toMachine));
    } else {
      path = h.status === "done" && (h.ask === "question" || h.ask === "decision") ? I.done : h.ask === "question" ? I.qc : h.ask === "decision" ? I.decide : I.result;
      part("who", nameOf(h.from)); part("verb", { question: " asked you", result: " sent you a result", decision: " needs your decision" }[h.ask]);
    }
    const answer = quiet ? answersOf(h) : null, root = t ? traceRoot(t) : null, msg = root?.start?.from === "you" ? root.start.brief : root?.u?.text;
    return { id: h.id, quiet, icon: quiet && h.kind === "toyou" ? I.done : path, parts, age: ago(h.at), preview: preview(h.brief),
      answer: answer ? answer.length ? "You answered: " + answer.join(" · ") : "Answered · reply not in these logs" : undefined,
      origin: root ? msg ? { message: oneLine(msg) } : { text: "Started by " + nameOf(root.start ? root.start.from : root.sid) } : undefined,
      context: [HARNESS[s.harness], MACHINE[s.machine]].join(" · "), harness: harnessSnapshot(s.harness), trace: t?.out.length ? t.id : undefined };
  }
  const activityHost = {
    session: goSession, committed: observeTitle, trace: goTrace,
    inbox(id) { const h = H.find(h => h.id === id) ?? inbox().find(h => h.id === id); if (!h) return; if (isResult(h)) markSeenResults([h]); goSession(h.kind === "move" ? h.to : h.from, HOLDS.get(h.id)?.id); },
    answered() { allAnswered = true; render(); },
  };
  function renderHome(page) {
    const open = inbox(), running = working(), many = Object.keys(MACHINE).length > 1;
    const rows = orderList(orderScope("page", pageSig(), navigation.route, pageState()), "working", running, byLast);
    const done = H.filter(h => h.kind === "toyou" && h.status === "done").sort((a, b) => b.at - a.at);
    renderHomeScreen(page, { waiting: open.length, working: running.length, up: upCount(), machines: Object.keys(MACHINE).length,
      inbox: open.map(h => inboxSnapshot(h, false)), live: rows.map(s => liveSnapshot(s, many)),
      answered: (allAnswered ? done : done.slice(0, 3)).map(h => inboxSnapshot(h, true)), totalAnswered: done.length, allAnswered }, activityHost);
  }
  // ---- Machines: where sessions run, and what happens when a machine goes away ------------------------------
  function renderMachines(page) {
    const ms = Object.keys(MACHINE).sort((a, b) => MACHINE_UP[a] - MACHINE_UP[b]);
    const rows = ms.map((m) => {
      const here = onMachine(m), w = here.filter((s) => s.state === "work").length, up = MACHINE_UP[m], state = !up ? "err" : w ? "work" : "idle";
      const mv = movedOff(m).length, mh = movesOf(m).find((h) => h.fromMachine === m);
      return { id: m, name: MACHINE[m], state, stateLabel: STATE[state] ?? state, status: !up ? "offline" : w ? "up" : "idle",
        detail: up ? [w + " working", here.length + (here.length === 1 ? " session" : " sessions")].join(" · ") : ["Not responding" + (mh ? " since " + clock(mh.at) : MACHINE_LAST[m] != null ? " since " + clock(MACHINE_LAST[m]) : ""), mv ? mv + (mv === 1 ? " session" : " sessions") + " moved off" : null].filter(Boolean).join(" · ") };
    });
    renderMachinesScreen(page, { rows, up: upCount(), working: working().length, admin: ADMIN }, { machine(id) { go({ v: "machine", id }); }, admin(href) { location.assign(href); }, committed: observeTitle });
  }
  function renderMachine(page, m) {
    const here = onMachine(m), off = movedOff(m), moves = movesOf(m);
    const ordered = orderList(orderScope("page", pageSig(), navigation.route, pageState()), "machine:" + m, here, byState);
    renderMachineScreen(page, { name: MACHINE[m], totalSessions: here.length, sessions: ordered.map(s => liveSnapshot(s, false)),
      off: off.map(s => ({ ...liveSnapshot(s, false), detail: "Now on " + MACHINE[s.machine] })), moves: moves.map(h => inboxSnapshot(h, true)) }, activityHost);
  }

  // ---- Trace: one turn and what it set off -------------------------------------------------------------------------
  // The root is the turn. Each spawn or relay it sent leads to the turn that handoff started in the receiving session,
  // and on down from there; a message to you is a leaf. One rail, as everywhere: depth shows as a smaller node.
  // One observer measures expandable messages, and releases detached nodes after a redraw.
  const RUN_EXPANDED = new Map();
  const { agentSnapshot } = createTraceCalculations({ sessions: SESS, turns: TURNS, starts: STARTS }, domain, () => NOW, RUN_EXPANDED, HARNESS, STATE);
  function renderTrace(page, id) {
    const root = TURN.get(id);
    if (!root) { renderTraceScreen(page, { empty: true, hops: [] }, { ...sentenceHost, committed: observeTitle, fold() {} }); return; }
    const visited = new Set(), read = turn => { if (!turn || visited.has(turn.id)) return; visited.add(turn.id); markSeenResults(turn.out); for (const h of turn.out) if (h.kind === "spawn" || h.kind === "relay") read(STARTS.get(h.id)); }; read(root);
    const seen = new Set([root.id]), sessions = new Set([root.sid]), scope = new Set([root.sid]), reached = new Set([root.id]); let n = 0;
    const reach = turn => { for (const h of turn.sent) { scope.add(h.kind === "toyou" ? h.from : h.to); const child = h.kind === "spawn" || h.kind === "relay" ? STARTS.get(h.id) : null; if (child && !reached.has(child.id)) { reached.add(child.id); reach(child); } } }; reach(root);
    const meta = (state, text, sid, turn, note) => { const s = SESS[sid], label = machineLabel(s, scope); return { state, stateLabel: STATE[state] ?? state, text,
      chip: s ? [s.kind ?? HARNESS[s.harness], label].filter(Boolean).join(" · ") : undefined, chipClass: s ? hcls(sid) : undefined, tip: label ? "Machine: " + hostOf(s) : undefined,
      harness: s ? harnessSnapshot(s.harness) : undefined, note: note ?? undefined, session: s && !s.stub ? sid : undefined, turn: turn?.id, name: s?.name }; };
    const start = root.start, text = start ? start.brief : root.u?.text, initial = start ? sentenceSnapshot(start, null) : root.u ? sentenceSnapshot({ kind: "ask", to: root.sid }, null) : { icon: I.more, parts: [{ className: "who", text: SESS[root.sid].name }, { className: "verb", text: " · a turn whose start isn't in these logs" }] };
    const outcome = turnEnd(root), hops = [{ key: "root:" + root.id, className: "k-root", icon: initial.icon, parts: initial.parts, nodeClass: hcls(start ? start.from : root.u ? "you" : root.sid), turn: root.id, handoff: start?.id, time: start ? clock(start.at) : undefined, brief: text || undefined, meta: meta(outcome?.st ?? "idle", outcome?.text ?? "Nothing recorded", root.sid, root) }];
    const walk = turn => { for (const h of turn.sent) {
      const result = h.kind === "toyou" && h.ask === "result", child = h.kind === "spawn" || h.kind === "relay" ? STARTS.get(h.id) : null, target = h.kind === "toyou" ? h.from : h.to;
      const sentence = result ? { icon: I.result, parts: [{ className: "verb", text: statWord(h) }] } : sentenceSnapshot(h, null);
      const hop = { key: h.id, className: "child k-" + h.kind + " s-" + h.status + (child || h.kind === "toyou" || h.kind === "move" ? "" : " stub"), icon: sentence.icon, parts: sentence.parts, nodeClass: hcls(target), handoff: h.id, turn: child?.id, time: clock(h.at) }; hops.push(hop);
      if (result) { n++; continue; }
      hop.brief = h.brief; hop.answers = answersOf(h) ?? undefined; hop.result = h.result || undefined;
      if (h.kind === "move") { hop.meta = meta("done", "Moved", h.to, turn); continue; }
      n++;
      if (h.kind === "toyou") { hop.meta = meta(isResult(h) ? SEEN_RESULTS.has(h.id) ? "read" : "new" : h.status === "done" ? "done" : h.status, statWord(h), h.from, turn); continue; }
      sessions.add(h.to); const end = child && turnEnd(child); hop.meta = meta(end ? end.st : h.status === "done" ? "done" : h.status, end ? end.text : statWord(h), h.to, child, child ? undefined : "Its turn isn't in these logs");
      if (child && !seen.has(child.id)) { seen.add(child.id); walk(child); }
    } }; walk(root);
    renderTraceScreen(page, { empty: false, hops, agents: agentSnapshot(root), summary: [sessions.size + (sessions.size === 1 ? " session" : " sessions"), n + (n === 1 ? " handoff" : " handoffs"), [...machineLabels(scope).values()].join(", ")].filter(Boolean).join(" · ") }, { ...sentenceHost, committed: observeTitle,
      fold(id) { RUN_EXPANDED.get(root.id)?.add(id); renderTrace(page, root.id); },
    });
    return [sessions.size + (sessions.size === 1 ? " session" : " sessions"), n + (n === 1 ? " handoff" : " handoffs"), [...machineLabels(scope).values()].join(", ")].filter(Boolean).join(" · ");
  }
  // ---- Session page --------------------------------------------------------------------------------------------------
  function renderSession(page, sid, opts = {}) {
    markSeenResults(H.filter(h => isResult(h) && h.from === sid));
    const raw = new Map(transcriptEntries(TX[sid] ?? [], sid).map(e => [e.slot != null ? sid + "#slot:" + e.slot : e.key, e]));
    renderSessionScreen(page, transcriptSnapshot(sid, opts), { ...sentenceHost, committed: observeTitle, trace: goTrace,
      toolAll(key, label) { const e = raw.get(key); if (e) { const [ic,v] = verb(e.name); openStepViewer(e, v, ic, label); } },
      script(key) { const e = raw.get(key); if (e) openScript(e); }, image: openImage,
      background(call, trigger) { const target = trigger.closest('section[aria-label="Transcript"]')?.querySelector('.step[data-tid="' + CSS.escape(call) + '"]'); if (!target) return; stopOpeningEndPin(); for (let parent = target.parentElement; parent; parent = parent.parentElement) { const toggle = opener(parent); if (toggle?.getAttribute("aria-expanded") === "false") toggle.click(); } centre(target); target.classList.add("flash"); setTimeout(() => target.classList.remove("flash"), 1500); },
      pager(button) { loadPager(button, true); }, jump: jumpToLatest,
    });
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
  function transcriptSnapshot(sid, opts = {}) {
    const entries = transcriptEntries(TX[sid] ?? [], sid), blocks = []; let tx = [], cur = null, loose = 0;
    const hit = text => !find || String(text ?? "").toLowerCase().includes(find);
    const filtering = !!find || !show.messages || !show.tools || !show.thinking;
    const entryKey = e => e.slot != null ? sid + "#slot:" + e.slot : e.key;
    const keyed = (view, e) => ({ ...view, key: e.key, entryKey: e.key ? entryKey(e) : undefined });
    const endedCalls = new Set(entries.filter(e => e.k === "bgend").map(e => e.call));
    let run = [], masked = false;
    const flush = () => {
      if (!run.length) return;
      const summary = [], ends = run.filter(r => r.end), counts = new Map();
      if (run.length > 1 && !filtering) {
        for (const r of run.filter(r => !r.end)) { const [, , p, one, many] = toolInfo(r.k), c = counts.get(p) ?? { n: 0, one, many }; c.n++; counts.set(p, c); }
        if (ends.length) {
          const outcomes = [[ends.filter(r => r.state === "failed").length, "failed"], [ends.filter(r => r.state === "killed").length, "stopped"]].filter(([n]) => n).map(([n, word]) => n + " " + word);
          summary.push({ text: "Finished ", className: "long" }, { text: ends.length + " background" }, { text: " command" + (ends.length === 1 ? "" : "s"), className: "long" });
          if (outcomes.length) summary.push({ text: " (" + outcomes.join(", ") + ")", className: "tt-counts" });
          for (const [p, c] of counts) summary.push({ text: ", " }, { text: p + " ", className: p === "ran" ? "long" : undefined }, { text: c.n + " " + (c.n === 1 ? c.one : c.many) });
        } else { const text = [...counts].map(([p, c]) => p + " " + c.n + " " + (c.n === 1 ? c.one : c.many)).join(", "); summary.push({ text: text[0].toUpperCase() + text.slice(1) }); }
      }
      const failed = run.filter(r => !r.end && r.err && (!r.bg || !endedCalls.has(r.tid))).length;
      const live = run.some(r => r.bg) ? run.filter(r => r.live).sort((a,b) => (elapsedMs(b.secs) ?? 0) - (elapsedMs(a.secs) ?? 0))[0] : run.find(r => r.live);
      tx.push({ kind: "group", key: run[0].key ? "g:" + run[0].key : undefined, entryKey: run[0].view.entryKey ? "g:" + run[0].view.entryKey : undefined, entries: run.map(r => r.view), lone: run.length === 1 && !filtering, summary, background: !!ends.length, failed, running: live?.secs, stack: I.stack, chevron: I.chev }); run = [];
    };
    const firsts = new Map((TURNS[sid] ?? []).filter(t => t.entries[0]?.key).map(t => [t.entries[0].key, t]));
    const owner = opts.only ? new Map((TURNS[sid] ?? []).flatMap(t => t.entries.map(e => [e.key, t.id]))) : null;
    const content = values => values.some(v => v.kind !== "label" || v.className === "harness-note");
    const closeTurn = () => {
      flush(); if (!cur) { if (tx.length) blocks.push({ kind: "loose", key: "loose:" + loose++, entries: tx }); tx = []; return; }
      const { t, view } = cur, end = turnEnd(t), returned = t.entries.some(e => e.k === "end" && /^Returned to /.test(e.text ?? ""));
      if (!filtering) { if (end?.st === "err" && !returned) view.error = end.text; if (t.out.length) view.trace = t.id; }
      view.entries = tx; if (!filtering || content(tx)) blocks.push({ kind: "turn", turn: view }); cur = null; tx = []; masked = false;
    };
    const openTurn = t => {
      closeTurn(); const h = t.start, view = { id: t.id, entries: [], traceIcon: I.trace, stateLabel: STATE.err };
      if (t.u || h?.kind === "ask") view.label = "Your message" + (h ? " at " + clock(h.at) : "");
      else if (h) view.header = { parts: sentenceSnapshot(h, sid, true).parts, mark: SESS[h.from] ? harnessSnapshot(SESS[h.from].harness) : undefined, time: clock(h.at) };
      cur = { t, view };
    };
    const toolStep = (e, v, ic, live) => {
      const bg = e.bg, bgRunning = bg?.state === "running", waiting = live && SESS[sid]?.state === "wait";
      const waitingText = SESS[sid]?.waiting_for?.includes("permission") ? "Waiting on permission" : "Waiting for your input";
      const status = bg ? null : waiting ? waitingText : live ? e.secs : e.unfinished ? "no result" : e.exit != null ? "exit " + e.exit + " · " + e.secs : e.ok ? e.secs : e.ok === null ? "exit unknown · " + e.secs : "failed · " + e.secs;
      const title = e.title ? String(e.title) : null, command = e.in ?? e.arg;
      const firstLine = typeof command === "string" ? command.split(/\r\n|\n|\r/).find(line => line.trim()) : null;
      const bgSecs = bgRunning && bg.since != null ? running(NOW - bg.since) : bg?.secs;
      return { className: "step" + (bg ? (bg.state === "failed" ? " err" : "") + " background" + (bgRunning ? " background-running" : "") : live ? " live" : e.ok || e.ok === null ? "" : " err"),
        key: e.key, entryKey: e.key ? entryKey(e) : undefined, tid: e.tid,
        sid: live || bgRunning ? sid : undefined, since: live ? e.since : bgRunning ? bg.since : undefined,
        running: live || bgRunning, background: !!bg, waiting: SESS[sid]?.state === "wait", named: !!title,
        prefix: title ? waiting ? waitingText + ": " : live ? "Running: " : e.ok === false ? "Failed: " : v + ": " : undefined,
        verb: waiting ? "Waiting" : live ? verbNow(e.name) : v, label: title ?? e.arg, tip: title && firstLine != null ? firstLine.slice(0, 200) : undefined,
        status: status == null ? null : String(status), backgroundStatus: bg ? backgroundText({ ...bg, secs: bgSecs }) : undefined, icon: I[ic], chevron: I.chev, data: e,
      };
    };
    for (const e of entries) {
      if (owner && !opts.only.has(owner.get(e.key))) continue;
      if (isGap(e)) { closeTurn(); if (!filtering) tx.push({ kind: "label", className: "divider", text: e.text }); closeTurn(); continue; }
      if (firsts.has(e.key)) openTurn(firsts.get(e.key));
      if (e.k === "think" && (!show.thinking || find)) continue;
      if (e.k === "signal") { const label = signalLabel(e); if (hit(label)) tx.push(keyed({ kind: "label", className: "signal-marker", text: label }, e)); continue; }
      if (e.k === "think" && isMaskedThought(e)) { if (!masked) { masked = true; tx.push(keyed({ kind: "thought", mode: "masked", label: "Thinking hidden by the harness" }, e)); } continue; }
      if (e.k === "bgend") {
        if (!show.tools || !hit(e.label ?? "")) continue;
        const word = e.state === "failed" ? "failed" : e.state === "killed" ? "stopped" : "completed";
        const view = keyed({ kind: "background", failed: e.state === "failed", label: "Background command " + word + " · " + (e.label ?? ""), call: e.call, loaded: entries.some(entry => entry.k === "tool" && entry.tid === e.call) }, e);
        run.push({ view, end: true, state: e.state, key: e.key }); continue;
      }
      if (e.k === "tool") {
        if (!show.tools || !hit(e.name + " " + (e.title ?? "") + " " + e.arg + " " + (e.in ?? "") + " " + (e.out ?? ""))) continue;
        const [ic, v] = verb(e.name), view = keyed({ kind: "tool", step: toolStep(e, v, ic, !!e.live && !e.bg) }, e);
        if (e.live && !e.bg) run.push({ view, v, k: e.name, live: true, secs: e.secs, key: e.key });
        else if (e.bg) { const live = e.bg.state === "running"; run.push({ view, v, k: e.name, err: e.bg.state === "failed", bg: true, tid: e.tid, live, secs: live && e.bg.since != null ? running(NOW - e.bg.since) : e.bg.secs ?? e.secs, key: e.key }); }
        else run.push({ view, v, k: e.name, err: e.ok === false, key: e.key }); continue;
      }
      flush();
      if (e.k === "u" || e.k === "a") { if (!show.messages || !hit(e.text)) continue; tx.push(keyed({ kind: "message", flavor: e.k === "u" ? "user" : "assistant", text: e.text, images: e.k === "u" ? attachmentSnapshot(e) : undefined }, e)); }
      else if (e.k === "think") tx.push(keyed(isPendingThought(e) ? { kind: "thought", mode: "pending" } : { kind: "thought", mode: "readable", label: thoughtLabel(e.displaySecs), text: thoughtText(e) }, e));
      else if (e.k === "harness") { if (!show.messages || find) continue; tx.push(keyed({ kind: "label", className: "harness-note", text: "Harness text added before the prompt (" + e.label + ")" }, e)); }
      else if (e.k === "end") { if (!filtering) tx.push(keyed({ kind: "label", className: "divider", text: e.text }, e)); }
      else if (e.k === "h") {
        const h = HID.get(e.id); if (!h || !hit(h.brief + " " + (h.result ?? ""))) continue;
        if (h.kind === "ask") { if (!show.messages) continue; tx.push(keyed({ kind: "message", flavor: "user", text: h.brief, images: attachmentSnapshot(e) }, e)); if (cur?.t.start === h) tx.push({ kind: "label", className: "msg-tm", text: clock(h.at) }); continue; }
        if (cur && cur.t.start === h) { if (show.messages) tx.push(keyed({ kind: "message", flavor: "incoming", text: h.brief, handoff: h.id }, e)); continue; }
        if (h.kind === "spawn" && h.from === sid && SESS[h.to]) { if (show.tools) tx.push(keyed(childSnapshot(h, SESS[h.to]), e)); continue; }
        if (find && h.kind === "move" || !show.messages && h.kind !== "move") continue;
        const sentence = sentenceSnapshot(h, sid, true);
        tx.push(keyed({ kind: "event", handoff: h.id, className: "event" + (h.kind === "toyou" && h.status === "wait" ? " waiting" : "") + (h.kind === "move" ? " move" : ""), icon: sentence.icon, parts: sentence.parts, time: clock(h.at), brief: preview(h.brief), result: h.result || undefined, resultLabel: h.kind === "relay" ? "Reply: " : "Returned: ", answers: answersOf(h) ?? undefined, waiting: h.kind === "toyou" && h.status === "wait", stateLabel: STATE.wait }, e));
      }
    }
    closeTurn();
    const range = TXM[sid], s = SESS[sid], origin = originHandoff(sid);
    return { id: sid, name: s.name, blocks, order: (TURNS[sid] ?? []).map(t => t.id), dirty: opts.only,
      before: range?.from > 0 ? pagerSnapshot(sid, "before") : undefined, after: range && range.to < range.total ? pagerSnapshot(sid, "after") : undefined,
      started: !range?.from && !filtering ? { lead: "Started " + clock(s.start) + " on\u00a0", machine: MACHINE[s.movedFrom ?? s.machine] } : undefined,
      empty: !opts.only && !blocks.some(b => content(b.kind === "turn" ? b.turn.entries : b.entries)) ? find ? "Nothing matches “" + find + "”." : "Nothing to show with these filters." : undefined,
      footer: showsFooter(s, origin) ? footerSnapshot(s, origin) : undefined };
  }
  // The whole tool call. A phone gets a full-screen sheet and a wider screen a dialog; either way it is a history entry,
  // so the back gesture closes it without leaving the page.
  let viewerEl = null, skipPop = false;
  // What a step shows opened: what was asked first (the command, the file, the input), then what came back. A failed command
  // shows the end of its output, where the failure is; anything else shows the start. "View all" opens the whole call.
  // The whole call, in a full sheet: what the preview cut, fetched from the server when it is longer than the preview.
  function openStepViewer(e, v, ic, inLabel) {
    if (e.more?.length && e.slot != null && !e.full) { const open = (f) => openStepViewer({ ...e, ...f, full: true }, v, ic, inLabel); fullOf(e).then(open, () => open({ fullFailed: true })); return; }
    const status = e.live ? "Running · " + e.secs : e.unfinished ? "No result" : e.ok ? "Done · " + e.secs : e.ok === null ? "Exit unknown · " + e.secs : "Failed · " + e.secs;
    const { body, show: open } = panel(v + " " + e.arg, { cls: "full", sub: e.name + " · " + status, label: v + " " + e.arg });
    body.classList.add("viewer-b");
    renderFullTool(body, e, inLabel, SESS[e.sid]?.state === "wait");
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
  function attachmentSnapshot(e) {
    const images = (e.img ?? []).map((a, i) => {
      const k = a.w > 0 && a.h > 0 ? Math.min(1, THUMB_W / a.w, THUMB_H / a.h) : null;
      return { unavailable: !!a.na, url: "/api/attachment?sid=" + enc(e.sid) + "&o=" + enc(a.o) + "&b=" + enc(a.b) + "&v=" + enc(a.v),
        label: "Attached image " + (i + 1) + " (" + (IMAGE_KIND[a.type] ?? "image") + ", " + sizeText(a.size) + ")",
        width: k ? Math.max(1, Math.round(a.w * k)) : undefined, height: k ? Math.max(1, Math.round(a.h * k)) : undefined };
    });
    return images;
  }
  function openImage(url, label, from) {
    const image = createImageViewer(url, label, {
      opened(d) { viewerEl = d; document.documentElement.classList.add("viewer-open"); try { history.pushState({ ...navigation.route, sheet: 1 }, ""); } catch {} },
      closed(d) {
        document.documentElement.classList.remove("viewer-open");
        if (viewerEl === d) { viewerEl = null; if (history.state?.sheet) { skipPop = true; history.back(); } }
        if (LIVE.pending) refresh();
        (from.isConnected ? from : [...document.querySelectorAll("button.attach")].find(x => x.querySelector("img")?.getAttribute("src") === url))?.focus();
      },
    });
    image.show();
  }

  function openScript(e) {
    const done = (fields) => openStepViewer({ ...e, ...fields, full: true }, "View script", "run", "Script");
    api("/api/entry?sid=" + enc(e.sid) + "&slot=" + e.slot + "&as=script")
      .then((result) => done({ scriptText: result.text, scriptTruncated: result.truncated }))
      .catch(() => done({ scriptFailed: true }));
  }

  // A brief or message clamped to three lines; "Show more" opens it in place, and only appears when it is cut.
  function childSnapshot(h, c) {
    const calls = countOf(c, "calls"), holder = HOLDS.get(h.id);
    return { kind: "child", handoff: h.id, id: c.id, turn: STARTS.get(h.id)?.id, name: c.name, state: c.state, stateLabel: STATE[c.state] ?? c.state,
      meta: [kindText(c), shortModel(c.model), dur(c.start, c.state === "work" ? null : c.last), (calls ?? "—") + (calls === 1 ? " step" : " steps"), costText(costForSession(c.id, true))].join(" · "), mark: harnessSnapshot(c.harness), brief: preview(h.brief), result: h.result || undefined, failed: h.status === "err", activity: !h.result && c.state === "work" && c.activity ? [verbNow(c.activity[0]), c.activity[1]] : undefined, trace: holder?.id, traceLabel: holder ? "Open run view for " + nameOf(h.from) : undefined, chevron: I.chev };
  }
  // The tips on a session footer's items, as plain text (the tooltip sets it with textContent): calls by tool, times, cost by kind.
  const callsTip = (s) => { const parts = Object.entries(s.tool_calls ?? {}).sort((a, b) => b[1] - a[1] || (a[0] < b[0] ? -1 : 1)).map(([name, n]) => name + " " + n), failed = countOf(s, "errors"); if (failed) parts.push(failed + " failed"); return parts.join(" · "); };
  const timeTip = (s, h, finished) => "Started " + clock(s.start) + " · last activity " + clock(s.last) + (finished ? " · finished " + clock(h?.done ?? s.last) : "");
  function costTip(cost) {
    const groups = Object.entries(cost.by_model).map(([id, m]) => { const k = m.usd_by_kind ?? {}, kinds = [["Input", k.input], ["Output", k.output], ["Cache read", k.cache_read], ["Cache write", (Number(k.cache_write_5m) || 0) + (Number(k.cache_write_1h) || 0)]]; if (Number(k.web_search) >= 0.005) kinds.push(["Web search", k.web_search]); return { id, text: kinds.map(([label, usd]) => label + " " + asMoney(Number(usd) || 0)).join(" · ") }; });
    return (groups.length > 1 ? groups.map((g) => g.id + ": " + g.text).join("; ") : groups.map((g) => g.text).join("")) + (groups.length ? ". " : "") + COST_TIP;
  }
  // What a footer shows: its text, its tips, its state, and whether it has the button to the parent.
  // The line a session page ends in, for a child (h: its origin) and any other session alike: the state, the calls, the time and
  // the API-equivalent cost, each with its breakdown as a tip. A returned child keeps "Returned to <parent>" and its button.
  function footerSnapshot(s, h) {
    const done = s.state === "done" || s.state === "err", finished = h ? done || h.status === "done" || h.status === "err" : done;
    const state = h ? finished ? s.state === "err" || h.status === "err" ? "err" : "done" : "work" : s.state;
    const cost = costForSession(s.id), priced = cost.usd != null && !costMissing(cost).length && cost.usd >= 0.005, items = [];
    if (!h || !finished) items.push({ kind: "calls", text: callsText(countOf(s, "calls")), tip: callsTip(s) });
    items.push({ kind: "time", text: dur(s.start, state === "work" ? null : s.last), tip: timeTip(s, h, finished) });
    if (priced) items.push({ kind: "cost", text: asMoney(cost.usd), tip: costTip(cost) });
    return { state, stateLabel: STATE[state] ?? state, text: h && finished ? "Returned to " + nameOf(h.from) + " · " + STATE[state] : STATE[state], items, parent: h && finished ? { id: h.from, turn: HOLDS.get(h.id)?.id, name: nameOf(h.from) } : undefined };
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
    if (!s || s.route !== navigation.route || s.box !== box || !box.contains(s.el)) { s?.ctx.destroy?.(); s = { route: navigation.route, box, ctx: {}, el: null }; s.el = build(s.ctx); SLOTS.set(key, s); }
    return s;
  }
  // Empties `box` of everything but the slots the route being drawn already holds in it.
  function clearBox(box, r) {
    for (const [key, s] of SLOTS) if (s.route !== r) { s.ctx.destroy?.(); SLOTS.delete(key); }
    if (ownsScreen(box)) { if (screenKind(box) === r.v && !(r.v === "machines" && viewerHost) && !NATIVE_PAGE) return; releaseScreen(box); }
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

    const focusSearch = focusSessionsSearchOnRender === navigation.route; focusSessionsSearchOnRender = null;
    if (SIDEBAR_ONLY) { domain.invalidate(); tick(); navigation.rendered = navigation.route; renderNav(); renderLanes(); return; } // the embedding page draws its own page and bar
    if (NATIVE_PAGE || (navigation.route.v === "machines" && viewerHost)) {
      tick(); navigation.rendered = navigation.route;
      if (navigation.content && !navigation.content.element.isConnected) { clearBox($("#page"), navigation.route); $("#page").append(navigation.content.element); }
      document.title = (NATIVE_PAGE?.title ?? "Machines") + " · Semon";
      renderTopbar(NATIVE_PAGE?.title ?? "Machines"); syncLayoutPrefs(); syncBarLine(); renderNav(); renderLanes(); renderDrawerAccount(); syncJump(); return;
    }
    // The page first, then the bar: the bar's summary (a trace's counts, a search's matches) comes from the page.
    if (viewerHost) document.title = ({ home: "Home", sessions: "Sessions", analytics: "Analytics" }[navigation.route.v] ?? "Semon") + " · Semon";
    resetPagerInput(); holdProgrammaticScroll(); closeAccountMenu(); stopOpeningEndPin(); domain.invalidate(); // a redraw inside the open-at-end window ends the pin
    ordPageState = ordState("page"); tick(); const page = $("#page"), r = navigation.route; navigation.rendered = r; setGeometry(page, "paddingBottom", null); clearBox(page, r); page.classList.remove("child-page");
    if (r.v === "home") { renderHome(page); renderTopbar("Home"); }
    else if (r.v === "analytics") { renderAnalytics(page); renderTopbar("Analytics", null, { analytics: true }); }
    else if (r.v === "sessions") { renderSessions(page, focusSearch); renderTopbar("Sessions"); }
    else if (r.v === "machines") { renderMachines(page); renderTopbar("Machines"); }
    else if (r.v === "machine") { renderMachine(page, r.id); renderTopbar(MACHINE[r.id], { label: "Machines", go: () => go({ v: "machines" }) }, { line2: machineLine(r.id) }); }
    else if (r.v === "trace") { renderTrace(page, r.turn); renderTopbar("Trace", { label: SESS[r.sid].name, go: () => goSession(r.sid, r.turn) }, { traceSession: SESS[r.sid] }); }
    else if (r.v === "session") { const s = SESS[r.id], lineage = lineageOf(r.id).slice(0, -1); renderSession(page, r.id); renderTopbar(s.name, null, { session: s, lineage, line2: sessionLine(s) }); }
    if (r.v === "session" && errOn(r.id)) markError(false);
    setGeometry(document.documentElement, "barHeight", $("#topbar").offsetHeight);
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
    if (navigation.route.v !== "analytics" || LIVE.ended || !visible()) return;
    const data = analyticsData(), behind = !AN.error && data && LIVE.version && data.version !== LIVE.version;
    AN.timer = setTimeout(() => { AN.timer = null; refreshAnalytics(); }, AN.error ? Math.max(0, AN.failedAt + AN_EVERY - performance.now()) : behind ? 1200 : AN_EVERY);
  }
  // `asked`: the reader changed the range or a filter, which asks at once. Anything else (a model update, the tab showing
  // again) waits out the backoff while asking fails, so failed asks keep 10 s apart however fast the model moves.
  function refreshAnalytics(asked = false) {
    if (navigation.route.v !== "analytics") return Promise.resolve();
    if (asked !== true && backingOff()) { if (!AN.timer) scheduleAnalytics(); return Promise.resolve(); }
    clearTimeout(AN.timer); AN.timer = null;
    return fetchAnalytics(asked === true).then((changed) => {
      if (changed && navigation.route.v === "analytics" && navigation.rendered === navigation.route) {
        if (viewerEl || accountChrome.open) LIVE.pending = true; // drawn when the sheet or the account menu closes
        else { const st = capture(); render(); restore(st); }
      }
      scheduleAnalytics();
    });
  }
  if (!SIDEBAR_ONLY) document.addEventListener("visibilitychange", () => { if (visible() && navigation.route.v === "analytics") refreshAnalytics(); else if (!visible()) { clearTimeout(AN.timer); AN.timer = null; } });
  const nameOfSid = (A, sid) => SESS[sid]?.name ?? A.sessions[sid]?.name ?? sid;
  const harnessOfSid = (A, sid) => SESS[sid]?.harness ?? A.sessions[sid]?.harness ?? "";
  const sessionFacetValue = (s, key) => key === "repo" ? s.repo ?? "__none__" : key === "model" ? s.model ?? s.modelId ?? "Unknown model" : s[key] ?? "";
  function matchesSessionFacets(s) { return Object.keys(sessionFilters).every((key) => !sessionFilters[key] || sessionFacetValue(s, key) === sessionFilters[key]); }
  // The four filters (Repo, Machine, Harness, Model) of Analytics and Sessions: one persistent control per page. It is one
  // "Filter" button (with the count of active filters) and one chip per active filter, whose × clears it. The button opens a
  // sheet (a bottom sheet on a phone, a dialog on a wide screen) holding the four Selects, with "Clear all" and "Done". The
  // choices apply when the sheet closes, however it closes. A redraw (`sync`) brings the Selects' option lists, the chips and the
  // count up to date in place. A selected value that no session has now stays selected, marked "(no sessions)", until the reader
  // changes it. The sheet lives inside the control, so its Selects are in the page even while it is shut.
  // On Analytics the range's own values join the model's: a repo that worked last week is a choice there.
  const rangeFacet = (key) => navigation.route.v !== "analytics" ? [] : (analyticsData()?.facets?.[key] ?? []).map((v) => v ?? "__none__");
  const FACETS = [
    ["repo", "Repo", "All repos", () => [...new Set([...Object.values(SESS).map((s) => sessionFacetValue(s, "repo")), ...rangeFacet("repo")])].sort((a, b) => a === "__none__" ? 1 : b === "__none__" ? -1 : a.localeCompare(b)), (v) => v === "__none__" ? "No repo" : v],
    ["machine", "Machine", "All machines", () => [...new Set([...Object.values(SESS).map((s) => s.machine ?? ""), ...rangeFacet("machine")])].sort(), (v) => MACHINE[v] ?? v],
    ["harness", "Harness", "All harnesses", () => [...new Set([...Object.values(SESS).map((s) => s.harness ?? ""), ...rangeFacet("harness")])].sort(), (v) => HARNESS[v] ?? v],
    ["model", "Model", "All models", () => [...new Set([...Object.values(SESS).map((s) => sessionFacetValue(s, "model")), ...rangeFacet("model")])].sort(), shortModel],
  ];
  function renderFacetFilters(box, onChange) {
    const s = slot("facets", box, (ctx) => {
      let before = "";
      const control = createFacetChrome({
        select(label, value, onChange) { return createSelect({ label, value, options: [], onChange }); },
        change(key, value) { sessionFilters[key] = value; },
        cleared(key) { sessionFilters[key] = ""; ctx.sync(); ctx.onChange(); },
        canOpen() { return !viewerEl; },
        opened(d) { ctx.sync(); before = JSON.stringify(sessionFilters); viewerEl = d; try { history.pushState({ ...navigation.route, sheet: 1, scrollTop: currentScroll() }, ""); } catch {} },
        closed(d, reason) { if (viewerEl === d) { viewerEl = null; if (history.state?.sheet) { skipPop = true; history.back(); } } if (reason === "destroyed") return; if (JSON.stringify(sessionFilters) !== before) { LIVE.pending = false; ctx.onChange(); } else if (LIVE.pending) refresh(); },
        clear() { for (const key of Object.keys(sessionFilters)) sessionFilters[key] = ""; },
      });
      ctx.destroy = () => control.destroy();
      ctx.sync = () => control.update(FACETS.map(([key, label, allLabel, valuesOf, showValue]) => {
        const current = sessionFilters[key], values = valuesOf().filter((value) => value !== ""), gone = current !== "" && !values.includes(current);
        if (gone) values.push(current);
        return { key, label, value: current, display: showValue(current), options: [{ value: "", label: allLabel }, ...values.map((value) => ({ value, label: showValue(value) + (gone && value === current ? " (no sessions)" : "") }))] };
      }));
      return control.element;
    });
    s.ctx.onChange = onChange; s.ctx.sync(); return s.el;
  }
  const hoursText = (ms) => (ms / HOUR).toFixed(1) + " h", rangeName = () => analyticsRange === 1 ? "24 h" : analyticsRange + " d";
  function chartWidth() { const page = $("#page"), style = getComputedStyle(page); return Math.max(280, Math.round(page.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight))); }
  const rangeAgo = (A) => A.days === 1 ? "24 h ago" : A.days + " d ago";
  function openModelItems(group) {
    const { d, body, show: open } = panel(shortModel(group.model) + " · " + group.band, { label: "Work items for " + group.model + ", " + group.band });
    renderModelItems(body, group.n + " work items" + (group.small_sample ? " · small sample" : ""), (group.items ?? []).map(item => ({ id: item.sid, name: SESS[item.sid]?.name,
      description: (item.models ?? []).map(shortModel).join(" → ") + " · " + (item.cost_usd == null ? "cost unknown" : asMoney(item.cost_usd)), trace: (TURNS[item.sid] ?? []).find(t => t.out.length)?.id, url: liveUrl(item.pr_url) ?? undefined })), group.items_more ? group.items_more + " more items in the selected range" : undefined, {
      session(id) { pendingSessionOpen = id; d.close(); }, trace(id) { afterPop = () => goTrace(id); d.close(); },
    }); open();
  }
  // A chart column's sessions, as the server listed them (most first); `more` counts those it left out.
  function openAnalyticsSlice(A, a, b, items, more, costMode = false) {
    const when = new Date(a).toLocaleString([], { weekday: "short", hour: "numeric", minute: "2-digit" }) + "–" + new Date(b).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" }), heading = costMode ? "Sessions with cost" : "Sessions busy";
    const sheet = createNativeSheet({ className: "analytics-slice", heading: heading + " · " + when, label: heading + " " + when, closeLabel: "Close sessions list" }, {
      opened(d) { viewerEl = d; document.documentElement.classList.add("viewer-open"); try { history.pushState({ ...navigation.route, sheet: 1, scrollTop: currentScroll() }, ""); } catch {} },
      closed(d) { document.documentElement.classList.remove("viewer-open"); if (viewerEl === d) { viewerEl = null; if (history.state?.sheet) { skipPop = true; history.back(); } else if (pendingSessionOpen) { const id = pendingSessionOpen; pendingSessionOpen = null; goSession(id); } } },
    });
    const rows = items.map(item => { const harness = harnessOfSid(A, item.sid); return { id: item.sid, name: nameOfSid(A, item.sid), harness, harnessName: HARNESS_SHORT[harness] ?? harness, harnessTip: HARNESS[harness] && HARNESS[harness] !== HARNESS_SHORT[harness] ? HARNESS[harness] : undefined, mark: harnessSnapshot(harness), value: costMode ? asMoney(item.usd) : timeText(item.ms) + " busy", href: SESS[item.sid] ? urlOf({ v: "session", id: item.sid }) : undefined, className: "analytics-session analytics-slice", missing: costMode && item.unpriced_models.length ? "no price for " + item.unpriced_models.join(", ") : undefined }; });
    renderSliceBody(sheet.body, rows, more ?? 0, costMode ? "No sessions had a recorded cost then." : "No sessions were busy then.", id => { pendingSessionOpen = id; sheet.dialog.close(); });
    sheet.show();
  }
  function renderAnalytics(page) {
    const A = analyticsData(), metrics = [], charts = [], breakdowns = [], lists = [], modelGroups = new Map(), slices = new Map();
    const heading = "Measured activity · Last " + (analyticsRange === 1 ? "24 hours" : analyticsRange + " days");
    let error = AN.error ? A ? "Couldn't update Analytics: " + AN.error + ". Showing the last answer." : "Couldn't load Analytics: " + AN.error : A ? undefined : "Loading…";
    const note = (value, previous, format) => { const delta = value - previous; return Math.abs(delta) < 1e-9 ? { text: "No change vs previous " + rangeName() } : { lead: (delta > 0 ? "+" : "−") + format(Math.abs(delta)), tone: delta > 0 ? "up" : "down", text: " vs previous " + rangeName() }; };
    const metric = (label, value, delta, explanation, info) => metrics.push({ id: "metric-more-analytics-" + metrics.length, label, value, note: delta, explanation, info });
    const widthOf = (measure, max) => Math.max(measure ? 2 : 0, measure / max * 100);
    const makeChart = (costMode) => {
      const key = costMode ? "cost" : "agents", W = chartWidth(), left = costMode ? 46 : 40, right = W - 4;
      if (costMode && !A.cost) return { key, heading: "Cost over time", info: COST_TIP, empty: "Cost is recorded per UTC day, so there is no hourly series. Pick 7 d or 30 d for a daily chart.", width: W, left, right, grid: [], bins: [], legend: [], role: "img", label: "", ago: rangeAgo(A) };
      const raw = costMode ? A.cost.days.map(d => ({ a: d.from, b: d.to, claude: d.claude_usd, codex: d.codex_usd, sessions: d.sessions, more: d.more })) : A.agents.columns.map(c => ({ a: c.from, b: c.to, claude: c.claude_ms / HOUR, codex: c.codex_ms / HOUR, sessions: c.sessions, more: c.more }));
      const most = Math.max(0, ...raw.map(b => b.claude + b.codex)), stepY = niceStep(most || 1), max = costMode ? Math.max(.01, most) : Math.max(stepY, Math.ceil(most / stepY) * stepY);
      const grid = []; if (costMode) for (let i = 0; i <= 2; i++) grid.push({ y: 151 - 139 * i / 2, label: "$" + (max * i / 2).toFixed(2) }); else for (let n = 0; n <= max + 1e-9; n += stepY) grid.push({ y: 151 - 139 * n / max, label: hLabel(n) });
      const step = (right - left) / Math.max(1, raw.length), barWidth = Math.max(2, step * .64);
      const bins = raw.map((bin, i) => { const id = key + ":" + i, total = bin.claude + bin.codex, when = clock(bin.a) + "–" + clock(bin.b), tip = when + ": " + hLabel(total); slices.set(id, { bin, costMode });
        return { key: id, x: left + i * step, width: step, barX: left + i * step + (step - barWidth) / 2, barWidth, claude: 139 * bin.claude / max, codex: 139 * bin.codex / max,
          active: costMode ? !!bin.sessions.length : total > 0, tip: costMode ? undefined : tip, label: costMode ? clock(bin.a) + " to " + clock(bin.b) + ": " + asMoney(total) : tip + ". Open the sessions busy then", when, value: costMode ? asMoney(total) : hLabel(total) + " agent-hours" }; });
      return { key, heading: costMode ? "Cost over time" : "Agents at work", info: costMode ? COST_TIP : undefined, sub: costMode ? "API-equivalent cost per UTC day · today so far · stacked by harness" : "Agent-hours " + A.agents.unit + " · stacked by harness", width: W, left, right, grid, bins,
        label: costMode ? "API-equivalent cost per day, stacked by harness" : "Agent-hours " + A.agents.unit + " over the selected range, stacked by harness", role: costMode ? "img" : "group", ago: rangeAgo(A),
        legend: [["claude", "Claude"], ["codex", "Codex"]].map(([id, label]) => ({ id, label, mark: harnessSnapshot(id) })), missing: costMode && A.cost.unpriced_models.length ? "no price for " + A.cost.unpriced_models.join(", ") + "; unpriced usage is omitted from bars." : undefined };
    };
    let models, allowance;
    const measures = [["Acceptance", "first_pass_acceptance", "acceptance_n", "acceptance", n => (n * 100).toFixed(0) + "%"], ["Review rounds", "median_review_rounds", "review_rounds_n", "review_rounds", String], ["Red CI heads", "median_red_ci_heads", "red_ci_n", "ci", String], ["Model time", "median_model_ms", "model_time_n", "model_time", timeText], ["API cost", "median_cost_usd", "cost_n", "cost", asMoney], ["Allowance / M input", "allowance_per_million_input", "allowance_n", "allowance", n => n.toFixed(2) + "%"]];
    if (A) {
      const now = A.current, previous = A.previous, pct = (errors, tools) => tools ? Math.round(errors / tools * 100) + "%" : "0%";
      metric("Agent-hours", hoursText(now.agent_ms), note(now.agent_ms, previous.agent_ms, hoursText), "Busy time summed across sessions; two sessions busy for an hour count two hours.");
      metric(A.days === 1 ? "Cost today (UTC)" : "Cost, last " + A.days + " UTC days", now.cost.usd == null ? "—" : asMoney(now.cost.usd), now.cost.usd == null || previous.cost.usd == null ? { text: "no price for " + [...new Set([...now.cost.unpriced_models, ...previous.cost.unpriced_models])].join(", ") } : note(now.cost.usd, previous.cost.usd, asMoney), A.days === 1 ? "API-equivalent cost. Cost is recorded per UTC day: this is the whole current UTC day so far, compared with the whole day before." : "API-equivalent cost. Cost is recorded per UTC day: the last " + A.days + " UTC days count, today so far, compared with the " + A.days + " whole UTC days before.", COST_TIP);
      metric("Sessions started", countText(now.started), note(now.started, previous.started, countText)); metric("Turns", countText(now.turns), note(now.turns, previous.turns, countText));
      const toolNote = note(now.tools, previous.tools, countText); if (A.calls_unknown) toolNote.tail = " · — for " + A.calls_unknown + (A.calls_unknown === 1 ? " session" : " sessions");
      metric("Tool calls", countText(now.tools), toolNote, countText(now.errors) + " failed (" + pct(now.errors, now.tools) + ") · previous " + rangeName() + ": " + countText(previous.errors) + " failed (" + pct(previous.errors, previous.tools) + ")");
      metric("Peak concurrency", countText(now.peak), note(now.peak, previous.peak, countText), "The most sessions busy at the same moment.");
      metric("Waited on you", timeText(now.wait_ms), note(now.wait_ms, previous.wait_ms, timeText), "Median wait " + timeText(now.median_wait_ms) + " · previous " + rangeName() + ": " + timeText(previous.median_wait_ms));
      const wait = A.longest_current_wait; metric("Longest current wait", wait ? timeText(wait.ms) : "—", note(wait ? wait.ms : 0, previous.longest_wait_ms, timeText), wait ? nameOfSid(A, wait.sid) + " has waited on you for " + timeText(wait.ms) : "No session is waiting on you");
      charts.push(makeChart(false), makeChart(true));
      for (const [key, title, groups] of [["repo", "By repo", A.breakdown.repo], ["machine", "By machine", A.breakdown.machine], ["harness", "By harness and model", A.breakdown.model]]) {
        const keyFor = g => key === "repo" ? g.repo ?? "__none__" : key === "machine" ? g.machine : g.harness + "\u0000" + g.model;
        const labelFor = id => key === "repo" ? id === "__none__" ? "No repo (roles)" : id : key === "machine" ? MACHINE[id] ?? id : (HARNESS[id.split("\u0000")[0]] ?? id.split("\u0000")[0]) + " · " + shortModel(id.split("\u0000")[1]);
        const selected = g => analyticsMeasure === "cost" ? g.usd : g.ms, rows = [...groups].sort((a, b) => selected(b) - selected(a) || labelFor(keyFor(a)).localeCompare(labelFor(keyFor(b)))), max = Math.max(1, ...rows.map(selected));
        breakdowns.push({ key, heading: title, rows: rows.map(g => ({ key: keyFor(g), name: labelFor(keyFor(g)), count: g.sessions + (g.sessions === 1 ? " session" : " sessions"), width: widthOf(selected(g), max), color: key === "harness" ? keyFor(g).startsWith("claude") ? "claude" : "codex" : undefined, hours: hoursText(g.ms), cost: g.unpriced_models.length ? "—" : asMoney(g.usd), missing: g.unpriced_models.length ? "no price for " + g.unpriced_models.join(", ") : undefined })) });
      }
      for (const [title, items, value, measure] of [["Top sessions · busy time", A.top.busy, x => timeText(x.ms), x => x.ms], ["Top sessions · waited on", A.top.waited, x => timeText(x.ms), x => x.ms], ["Most expensive sessions · API-equivalent cost", A.top.cost, x => x.usd == null ? "—" : asMoney(x.usd), x => x.usd ?? 0]]) {
        const max = Math.max(1, ...items.map(measure)); lists.push({ heading: title, rows: items.map(item => { const harness = harnessOfSid(A, item.sid), missing = item.unpriced_models ?? []; return { id: item.sid, name: nameOfSid(A, item.sid), harness, harnessName: HARNESS_SHORT[harness] ?? harness, harnessTip: HARNESS[harness] && HARNESS[harness] !== HARNESS_SHORT[harness] ? HARNESS[harness] : undefined, mark: harnessSnapshot(harness), value: value(item), href: SESS[item.sid] ? urlOf({ v: "session", id: item.sid }) : undefined, rank: widthOf(measure(item), max), missing: missing.length ? "no price for " + missing.join(", ") : undefined }; }) });
      }
      if (A.models) {
        const reasons = A.models.unknown_reasons ?? {}; models = [];
        for (const band of ["easy", "medium", "hard", "unknown"]) {
          const rows = (A.models.groups ?? []).filter(g => g.band === band); if (!rows.length) continue;
          const points = rows.filter(g => g.first_pass_acceptance != null && g.median_cost_usd != null), W = Math.max(280, chartWidth()), maxCost = Math.max(.01, ...points.map(g => g.median_cost_usd));
          models.push({ key: band, heading: band === "unknown" ? "Unknown difficulty" : band[0].toUpperCase() + band.slice(1), width: W,
            rows: rows.map(g => { const key = band + "\u0000" + g.model; modelGroups.set(key, g); const tokens = g.tokens ?? {}; return { key, name: shortModel(g.model), tip: g.model, count: g.n + (g.small_sample ? " · small sample" : ""), cells: [...measures.map(([, valueKey, nKey, reason, format]) => ({ text: g[valueKey] == null ? "Unknown" : format(g[valueKey]), n: g[nKey] ?? 0, tip: g[valueKey] == null ? reasons[reason] : undefined })), { text: g.tokens_n ? [tokens.input ?? 0, tokens.output ?? 0, (tokens.cache_read ?? 0) + (tokens.cache_write ?? 0)].map(countText).join(" / ") : "Unknown", n: g.tokens_n ?? 0 }] }; }),
            points: points.map(g => ({ x: 42 + g.median_cost_usd / maxCost * (W - 58), y: 150 - g.first_pass_acceptance * 130, label: shortModel(g.model), tip: g.model + ": " + asMoney(g.median_cost_usd) + ", " + (g.first_pass_acceptance * 100).toFixed(0) + "% accepted; acceptance n=" + g.acceptance_n + ", cost n=" + g.cost_n })) });
        }
      }
      const limits = A.allowance;
      if (limits?.recorded_at != null && limits.windows?.length) allowance = { when: new Date(limits.recorded_at).toLocaleString([], { hour: "numeric", minute: "2-digit" }), windows: limits.windows.map(limit => ({ label: limit.minutes === 300 ? "5-hour window" : limit.minutes === 10080 ? "Weekly window" : limit.minutes + "-minute window", used: limit.used_percent + "% used", reset: "Resets " + new Date(limit.resets_at).toLocaleString([], { weekday: "short", hour: "numeric", minute: "2-digit" }) })) };
    }
    renderAnalyticsScreen(page, { heading, error, ready: !!A, query: analyticsQuery(), metrics, charts, breakdowns, lists, models, measure: analyticsMeasure, allowance, modelHeaders: ["Model", "Work items", ...measures.map(m => m[0]), "Tokens · input / output / cache"] }, {
      committed: observeTitle, facets: () => renderFacetFilters(page, () => { render(); refreshAnalytics(true); }), session: goSession,
      measure(value) { if (analyticsMeasure === value) return; const top = currentScroll(); analyticsMeasure = value; render(); restoreScroll(top); },
      breakdown(group, key) { if (group === "repo") sessionFilters.repo = key; else if (group === "machine") sessionFilters.machine = key; else { const [harness, model] = key.split("\u0000"); sessionFilters.harness = harness; sessionFilters.model = model; } query = ""; groupBy = "recent"; go({ v: "sessions" }); },
      slice(key) { const { bin, costMode } = slices.get(key); openAnalyticsSlice(A, bin.a, bin.b, bin.sessions, bin.more, costMode); }, model(key) { openModelItems(modelGroups.get(key)); },
    });
  }

  // ---- Sessions: every top-level session and its child runs ---------------------------------------------------------------
  const laneOf = (sid) => { const seen = new Set(); while (parentOf(sid) && !seen.has(sid)) { seen.add(sid); sid = parentOf(sid); } return sid; };
  function childRuns(sid) {
    const kids = Object.values(SESS).filter((x) => x.id !== sid && laneOf(x.id) === sid && (showApprovalReviews || !isApprovalReview(x))), sub = kids.filter((x) => x.kind === "Subagent").length, cdx = kids.filter((x) => x.kind === "Codex run").length, other = kids.length - sub - cdx;
    return [sub ? sub + (sub === 1 ? " subagent" : " subagents") : null, cdx ? cdx + (cdx === 1 ? " Codex run" : " Codex runs") : null, other ? other + (other === 1 ? " other run" : " other runs") : null].filter(Boolean).join(" · ");
  }
  function renderSessions(page, focusSearch = false) {
    const all = Object.values(SESS).filter(s => (showApprovalReviews || !isApprovalReview(s)) && matchesSessionFacets(s));
    const lanes = all.filter(s => sessMatch(s, query)), order = orderScope("page", pageSig(), navigation.route, pageState());
    let groups;
    if (groupBy === "recent") groups = [["", orderList(order, "recent", lanes, byLast), lanes.length]];
    else { const key = { machine: s => MACHINE[s.machine], project: s => s.repo ?? "No repo (roles)", harness: s => HARNESS[s.harness] }[groupBy];
      const keys = [...new Set(lanes.map(key))].sort((a, b) => a.startsWith("No repo") - b.startsWith("No repo") || a.localeCompare(b));
      groups = keys.map(k => { const rows = lanes.filter(s => key(s) === k); return [k, orderList(order, "g:" + k, rows, byLast), rows.length]; }); }
    const rows = groups.filter(([, items, total]) => items.length || !total).map(([title, items, total]) => ({ title, total,
      harness: groupBy === "harness" ? harnessSnapshot(Object.keys(HARNESS).find(k => HARNESS[k] === title)) : undefined,
      rows: items.map(s => {
        const fields = [{ className: "row-duration", text: dur(s.start, s.state === "work" || s.state === "wait" ? null : s.last), priority: 1, tip: "Duration", icon: I.duration }];
        if (Object.keys(MACHINE).length > 1) fields.push({ className: "row-machine host", text: shortHost(s), priority: 2, tip: "Machine: " + hostOf(s), icon: I.machine });
        if (s.repo) fields.push({ className: "repo-short", text: s.repo, priority: 3, tip: "Repo: " + s.repo, icon: I.repo });
        const counts = childRuns(s.id); if (counts) fields.push({ className: "row-counts", text: counts.replace(/Codex runs/g, "runs"), priority: 4 });
        return { id: s.id, name: s.name, state: s.state, stateLabel: STATE[s.state] ?? s.state, age: ago(s.last), harness: harnessSnapshot(s.harness),
          model: shortModel(s.model ?? s.modelId), modelTip: "Model: " + modelIdOf(s), delegation: parentOf(s.id) ? I.spawn ?? I.stack : undefined, fields };
      }) }));
    renderSessionsScreen(page, { total: all.length, working: all.filter(s => s.state === "work").length, waiting: all.filter(s => s.state === "wait").length,
      query, groupBy, reviews: showApprovalReviews, showReviews: Object.values(SESS).some(isApprovalReview) || showApprovalReviews, groups: rows, matches: lanes.length }, {
      session: goSession, committed: observeTitle, facets: () => renderFacetFilters(page, () => render()),
      search(value) { query = value.trim(); const address = new URL(location.href); if (query) address.searchParams.set("q", query); else address.searchParams.delete("q"); history.replaceState(history.state, "", address); renderSessions(page); renderLanes(); },
      group(value) { groupBy = value; renderSessions(page); renderLanes(); },
      reviews() { showApprovalReviews = !showApprovalReviews; ORD.delete("page"); ORD.delete("side"); render(); },
    }, focusSearch);
    renderLanes();
  }

  // ---- Drawer (phone) ---------------------------------------------------------------------------------------------------------
  const sidebar = $("#sidebar");
  function openDrawer() { shellChrome?.openDrawer(); }
  function closeDrawer(quiet) { shellChrome?.closeDrawer(quiet); }
  // Sidebar-only consumers retain their existing server shell.js owner.
  if (SIDEBAR_ONLY) window.addEventListener("semon:drawer-open", () => orderApply("side"));
    if (!SIDEBAR_ONLY) document.addEventListener("keydown", (e) => { if (e.key === "Escape" && accountSheet) accountChrome.escape(); else if (e.key === "Escape" && !viewerEl) { closeDrawer(); closeAccountMenu(); $(".session-menu")?.remove(); $("#more-btn")?.setAttribute("aria-expanded", "false"); } if (e.key === "/" && !/INPUT|TEXTAREA/.test(document.activeElement?.tagName ?? "") && !document.activeElement?.isContentEditable && !viewerEl) { e.preventDefault(); if (navigation.route.v === "session") { findOpen = true; render(); $("#find")?.focus(); } else if (navigation.route.v === "sessions") { $("#sq")?.focus(); } else { const r = { v: "sessions", q: query }; focusSessionsSearchOnRender = r; go(r); } } });
  phone.addEventListener("change", () => {
    closeDrawer(true); syncLayoutPrefs(); expandedAll = null; renderLanes();
    if (!phone.matches && viewerEl?.classList.contains("kids-sheet")) viewerEl.close(); // a sheet is a phone's: a wide screen opens the list in the tree
    if (navigation.route.v === "session" || navigation.route.v === "analytics") { const top = currentScroll(); render(); restoreScroll(top); }
    else renderLanes();
    const l2 = $("#topbar .meta-line"); if (l2 && navigation.route.v === "session") measureViewerBar($("#topbar")); syncJump();
  });

  // ---- Live updates (deliberate difference 3) --------------------------------------------------------------------------------
  // Every screen polls /api/model?since= every 2 s while the tab is visible, one request at a time: it backs off up to 30 s
  // on errors and stops on 403 (the server restarted with a new token). A new model swaps the globals and draws the screen
  // again with its view state kept: the scroll position, anchored to the first visible block; what is open, by stable keys;
  // focus, find and filters; the drawer. A session page follows its transcript's tail
  // with /api/tx?after= and replaces only the turns that changed. An open View all sheet holds the redraw until it closes.
  const liveController = createLiveController({
    async poll() {
      const response = await requestJson("/api/model?delta=1&since=" + enc(LIVE.late ? "" : LIVE.version ?? ""), undefined, true);
      if (!response) return;
      let model; try { model = applyModelDelta(response); } catch { await api("/api/model?delta=1").then(update); return; }
      await update(model);
    },
    failed(error) { return !!viewerHost?.modelFailed?.(error?.status ?? 0); }, ended,
  });
  const LIVE = liveController.state;
  // The turn index as last drawn, to tell which turns an update changed.
  const remember = (m) => { LIVE.turns = new Map(m.turns.map((x) => [x.id, turnKey(x)])); };
  // What an update can change in a turn record or a handoff, cheaply (not the text, which a record never rewrites).
  const turnKey = (x) => [x.start, x.end?.st, x.end?.why, x.end?.h, x.sent.join(","), x.last ? 1 : 0].join("|");
  const handKey = (h) => [h.status, h.to, h.done, h.result?.length, h.answer?.length, h.answers?.length, h.declined ? 1 : 0].join("|");
  // rendered route is owned by navigation; // the route the page shows
  const visible = () => document.visibilityState === "visible";
  const schedule = ms => liveController.schedule(ms);
  // An embedding page can cancel `semon:ended` to draw its own note in place of this one.
  function ended(status) {
    liveController.stop(); if ($(".livenote, .livenote-side")) return;
    if (!window.dispatchEvent(new CustomEvent("semon:ended", { cancelable: true, detail: { status } }))) return;
    const n = createStatusNote(SIDEBAR_ONLY ? "Sessions stopped updating: reload the page" : "Session ended: reload with the printed URL", SIDEBAR_ONLY ? "ghead livenote-side" : "livenote");
    if (SIDEBAR_ONLY) $("#lanes").after(n); else document.body.append(n); // on an embedding page, under the list that stopped
  }
  // A 403 or a dropped connection fails the update (and backs off); anything else skips that one transcript.
  const soft = (p) => p.catch((e) => { if (e?.status === 403 || e?.status === 0) throw e; });
  // The transcript on screen: a session page's own. Any other loaded transcript is dropped from TX (the last few opened are kept in
  // TXCACHE, and brought up to date when opened again). A child run's card is drawn from the model, so its transcript is not loaded.
  const viewed = () => { const v = new Set(); if (navigation.route.v === "session") v.add(navigation.route.id); return v; };
  // What a child card shows, so an update knows which cards changed: the run's name, state, kind, model, steps and current call.
  const cardKeys = () => new Map(H.filter((h) => h.kind === "spawn" && SESS[h.to]).map((h) => { const c = SESS[h.to]; return [h.id, [c.name, c.state, c.kind, c.model, countOf(c, "calls"), c.activity?.join("|"), h.status, h.result].join("\u0001")]; }));
  const applyModelDelta = value => modelStore.apply(value);
  function update(m) {
    m = applyModelDelta(m);
    const oldH = new Map(H.map((h) => [h.id, handKey(h)])), oldT = LIVE.turns, oldCards = cardKeys(), names = new Map(Object.values(SESS).map((x) => [x.id, x.name]));
    const hadOrigins = new Set([...TXCACHE.keys()].filter((sid) => !!originHandoff(sid)));
    const hadOrigin = navigation.route.v === "session" && !!SESS[navigation.route.id] && !!originHandoff(navigation.route.id);
    adopt(m); remember(m);
    for (const sid of STALE_BRIEFS) if (!SESS[sid]) STALE_BRIEFS.delete(sid);
    for (const sid of [...TXCACHE.keys()]) if (!hadOrigins.has(sid) && originHandoff(sid)) TXCACHE.delete(sid);
    // A page that had no origin and now has one (its parent's spawn arrived) loads its transcript again: the first prompt it drew as
    // a message is the brief, which the intro now shows. A failed request is retried on the next poll, which backs off, up to
    // LATE_TRIES requests in all; after that the page stays as drawn (the brief shows twice until a reload) and polls as usual.
    if (LIVE.late !== navigation.route.id) { if (LIVE.late) TXCACHE.delete(LIVE.late); LIVE.late = null; }
    if (navigation.route.v === "session" && !hadOrigin && !!SESS[navigation.route.id] && !!originHandoff(navigation.route.id)) { LIVE.late = navigation.route.id; LIVE.lateTries = 0; STALE_BRIEFS.add(navigation.route.id); TXCACHE.delete(navigation.route.id); }
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
    return chain.then(() => { LIVE.version = m.version; refresh(full ? null : dirtyTurns(cuts, grown, changedH, oldT, changedCards, patched)); if (navigation.route.v === "analytics") refreshAnalytics(); const e = errorsLive(); return e && soft(e); });
  }
  // The turns of the session page an update changed: those holding entries its tail brought (from the cut on), or a background
  // call it updated, those whose record or handoffs changed, and those holding the spawn of a child run that grew. Null: draw
  // them all.
  function dirtyTurns(cuts, grown, changedH, oldT, changedCards, patched) {
    if (navigation.route.v !== "session" || !TX[navigation.route.id]) return null;
    const sid = navigation.route.id, dirty = new Set(), owner = new Map((TURNS[sid] ?? []).flatMap((t) => t.entries.map((e) => [e, t.id])));
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
    if (sid !== navigation.route.id) { dropTx(sid); return Promise.resolve({ cut: null, reload: true }); }
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
    LIVE.pending = false; const r = navigation.route;
    if (navigation.rendered !== r || (r.v === "session" && !SESS[r.id]) || (r.v === "trace" && !SESS[r.sid]) || (r.v === "machine" && !MACHINE[r.id])) return;
    const st = capture(); setGeometry($("#page"), "paddingBottom", null);
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
    if (navigation.route.v !== "session" || performance.now() >= openingEndUntil) { stopOpeningEndPin(); return; }
    const sc = scroller(); scrollProgrammatically(() => { sc.scrollTop = sc.scrollHeight; }); scrollController.anchor = null; syncJump(); saveHistoryScroll();
  }
  function startOpeningEndPin() {
    resetPagerInput();
    stopOpeningEndPin(); if (navigation.route.v !== "session" || location.hash) return;
    openingEndUntil = performance.now() + 2000;
    disconnectPagerObservers();
    const turns = $("#page section[aria-label='Transcript'] .turns");
    if (turns) { openingEndObserver = new ResizeObserver(pinOpeningEnd); openingEndObserver.observe(turns); }
    pinOpeningEnd(); openingEndTimer = setTimeout(stopOpeningEndPin, 2000);
  }
  const scrollController = createScrollTransactions({ page: () => $("#page"), main: () => $("#main"), phone: () => phone.matches, edge, rendered: () => navigation.rendered, sheet: () => !!viewerEl, revision: () => scrollRevision, programmatic: scrollProgrammatically, sync: syncBarLine, restoreDrawer: () => shellChrome?.restoreDrawer() });
  const { capture, restore, opener, stateKey, identOf } = scrollController;
  // A session page in place: its turns are drawn again and only those that changed (or are new) replace the ones shown, so
  // the rest keep their nodes and state. The bar's summary line, the title and the sidebar follow. Returns how many entries
  // are new.
  function patchSession(dirty) {
    resetPagerInput(); holdProgrammaticScroll();
    tick(); const s = SESS[navigation.route.id], box = $("#page .turns");
    const keys = () => new Set([...$("#page").querySelectorAll(".turns :is(.msg, .bubble, .step, .event, .child-card, .thought, .think-pending)[data-e]")].map((n) => n.dataset.e));
    const before = keys();
    // Only the changed turns are drawn again, unless the turns shown no longer match the index or nothing was shown.
    const whole = !dirty || box.querySelector(":scope > p.empty") || [...box.querySelectorAll(":scope > .turn")].some((b) => !TURN.has(b.dataset.turn));
    renderSession($("#page"), navigation.route.id, whole ? {} : { only: dirty });
    drawSessionBar();
    for (const pager of box.querySelectorAll("[data-pager-where]")) paintPager(pager);
    renderNav(); renderLanes(); ticker();
    let n = 0; for (const k of keys()) if (!before.has(k)) n++;
    queuePagerObservers();
    return n;
  }
  // Jump to the latest: centred at the transcript column's foot, sticky, with the count of what arrived while the reader was away.
  function scrollMetrics() {
    if (phone.matches) return { top: window.scrollY, height: document.documentElement.scrollHeight, viewport: window.innerHeight, gap: Math.max(0, document.documentElement.scrollHeight - window.innerHeight - window.scrollY) };
    const m = $("#main"); return { top: m.scrollTop, height: m.scrollHeight, viewport: m.clientHeight, gap: Math.max(0, m.scrollHeight - m.clientHeight - m.scrollTop) };
  }
  function scrollToEnd(behavior = "smooth") { scrollProgrammatically(() => { if (phone.matches) window.scrollTo({ top: document.documentElement.scrollHeight, behavior }); else { const m = $("#main"); m.scrollTo({ top: m.scrollHeight, behavior }); } }); }
  // The button is rebuilt only when what it shows changes (hidden or not, and the new-entry count), not on every scroll.
  let jumpBusy = false;
  function syncJump() {
    if (navigation.route.v !== "session") { LIVE.fresh = 0; return; }
    const { gap } = scrollMetrics(), newer = TXM[navigation.route.id]?.newer ?? 0; if (gap <= 80) LIVE.fresh = 0;
    updateSessionJump($("#page"), gap > 80 || !!newer, LIVE.fresh + newer, jumpBusy);
  }
  function clearNewEntries() { LIVE.fresh = 0; updateSessionJump($("#page"), false, 0, jumpBusy); }
  function jumpToLatest() {
    const sid = navigation.route.id, r = navigation.route, m = TXM[sid];
    if (m && m.to < m.total) {
      jumpBusy = true; syncJump(); fetchTx(sid, "").then(() => { if (navigation.route === r) goSession(sid); }).catch(() => {}).finally(() => { jumpBusy = false; syncJump(); });
    } else scrollToEnd("smooth");
  }
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
    if (navigation.route.v === "session" && navigation.rendered === navigation.route) updateSessionClock($("#page"), NOW, Object.fromEntries(Object.values(SESS).filter(s => s.activity?.[3] != null).map(s => [s.id, s.activity[3]])));
    else if (navigation.route.v === "home" && navigation.rendered === navigation.route) renderHome($("#page"));
  }

  // An embedding page's sidebar: the row its data-viewer-nav names (home, sessions or machines) is current.
  if (SIDEBAR_ONLY) { const nav = app.dataset.viewerNav; navigation.route = { v: ["home", "sessions", "machines"].includes(nav) ? nav : "" }; }
  if (viewerHost) {
    ACCOUNT = accountOf(viewerHost.account);
    if (NATIVE_PAGE) navigation.route = { v: NATIVE_PAGE.nav };
    else if (navigation.content) navigation.route = { v: "machines" };
    if (NATIVE_PAGE || navigation.content) render();
  }
  boot();
});
