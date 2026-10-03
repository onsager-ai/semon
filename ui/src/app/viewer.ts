import { createDomain } from "../domain/calculations";
import { ago as formatAgo,clock as formatClock,dur as formatDuration,machineShorts,preview,shortModel } from "../domain/format";
import type { Cost,Entry,Handoff,Session,TranscriptMeta,Turn } from '../domain/types';
import { dictionary,object,optional,text } from '../domain/validate';
import type { Account,AccountChromeHost,Attachment,ChildView,FooterView,ModelWire,RouteModel,SentenceHost,SentencePart,SentenceSnapshot,ShellDestination } from '../lib';
import { createAccountChrome,createImageViewer,createLiveController,createOrdering,createPagerController,createShellChrome,createStatusNote,measureViewerBar,orderRows,ownsScreen,parseAccount,parseRoute,releaseGeometry,releaseScreen,renderFullTool,renderPlaceholder,renderSessionsScreen,renderShellNavigation,requestJson,routeUrl,screenKind,setGeometry,updateSessionClock,updateSessionPager } from "../lib";
import { ApiError } from '../lib/model';
import type { ToolData } from '../lib/tool-details';
import type { ApplicationRoute,HostFocus } from '../navigation/routes';
import { NavigationController } from "../navigation/routes";
import { ViewerModelStore } from "../state/model";
import type { PageDirection } from '../state/transcript';
import { TranscriptStore } from "../state/transcript";
import { parseEntry } from '../state/transcript-wire';
import type { ViewerHost } from '../viewer-host';
import { getViewerHost } from "../viewer-host";
import { createAnalytics } from './analytics';
import { createDestination } from './destination';
import { EffectScope } from './effects';
import { createLiveUpdates } from './liveUpdates';
import { createRecentNavigation } from './recentNavigation';
import { HARNESS,HARNESSES,HARNESS_SHORT,I,STATE } from './registry';
import { createScreenViews } from './screenViews';
import { createSessionChrome } from './sessionChrome';
import { createTranscriptView } from './transcriptView';
import { createViewport } from './viewport';
export interface ViewerApplication { destroy(): void }
let mounted: ViewerApplication | null = null;
type ToolEntry = Extract<Entry, {k: 'tool'}> & {full?: boolean; scriptLoaded?: boolean};
interface SlotContext { destroy?: () => void; sync: () => void; onChange: () => void }
interface ControlSlot { route: ApplicationRoute; box: HTMLElement; ctx: SlotContext; el: HTMLElement }
interface TreePref { open: boolean; at: number }
export function mountViewerApplication(host: ViewerHost | null = getViewerHost()): ViewerApplication {
  mounted?.destroy();
  const scope = new EffectScope(), dialogs = new Map<HTMLDialogElement, {destroy(): void}>();
  let disposed = false;
  const application: ViewerApplication = {destroy() {
    if (disposed) return; disposed = true;
    scope.destroy(); liveController.destroy(); navigation.destroy(); transcripts.destroy();
    stopOpeningEndPin(); pagerController.disconnect(); errorNavigation.destroy(); scrollController.destroy();
    for (const dialog of dialogs.values()) dialog.destroy(); dialogs.clear();
    for (const slot of SLOTS.values()) slot.ctx.destroy?.(); SLOTS.clear();
    if (!SIDEBAR_ONLY) releaseScreen($('#page'));
    recentRenderer.destroy(); viewerBar.destroy(); shellChrome?.destroy(); accountChrome.destroy();
    releaseGeometry(app); releaseGeometry(document.documentElement, ['barHeight']);
    document.querySelectorAll('.livenote, .livenote-side').forEach(node => node.remove());
    document.documentElement.classList.remove('viewer-open', 'panel-open');
    if (mounted === application) mounted = null;
  }};
  mounted = application;

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
  let ADMIN: {href: string; label: string} | null = null;
  let ACCOUNT: Account | null = null;
  const viewerHost = host;
  const NATIVE_PAGE = viewerHost?.nativePage;
  let NAV_MACHINES = viewerHost?.machinesPath ?? null;
  // An embedding page that shows the viewer's sidebar beside its own content marks its .app data-viewer="sidebar" (docs/shell.md):
  // only the sidebar is drawn there, and every destination opens the viewer's own page.
  const SIDEBAR_ONLY = document.querySelector<HTMLElement>(".app")?.dataset.viewer === "sidebar";
  // The harnesses Semon can name. Mirrors crates/semon-sessions/src/harness.rs (a Rust test keeps them equal). Icons identify the source only; the artwork is served unmodified.
  const SESS = modelStore.sessions;
  const H = modelStore.handoffs;
  const transcripts = new TranscriptStore({ request: (path, signal) => api(path, signal), turns: (sid) => modelStore.turns[sid] ?? [], turn: (id) => modelStore.turn.get(id), entry: (e) => txEntry(e), cleared(sid) { if (navigation.route.v === "session" && ("id" in navigation.route ? navigation.route.id : "") === sid) resetPagerInput(); } });
  const TX = transcripts.entries;
  const SEEN_KEY = "semon.seen", SEEN_LIMIT = 2000;
  const SEEN_RESULTS = (() => {
    try {
      const ids: unknown = JSON.parse(window.localStorage.getItem(SEEN_KEY) ?? "[]");
      if (!Array.isArray(ids)) return new Set<string>();
      const clean = ids.filter((id: unknown): id is string => typeof id === "string").slice(-SEEN_LIMIT), seen = new Set(clean);
      if (seen.size !== ids.length) try { window.localStorage.setItem(SEEN_KEY, JSON.stringify([...seen])); } catch {}
      return seen;
    } catch { return new Set<string>(); }
  })();

  // ====================================================================================
  const HID = modelStore.handoff;
  const TURNS = modelStore.turns, TURN = modelStore.turn, STARTS = modelStore.starts, HOLDS = modelStore.holds;
  const TXM = transcripts.meta; // per session: the loaded range of its transcript { from, to, total } and its totals { calls, errors }
  const domain = createDomain({ sessions: SESS, machines: MACHINE, handoffs: H, turns: TURNS, turn: TURN, starts: STARTS, holds: HOLDS, handoff: HID, transcriptMeta: TXM }, () => NOW, SEEN_RESULTS);
  const { nameOf, hcls, where, hostOf, machineLabels, machineLabel, branchOf, shortHost, parentOf, originHandoff, RANK, isResult, inbox, working, answersOf, statWord, hasTurn, oneLine, TOYOU, turnEnd, traceRoot, countOf, callsText, sessionChildren, childSessions, descendantsOf, TOTAL_TOKEN_KINDS, TOKEN_KINDS, asMoney, usageTotal, costForSessions, costForSession, costText, costMissing, TREE_RANK, urgentDescendant, childParts, defaultTreeOpen, kidRank, lineageOf, byState, onMachine, movedOff, movesOf, shortMoney } = domain;
  const clock = (t: number) => formatClock(t, NOW), ago = (t: number) => formatAgo(t, NOW), dur = (a: number, b: number | null | undefined) => formatDuration(a, b ?? undefined, NOW);
  const $ = <T extends HTMLElement = HTMLElement>(s: string, r: ParentNode = document) => r.querySelector<T>(s)!;
  // Instrument Sans sets the middle dot with little side bearing. Thin spaces keep separators readable without changing code.
  const spaced = (t: unknown) => String(t).replace(/ · /g, "\u2009 · \u2009").replace(/^· /, "·\u2009 ");
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
  const facetLine = (s: Session) => [s.kind ?? HARNESS[s.harness], MACHINE[s.machine], where(s)].join(" · ");
  function markSeenResults(handoffs: Iterable<Handoff>) {
    let changed = false;
    for (const h of handoffs) if (isResult(h) && typeof h.id === "string" && !SEEN_RESULTS.has(h.id)) {
      SEEN_RESULTS.add(h.id); changed = true;
    }
    while (SEEN_RESULTS.size > SEEN_LIMIT) SEEN_RESULTS.delete(SEEN_RESULTS.values().next().value!);
    if (changed) try { window.localStorage.setItem(SEEN_KEY, JSON.stringify([...SEEN_RESULTS])); } catch {}
  }
  // Inline marks: `code`, **bold**, *italic*, ~~strike~~, [text](url) and bare URLs. Everything goes in as text nodes;
  // only http(s) links are live, and they open in a new tab. Anything else stays literal text, <tags> included.
  // What you answered, when the logs kept it: h.answer holds one string per question, in the brief's order.
  function sentenceSnapshot(h: Handoff, viewer: string | null | undefined, links = false): SentenceSnapshot {
    const parts: SentencePart[] = [], text = (className: string, text: string) => parts.push({ className, text });
    const who = (id: string, action?: SentencePart["action"], label?: (name: string) => string) => { const name = nameOf(id), linked = links && action && id !== "you" && id !== viewer && SESS[id]; parts.push({ text: name, className: linked ? "who-link" : "who", action: linked ? action : undefined, label: linked ? label?.(name) : undefined }); };
    const sender: SentencePart["action"] = { kind: "sender", id: h.id }, recipient: SentencePart["action"] = { kind: "session", id: h.to, turn: STARTS.get(h.id)?.id };
    const senderLabel = (name: string) => "Open " + name + " where it sent this", recipientLabel = (name: string) => "Open " + name + " at the turn this started";
    let path;
    if (h.kind === "ask") { path = I.ask; who("you"); text("verb", " asked "); who(h.to, recipient, recipientLabel); }
    else if (h.kind === "spawn" || h.kind === "relay") {
      if (viewer === h.to) { path = I.in; text("verb", h.kind === "spawn" ? "Brief from " : "Relay from "); who(h.from, sender, senderLabel); }
      else { path = I.out; who(h.from, sender, senderLabel); text("verb", h.kind === "spawn" ? " handed off to " + (SESS[h.to]?.kind === "Subagent" ? "subagent" : SESS[h.to]?.kind ?? "") + " " : " relayed to "); who(h.to, recipient, recipientLabel); }
    } else if (h.kind === "move") {
      path = I.move; const short = machineShorts([h.fromMachine, h.toMachine].map((id) => [id, MACHINE[id] ?? id]));
      const machine = (id: string) => parts.push({ className: "verb mach", text: short.get(id) ?? id, tip: "Machine: " + (MACHINE[id] ?? id), action: links ? { kind: "machine", id } : undefined, label: links ? "Open machine " + (MACHINE[id] ?? id) : undefined });
      text("verb", "Semon moved "); who(h.to, { kind: "session", id: h.to }, (name: string) => "Open " + name); text("verb", " from "); machine(h.fromMachine); text("verb", " to "); machine(h.toMachine);
    } else if (h.kind === "toyou") { path = h.status === "done" && (h.ask === "question" || h.ask === "decision") ? I.done : h.ask === "question" ? I.qc : h.ask === "decision" ? I.decide : I.result; who(h.from, sender, senderLabel); text("verb", { question: " asked you", result: " sent you a result", decision: " needs your decision" }[h.ask]); }
    return { icon: path ?? "", parts };
  }
  const sentenceHost: SentenceHost = { session(id, turn) { goSession(id, turn); }, machine(id) { go({ v: "machine", id }); }, sender(id) { const h = HID.get(id); if (h) openSender(h); } };
  // ---- Turns and traces, computed from the transcripts ---------------------------------------------------
  // A turn starts at each incoming entry: your message, a relay from another session, or the brief that starts a
  // subagent or Codex run. It runs to the next incoming entry. What it sends on (spawns, relays, messages to you)
  // and machine moves are content inside it. Entries before the first incoming one form a leading turn without a
  // header. A turn's id is its start handoff's id, else session:index. STARTS maps a handoff to the turn it started;
  // HOLDS maps it to the turn that sent it.
  // A gap marker ("Earlier entries not included in this copy") is where this copy skips part of the log. It ends the
  // turn before it, and what follows starts a turn of its own, so the marker is drawn between turns, never inside one.
  const isGap = (e: Entry) => e.k === "end" && /entries (not included|omitted)|^No activity/.test(e.text ?? "");
  // TURNS, TURN, STARTS and HOLDS keep the shapes above, filled from the server's turn index (adopt), since the page no longer
  // holds every transcript. A turn's entries are the loaded ones (spread).
  // ---- Loading: the model from /api/model, transcripts a page at a time from /api/tx --------------------------
  // A session's tool calls and errors, from the model (`calls` and `errors` on each session; absent from an older server or
  // cache: null, shown as "—"). Nothing fetches a transcript only to count it. A transcript loaded to its end is tailed by
  // every update, so its own totals agree with its entries; a range that stops short (a deep link, a child's start turn)
  // keeps the totals from when it was fetched, so the model's win there.
  let serverNow = 0, fetchedAt = 0;
  let TOK: Record<string,string> = {}; // per session: its transcript's growth mark in the model; a loaded transcript is tailed only when it moved
  const enc = encodeURIComponent;
  const safePath = (href: unknown) => typeof href === "string" && href.startsWith("/") && !href.startsWith("//") && !href.includes("\\") && !/[\u0000-\u001f\u007f-\u009f]/.test(href) && href.length <= 512;
  const accountOf = parseAccount;
  // The embedding page's menu, `window.semonEmbed.account`, validated like a server's. A rejected one is reported once on
  // the console, since the embedding page gets no other sign of it.
  let embedWarned = false;
  function embeddedAccount() {
    let source: unknown; try { const config: unknown = Reflect.get(window, 'semonEmbed'); source = config && typeof config === 'object' && 'account' in config ? config.account : undefined; } catch { source = undefined; }
    const account = accountOf(source);
    if (source != null && !account && !embedWarned) { embedWarned = true; console.warn("semon: window.semonEmbed.account was rejected (see the account menu rules)"); }
    return account;
  }
  // An error carries the HTTP status (0: no response), so live polling can tell a 403 from a dropped connection.
  async function api(path: string, signal?: AbortSignal, unchanged = false): Promise<unknown> {
    if (disposed) throw new DOMException('Viewer destroyed', 'AbortError');
    const controller = scope.request(), release = () => scope.releaseRequest(controller);
    try {
      const response = await requestJson(path, signal ? AbortSignal.any([signal, controller.signal]) : controller.signal, unchanged);
      if (disposed) throw new DOMException('Viewer destroyed', 'AbortError'); return response;
    } finally { release(); }
  }
  // NOW follows the client clock from the model's `now`, so every "ago" keeps moving; a running tool's age follows NOW.
  function tick() {
    NOW = serverNow + (Date.now() - fetchedAt);
    for (const s of Object.values(SESS)) if (s.activity && s.activity[3] != null) s.activity[2] = Math.floor((NOW - s.activity[3]) / 1000);
  }
  function adopt(value: unknown) {
    const m = modelStore.adopt(value);
    domain.invalidate();
    serverNow = m.now; fetchedAt = Date.now(); TOK = m.tx == null ? {} : dictionary(m.tx, text); transcripts.marks = TOK;
    const admin = m.admin == null ? null : object(m.admin);
    ADMIN = admin && typeof admin.href === "string" && safePath(admin.href) ? {href: admin.href, label: text(admin.label)} : null;
    // A server-provided menu wins; otherwise an embedding page may set `window.semonEmbed.account`, held to the same rules.
    ACCOUNT = accountOf(m.account) ?? accountOf(viewerHost?.account) ?? embeddedAccount();
    viewerHost?.modelAccount?.(ACCOUNT);
    const nav = m.nav == null ? null : object(m.nav);
    NAV_MACHINES = viewerHost?.machinesPath ?? (nav && typeof nav.machines === "string" && safePath(nav.machines) ? nav.machines : null);
    tick(); return m;
  }
  // Each loaded entry goes to its turn: an entry that starts a turn (or a page) names it. Its key, its turn and place in
  // it, stays the same while the transcript only grows: live updates find what was open and where the reader was by it.
  const spread = (sid: string) => transcripts.spread(sid);
  // A return line arrives as data; it reads as the mockup's "Returned to … · HH:MM".
  const txEntry = (e: Entry): Entry => e.k === "end" && e.ret ? { k: "end", text: "Returned to " + nameOf(e.ret.to) + (e.ret.failed ? " · failed" : "") + (e.ret.at != null ? " · " + clock(e.ret.at) : ""), turn: e.turn } : e;
  // where: "before" and "after" extend the loaded range; otherwise the page replaces it.
  const fetchTx = (sid: string, q: string = "", where: PageDirection | undefined = undefined, signal: AbortSignal | undefined = undefined, onPage: (() => void) | undefined = undefined) => transcripts.fetch(sid, q, where, signal, onPage);
  // What a route needs before it can draw: a session's page (the one holding a deep-linked turn).
  // `signal` cancels what a navigation asked for when the reader goes elsewhere first.
  function load(r: ApplicationRoute, signal: AbortSignal | undefined = undefined) {
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
  const cacheTx = (sid: string, entries: Entry[], meta: TranscriptMeta) => transcripts.keep(sid, entries, meta, !!originHandoff(sid));
  const adoptCached = (r: Extract<ApplicationRoute, {v: "session"}>) => transcripts.adoptCached(r.id, r.turn);
  // A mark with fewer entries or bytes than the one loaded means the file was cut or rewritten: load it again.
  function shrank(a: string | undefined, b: string | undefined) { const [s0, b0] = String(a).split(".").map(Number), [s1, b1] = String(b).split(".").map(Number); return s1 < s0 || b1 < b0; }
  // A transcript drawn from the cache is brought up to date the way a live update does it: when the model's mark for it moved
  // since it was kept, its tail is fetched (or the whole page, if the file shrank), and its child work loads. Nothing is asked
  // for when the mark is the same. The page is drawn again, keeping the reader's place, once something arrived.
  function revalidate(r: Extract<ApplicationRoute, {v: "session"}>) {
    const sid = r.id, m = TXM[sid], moved = m && m.to >= m.total && m.tok != null && TOK[sid] != null && m.tok !== TOK[sid];
    const job = moved ? (shrank(m.tok, TOK[sid]) ? reload(sid) : tail(sid)) : null, work = job;
    if (work) work.then(() => { if (navigation.route === r && navigation.rendered === r) refresh(null); }, () => {});
  }
  // Paging state survives redraws: a click and an observer share one request per session and direction, and a failed
  // page stays manual until Retry succeeds. Observers belong only to the buttons currently drawn.
  const pagingStore = transcripts.paging, PAGING = pagingStore.states;
  let pagerArmed = false, automaticLoads = 0, scrollRevision = 0, programmaticScrollPending = false, programmaticScrollTimer: number | undefined = undefined;
  const clearPaging = (sid: string) => transcripts.clearPaging(sid);
  const dropTx = (sid: string) => transcripts.drop(sid);
  function resetPagerInput() { pagerArmed = false; automaticLoads = 0; scrollRevision++; disconnectPagerObservers(); }
  function holdProgrammaticScroll() {
    programmaticScrollPending = true;
    clearTimeout(programmaticScrollTimer);
    // Scroll events arrive after scrollTop writes. Smooth jumps keep extending this guard until scrolling is quiet.
    programmaticScrollTimer = scope.timeout(() => { programmaticScrollPending = false; programmaticScrollTimer = undefined; }, 120);
  }
  function scrollProgrammatically(fn: () => void, jump = true) {
    if (jump) resetPagerInput();
    holdProgrammaticScroll(); fn();
  }
  function readerScrollInput() {
    if (navigation.route.v !== "session" || navigation.rendered !== navigation.route || $("#page").hasAttribute("aria-busy")) return;
    clearTimeout(programmaticScrollTimer); programmaticScrollTimer = undefined; programmaticScrollPending = false;
    stopOpeningEndPin(); pagerArmed = true; automaticLoads = 0; scrollRevision++; queuePagerObservers();
  }
  const automaticPagingAllowed = () => pagerArmed && automaticLoads < 3 && !viewport.openingEndUntil && !findOpen && !find && show.messages && show.tools && show.thinking;
  const pagingState = (sid: string, where: PageDirection) => pagingStore.get(sid, where);
  function pagerSnapshot(sid: string, where: PageDirection) {
    const state = pagingState(sid, where), direction = where === "before" ? "earlier" : "later";
    return { sid, where, disabled: state.busy, busy: state.busy, text: state.busy ? "Loading " + direction + "…" : state.failed ? "Couldn't load " + direction + " entries · Retry" : "Load " + direction + (where === "after" && TXM[sid]?.newer ? " · " + TXM[sid].newer + " new" : "") };
  }
  function paintPager(b: HTMLElement) { updateSessionPager($("#page"), pagerSnapshot(b.dataset.pagerSid!, b.dataset.pagerWhere === "before" ? "before" : "after")); }
  const pagerController = createPagerController({
    route(sid) { return navigation.route.v === "session" && ("id" in navigation.route ? navigation.route.id : "") === sid && navigation.rendered === navigation.route ? navigation.route : null; }, range(sid) { return TXM[sid]; }, state: pagingState, automatic: automaticPagingAllowed,
    current(sid, where, state, r) { return PAGING.get(sid)?.[where] === state; }, beginManual: () => stopOpeningEndPin(), countAutomatic() { automaticLoads++; }, paint: paintPager,
    load(sid, where, boundary, signal, applied) { return fetchTx(sid, where + "=" + boundary, where, signal, applied); },
    commit(r, where, manual) {
      const box = scroller(), top = phone.matches ? 0 : box.getBoundingClientRect().top, st = capture();
      const entry = [...$("#page").querySelectorAll<HTMLElement>(".turns [data-e][data-entry-key]:not(.tgroup)")].find((n) => { const rect = n.getBoundingClientRect(); return rect.height && rect.top >= top; });
      st.paging = { anchor: entry ? { key: entry.dataset.entryKey, off: entry.getBoundingClientRect().top - top } : null, height: box.scrollHeight, before: where === "before" };
      const armed = pagerArmed, used = automaticLoads; render(); restore(st); if (!manual) { pagerArmed = armed; automaticLoads = used; } syncJump(); saveHistoryScroll();
    }, queue: queuePagerObservers,
  });
  function disconnectPagerObservers() { pagerController.disconnect(); }
  function queuePagerObservers() {
    if (disposed || SIDEBAR_ONLY) return;
    pagerController.queue($("#page"), phone.matches ? null : $("#main"), () => navigation.route.v === "session" && navigation.rendered === navigation.route && automaticPagingAllowed() && !$("#page").hasAttribute("aria-busy"));
  }
  function loadPager(button: HTMLButtonElement, manual: boolean) { return pagerController.load(button, manual); }
  // "Load earlier" at the top of a transcript, and "Load later" at its end when a deep link loaded a middle page.

  // "View all" reads the whole call: each part the server cut ("more") is fetched in full from /api/entry, by the entry's
  // session and slot. A part cut again at 8 MB is noted (fullCut).
  function parseToolDetail(data: Record<string, unknown>): ToolData {
    const entry = parseEntry({ ...data, k: 'tool', name: '', arg: '', ok: true });
    if (entry.k !== 'tool') throw new Error('Invalid tool details'); return entry;
  }
  function fullOf(e: ToolEntry): Promise<Partial<Omit<ToolData, 'bg'>>> {
    return Promise.all((e.more ?? []).map(async part => ({part, data: object(await api('/api/entry?sid=' + enc(e.sid ?? '') + '&slot=' + e.slot + '&as=' + part))}))).then(parts => {
      const full: Partial<Omit<ToolData, 'bg'>> = {}, fullCut: string[] = [];
      for (const {part, data} of parts) {
        if (part === 'diff') { full.diff = parseToolDetail(data).diff; full.changes = parseToolDetail(data).changes; }
        else if (part === 'out') { full.out = optional(data.text, text); full.cut = parseToolDetail(data).cut; }
        else if (part === 'in') full.in = optional(data.text, text);
        else if (part === 'arg') full.arg = optional(data.text, text);
        if (data.truncated === true) fullCut.push(part);
      }
      full.fullCut = fullCut; return full;
    });
  }
  // Real URLs: every screen has one, and the server serves this page for each.
  const routeModel: RouteModel = { session(id) { return SESS[id]; }, machine(id) { return !!MACHINE[id]; }, turn(id) { return TURN.get(id); }, get machinesPath() { return viewerHost?.machinesPath; } };
  const urlOf = (r: ApplicationRoute) => routeUrl(r, routeModel), routeOf = (location: Pick<Location,"pathname" | "search" | "hash">) => parseRoute(location, routeModel);
  function boot() {
    api("/api/model?delta=1").then((m) => {
      const adopted = adopt(m); LIVE.version = adopted.version; remember(adopted); if (SIDEBAR_ONLY || NATIVE_PAGE) { if (NATIVE_PAGE) navigation.route = navigation.historyRoute({ v: NATIVE_PAGE.nav }, {v: 'home'}); render(); schedule(2000); return; } navigation.route = routeOf(location);
      if (navigation.route.v === "sessions") query = (new URLSearchParams(location.search).get("q") ?? "").trim(); // direct Sessions links can prefill its search field
      if (navigation.route.v === "machines" && NAV_MACHINES && !viewerHost) { location.assign(NAV_MACHINES); return; }
      try { history.replaceState({ ...navigation.route, scrollTop: 0 }, "", urlOf(navigation.route) + (navigation.route.v === "session" ? location.hash : navigation.route.v === "sessions" && query ? "?q=" + enc(query) : "")); } catch {}
      const done = () => {
        render();
        if (navigation.route.v === "session" && ("turn" in navigation.route ? navigation.route.turn : undefined)) { revealTurn(("turn" in navigation.route ? navigation.route.turn : undefined) ?? "", true); if (location.hash) scope.frame(() => scope.frame(revealEntryHash)); }
        else if (navigation.route.v === "session" && location.hash) revealEntryHash();
        else if (navigation.route.v === "session") { openSessionAtEnd(); syncJump(); }
        else quietTop();
        schedule(2000); scope.interval(ticker, 1000);
      };
      const initialRoute = navigation.route, p = load(initialRoute);
      if (p) p.then(() => { done();  }, done);
      else { done();  }
    }, (err) => { if (disposed) return; if (viewerHost?.modelFailed?.(err?.status ?? 0)) return; if (viewerHost) { console.warn("semon: model unavailable", err.status); return; } renderPlaceholder($(SIDEBAR_ONLY ? "#lanes" : "#page"), "Couldn't load the sessions: " + err.message); });
  }

  // ---- State & navigation ---------------------------------------------------------------
  const phone = window.matchMedia("(max-width: 760px)");
  const navigation = new NavigationController({ model: routeModel, loadMachines: viewerHost ? (signal) => viewerHost.loadMachines(signal) : undefined }, viewerHost?.initialMachines ?? null); // (before the layout preferences, which read it)
  let wideMode = false, railMode = false; let treePrefs: Record<string,TreePref> = {};
  try { wideMode = localStorage.getItem("semon.wide") === "1"; } catch {}
  try { railMode = !SIDEBAR_ONLY && localStorage.getItem("semon.rail") === "1"; } catch {} // the rail is the viewer's own layout: an embedding page keeps its sidebar whole
  try { const saved = JSON.parse(localStorage.getItem("semon.tree") ?? "{}"); if (saved && typeof saved === "object" && !Array.isArray(saved)) treePrefs = pruneTreePrefs(saved); } catch {}
  const app = $(".app");
  const syncLayoutPrefs = () => { if (SIDEBAR_ONLY) return; app.classList.toggle("rail", railMode && !phone.matches); $("#page").classList.toggle("wide-mode", wideMode && !phone.matches && navigation.route.v === "session"); };
  function setWideMode(on: boolean) { wideMode = on; try { localStorage.setItem("semon.wide", on ? "1" : "0"); } catch {} syncLayoutPrefs(); $(".wide-toggle")?.setAttribute("aria-pressed", String(on)); accountChrome.updateWide(on); }
  function setRailMode(on: boolean) { railMode = on; ORD.delete("side"); try { localStorage.setItem("semon.rail", on ? "1" : "0"); } catch {} syncLayoutPrefs(); recentNavigation.expandedAll = null; renderLanes(); const b = $("#rail-toggle"); b?.setAttribute("aria-expanded", String(!on)); b?.setAttribute("aria-label", on ? "Expand sidebar" : "Collapse sidebar"); b?.setAttribute("data-tip", on ? "Expand sidebar" : "Collapse sidebar"); }
  // A parent's saved choice is whether it is `open`. Saves from before the sidebar's "All N" row also held `more`, which nothing reads now:
  // it is dropped on load, along with any entry that has no `open`, and the next save writes the pruned list.
  function pruneTreePrefs(saved: unknown) {
    const kept: Record<string,TreePref> = {};
    if (!saved || typeof saved !== "object") return kept;
    for (const [id, pref] of Object.entries(saved)) if (pref && typeof pref === "object" && "open" in pref && typeof pref.open === "boolean") kept[id] = { open: pref.open, at: "at" in pref ? Number(pref.at) || 0 : 0 };
    return kept;
  }
  function saveTreePref(id: string, open: boolean) {
    treePrefs[id] = { open, at: Date.now() };
    treePrefs = Object.fromEntries(Object.entries(treePrefs).sort((a, b) => (b[1]?.at ?? 0) - (a[1]?.at ?? 0)).slice(0, 500));
    try { localStorage.setItem("semon.tree", JSON.stringify(treePrefs)); } catch {}
  }
  // An embedding page's sidebar has no rail and no toggle for it (shell::session_sidebar): the toggle is then a detached button.
  const railToggle = SIDEBAR_ONLY ? ($("#rail-toggle") ?? document.createElement("button")) : document.createElement("button");  railToggle.setAttribute("aria-expanded", String(!railMode)); railToggle.setAttribute("data-tip", railMode ? "Expand sidebar" : "Collapse sidebar"); railToggle.setAttribute("aria-label", railMode ? "Expand sidebar" : "Collapse sidebar"); scope.listen(railToggle, "click", () => setRailMode(!railMode)); syncLayoutPrefs();
  let groupBy = "recent"; let query = ""; let focusSessionsSearchOnRender: ApplicationRoute | null = null; let analyticsRange = 7, analyticsMeasure = "hours";
  let showApprovalReviews = false;
  const sessionFilters = { repo: "", machine: "", harness: "", model: "" };
  let pendingSessionOpen: string | null = null;
  // The phone's account menu adds a history entry, so the back gesture closes it.
  let accountSheet = false;
  // What to do once the account menu's history entry has been stepped back over (leaving the page from one of its items).
  let afterPop: (() => void) | null = null;
  if (!SIDEBAR_ONLY) try { history.scrollRestoration = "manual"; } catch {}
  const SHOW_ALL = { messages: true, tools: true, thinking: true };
  let show = { ...SHOW_ALL }; let find = ""; let findOpen = false;
  const currentScroll = () => phone.matches ? window.scrollY : $("#main").scrollTop;
  const restoreScroll = (top: number) => scrollProgrammatically(() => { if (phone.matches) window.scrollTo(0, top); else $("#main").scrollTop = top; });
  let hostFocus: HostFocus | undefined;
  if (viewerHost) scope.listen(document, "focusin", (event) => {
    const node = event.target;
    if (!(node instanceof HTMLElement) || !$("#page").contains(node)) return;
    hostFocus = node.id ? { id: node.id } : node.dataset.id ? { row: node.dataset.id } : node.getAttribute("aria-label") ? { label: node.getAttribute("aria-label") ?? undefined } : undefined;
  });
  function restoreHostFocus(saved: HostFocus | undefined) {
    if (!viewerHost || !saved) return;
    const selector = saved.id ? "#" + CSS.escape(saved.id) : saved.row ? '[data-id="' + CSS.escape(saved.row) + '"]' : saved.label ? '[aria-label="' + CSS.escape(saved.label) + '"]' : null;
    if (selector) $("#page")?.querySelector<HTMLElement>(selector)?.focus({ preventScroll: true });
  }
  const saveHistoryScroll = () => { if (viewerEl) return; try { if (history.state?.v) history.replaceState({ ...history.state, scrollTop: currentScroll(), ...(viewerHost ? { hostFocus } : {}) }, ""); } catch {} };
  let scrollSaveFrame = false;
  const queueScrollSave = () => { if (scrollSaveFrame) return; scrollSaveFrame = true; scope.frame(() => { scrollSaveFrame = false; saveHistoryScroll(); }); };
  if (!SIDEBAR_ONLY) { scope.listen(window, "scroll", queueScrollSave, { passive: true }); scope.listen($("#main"), "scroll", queueScrollSave, { passive: true }); }
  const quietTop = () => restoreScroll(0);
  const destination = createDestination({
get startOpeningEndPin() { return startOpeningEndPin; },
get $() { return $; },
get renderNav() { return renderNav; },
get renderLanes() { return renderLanes; },
get drawSessionBar() { return drawSessionBar; },
get LIVE() { return LIVE; },
get scope() { return scope; },
get navigation() { return navigation; },
get quietTop() { return quietTop; },
get syncBarLine() { return syncBarLine; },
get TURN() { return TURN; },
get NATIVE_PAGE() { return NATIVE_PAGE; },
get NAV_MACHINES() { return NAV_MACHINES; }, set NAV_MACHINES(value) { NAV_MACHINES = value; },
get urlOf() { return urlOf; },
get viewerHost() { return viewerHost; },
get closeDrawer() { return closeDrawer; },
get closeAccountMenu() { return closeAccountMenu; },
get focusSessionsSearchOnRender() { return focusSessionsSearchOnRender; }, set focusSessionsSearchOnRender(value) { focusSessionsSearchOnRender = value; },
get SIDEBAR_ONLY() { return SIDEBAR_ONLY; },
get clearPaging() { return clearPaging; },
get resetPagerInput() { return resetPagerInput; },
get stopOpeningEndPin() { return stopOpeningEndPin; },
get saveHistoryScroll() { return saveHistoryScroll; },
get dropErrors() { return dropErrors; },
get TX() { return TX; },
get TXM() { return TXM; },
get originHandoff() { return originHandoff; },
get cacheTx() { return cacheTx; },
get show() { return show; }, set show(value) { show = value; },
get SHOW_ALL() { return SHOW_ALL; },
get ORD() { return ORD; },
get find() { return find; }, set find(value) { find = value; },
get findOpen() { return findOpen; }, set findOpen(value) { findOpen = value; },
get clearNewEntries() { return clearNewEntries; },
get render() { return render; },
get restoreScroll() { return restoreScroll; },
get restoreHostFocus() { return restoreHostFocus; },
get syncJump() { return syncJump; },
get SESS() { return SESS; },
get STALE_BRIEFS() { return STALE_BRIEFS; },
get TXCACHE() { return TXCACHE; },
get adoptCached() { return adoptCached; },
get spread() { return spread; },
get revalidate() { return revalidate; },
get load() { return load; },
get skipPop() { return skipPop; }, set skipPop(value) { skipPop = value; },
get afterPop() { return afterPop; }, set afterPop(value) { afterPop = value; },
get pendingSessionOpen() { return pendingSessionOpen; }, set pendingSessionOpen(value) { pendingSessionOpen = value; },
get accountSheet() { return accountSheet; }, set accountSheet(value) { accountSheet = value; },
get viewerEl() { return viewerEl; }, set viewerEl(value) { viewerEl = value; },
get routeOf() { return routeOf; },
get HOLDS() { return HOLDS; },
get scrollProgrammatically() { return scrollProgrammatically; },
get phone() { return phone; }
  });
  const {goSession, go, openSender, revealTurn, revealEntryHash, openSessionAtEnd, goTrace} = destination;
  const accountHost: AccountChromeHost = {
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
      if (disposed) return;
      if (accountSheet) { accountSheet = false; if (!keepEntry && history.state?.sheet) { skipPop = true; history.back(); } }
      if (LIVE.pending && !navigating) scope.timeout(() => { if (LIVE.pending && !viewerEl && !accountChrome.open) refresh(); }, 0);
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
    navigate(destination) { go(navigation.historyRoute({ v: destination.key }, {v: "home"})); return true; },
    drawerOpened() { orderApply("side"); },
    drawerClosed() { scope.timeout(() => { if (phone.matches && !document.body.classList.contains("drawer-open")) orderApply("side"); }, ORD_DRAWER_MS); },
    railChanged() { setRailMode(!railMode); renderNav(); },
  });
  if (shellChrome) shellChrome.mount(app);
  const accountChrome = shellChrome?.account ?? createAccountChrome(accountHost);
  function closeAccountMenu(keepEntry: boolean | undefined = undefined, navigating: boolean | undefined = undefined) { accountChrome.close({ keepEntry, navigating }); }
  // Step over the phone sheet before leaving so Back lands on the page, not a removed menu.
  function leaveAccountSheet(go: () => void) {
    accountSheet = false;
    if (history.state?.sheet) { skipPop = true; afterPop = go; history.back(); } else go();
  }
  function accountWidget(compact: boolean) {
    return ACCOUNT ? accountChrome.mount({ account: ACCOUNT, compact, wide: wideMode, onWideChange: () => setWideMode(!wideMode) }) : null;
  }
  function renderDrawerAccount() {
    if (shellChrome) { shellChrome.drawerAccount(ACCOUNT ? { account: ACCOUNT, compact: true, wide: wideMode, onWideChange: () => setWideMode(!wideMode) } : null); return; }
    const old = $("#account-drawer");
    if (old) { accountChrome.unmount(old); old.remove(); }
    if (!ACCOUNT) return;
    const widget = accountWidget(true); if (!widget) return; widget.id = "account-drawer"; $("#sidebar").append(widget);
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
  const ordering = createOrdering<ApplicationRoute>(), ORD = ordering.scopes, ordTouch = { down: false }, byLast = (a: Session, b: Session) => b.last - a.last;
  const pageSig = () => JSON.stringify([query, groupBy, sessionFilters]);
  scope.listen(document, "pointerdown", () => { ordTouch.down = true; }, true);
  for (const t of ["pointerup", "pointercancel"]) scope.listen(document, t, () => { ordTouch.down = false; }, true);
  scope.listen(window, "blur", () => { ordTouch.down = false; });
  // A tab that comes back from the background applies what was held, once, before it paints, from the data it has. What the catch-up
  // poll brings after that is held like any other update.
  scope.listen(document, "visibilitychange", () => { ordTouch.down = false; if (visible()) { orderApply("page"); orderApply("side"); } });
  // How many rows must move to put `rows` in `cmp` order: all of them but the longest run already in it.
  // Whether the reader could be using a screen's list: is its top in view, is it touched. Read before a draw empties the list (an
  // emptied scroller clamps to 0 and the rows and their hover are gone). Focus counts only when it is keyboard focus on a row of this
  // list, a mouse only over one of its rows.
  function ordState(name: string) {
    const side = name === "side", rows = side ? "#lanes .treeitem" : "#page .nrow", a = document.activeElement;
    const inView = side ? sideRegion().scrollTop <= 1 : !$(rows) || $(rows).getBoundingClientRect().top >= $("#topbar").getBoundingClientRect().bottom - 1;
    const touched = ordTouch.down || (!!a?.matches?.(":focus-visible") && !!a.closest(side ? "#lanes" : rows)) || (matchMedia("(hover: hover)").matches && !!$(rows + ":hover"));
    return { inView, touched };
  }
  let ordPageState: ReturnType<typeof ordState> | null = null;
  const pageState = () => ordPageState ?? ordState("page");
  // One draw of a screen's lists. `sig` is what the reader chose to show, `tie` the route it belongs to (null: the sidebar, which
  // outlives routes); a draw with another sig or tie starts from the sorted order. `st` is ordState from before the draw.
  const orderScope = (name: string, sig: string, tie: ApplicationRoute | null, state: {inView: boolean; touched: boolean}) => ordering.begin(name, sig, tie, state), orderList = orderRows;
  const sideRegion = () => $("#side-list") ?? $("#sidebar");
  // How long the wide screen's sidebar is left alone before it applies what it holds. Read once, at load, with 10 s as the default; a
  // browser check may set window.__semonOrderIdleMs to a finite number from 200 to 60000 ms before load; other values use the default.
  const ordIdleMs = Reflect.get(window,"__semonOrderIdleMs");
  const ORD_IDLE_MS = Number.isFinite(ordIdleMs) && ordIdleMs >= 200 && ordIdleMs <= 60000 ? ordIdleMs : 10000;
  const ORD_DRAWER_MS = 320; // the drawer's slide (0.24 s) and a little
  // Applies what a screen holds: the list is drawn sorted, from scratch.
  function orderApply(name: string) {
    const sc = ORD.get(name); if (!sc?.n || (name === "page" && (sc.tie !== navigation.route || navigation.rendered !== navigation.route || viewerEl || $("#page").hasAttribute("aria-busy")))) return;
    ORD.delete(name);
    if (name === "side") renderLanes(); else { const st = capture(); render(); restore(st); }
  }
  // The wide screen's sidebar is always in view: what it holds is applied once it has been left alone for ORD_IDLE_MS. Left alone means
  // no pointer over it or down, no keyboard focus or focused text field in it, and no menu or dialog open (the session menu hangs from
  // the top bar, outside the sidebar, so it is named here); anything the reader does to it starts the wait again.
  let ordIdle: number | undefined;
  function ordIdleArm() {
    clearTimeout(ordIdle); ordIdle = undefined;
    if (phone.matches || !ORD.get("side")?.n) return;
    ordIdle = scope.timeout(() => {
      ordIdle = undefined; const bar = $("#sidebar"), a = document.activeElement;
      const menu = $(".session-menu, .account-popover, .runs-popover, .filters.pop:not([hidden])");
      if (ordTouch.down || bar.matches(":hover") || (a && bar.contains(a) && (a.matches(":focus-visible") || a.matches("input, textarea, select, [contenteditable]"))) || menu || viewerEl) ordIdleArm(); else orderApply("side");
    }, ORD_IDLE_MS);
  }
  for (const t of ["pointermove", "pointerdown", "pointerleave", "focusin", "focusout", "wheel", "keydown"]) scope.listen($("#sidebar"), t, () => { if (ordIdle) ordIdleArm(); }, { passive: true });
  function renderNav() {
    const nav = $("#nav"), destinations: ShellDestination[] = [];
    // A session or a trace sits under Sessions, a machine under Machines.
    const under: Record<"home" | "analytics" | "sessions" | "machines", string[]> = { home: ["home"], analytics: ["analytics"], sessions: ["sessions", "session", "trace"], machines: ["machines", "machine"] };
    const item = (v: "home" | "analytics" | "sessions" | "machines", label: string, ic: string, count?: number, hot?: boolean) => { destinations.push({ key: v, label, icon: ic, href: v === "machines" && NAV_MACHINES ? NAV_MACHINES : urlOf({ v }), current: under[v].includes((SIDEBAR_ONLY ? app.dataset.viewerNav : NATIVE_PAGE?.nav) ?? navigation.route.v), count, hot }); };
    item("home", "Home", I.home, inbox().length, true);
    item("sessions", "Sessions", I.sessions);
    item("analytics", "Analytics", I.chart);
    item("machines", "Machines", I.machine, Object.keys(MACHINE).filter((m) => !MACHINE_UP[m]).length, true);
    if (shellChrome) shellChrome.update(destinations, railMode);
    else renderShellNavigation(nav, destinations, (destination) => { go(navigation.historyRoute({ v: destination.key }, {v: "home"})); return true; });
  }
  // Sessions match by name, repo, branch, machine, harness and the messages that started their turns.
  const sessMatch = (s: Session, q: string) => !q || [s.name, s.repo, s.branch, MACHINE[s.machine], s.movedFrom ? MACHINE[s.movedFrom] : "", HARNESS[s.harness], s.role ? "role no repo" : "", ...(TURNS[s.id] ?? []).map((t) => t.start?.brief ?? t.u?.text ?? "")].join(" ").toLowerCase().includes(q.toLowerCase());
  // Built once per model and per render (both drop it) and shared: callers copy an array before reordering it.
  const recentNavigation = createRecentNavigation({
get showApprovalReviews() { return showApprovalReviews; }, set showApprovalReviews(value) { showApprovalReviews = value; },
get sessionChildren() { return sessionChildren; },
get SESS() { return SESS; },
get parentOf() { return parentOf; },
get byLast() { return byLast; },
get navigation() { return navigation; },
get kidRank() { return kidRank; },
get orderList() { return orderList; },
get descendantsOf() { return descendantsOf; },
get $() { return $; },
get goSession() { return goSession; },
get saveTreePref() { return saveTreePref; },
get phone() { return phone; },
get scrollProgrammatically() { return scrollProgrammatically; },
get STATE() { return STATE; },
get ago() { return ago; },
get sessMatch() { return sessMatch; },
get pendingSessionOpen() { return pendingSessionOpen; }, set pendingSessionOpen(value) { pendingSessionOpen = value; },
get viewerEl() { return viewerEl; }, set viewerEl(value) { viewerEl = value; },
get SIDEBAR_ONLY() { return SIDEBAR_ONLY; },
get currentScroll() { return currentScroll; },
get disposed() { return disposed; }, set disposed(value) { disposed = value; },
get skipPop() { return skipPop; }, set skipPop(value) { skipPop = value; },
get LIVE() { return LIVE; },
get refresh() { return refresh; },
get dialogs() { return dialogs; },
get treePrefs() { return treePrefs; }, set treePrefs(value) { treePrefs = value; },
get defaultTreeOpen() { return defaultTreeOpen; },
get childParts() { return childParts; },
get HARNESSES() { return HARNESSES; },
get dur() { return dur; },
get I() { return I; },
get MACHINE() { return MACHINE; },
get shortHost() { return shortHost; },
get hostOf() { return hostOf; },
get HARNESS() { return HARNESS; },
get modelIdOf() { return modelIdOf; },
get darkTheme() { return darkTheme; },
get urgentDescendant() { return urgentDescendant; },
get railMode() { return railMode; }, set railMode(value) { railMode = value; },
get ORD() { return ORD; },
get ordState() { return ordState; },
get orderScope() { return orderScope; },
get ordIdle() { return ordIdle; }, set ordIdle(value) { ordIdle = value; },
get ordIdleArm() { return ordIdleArm; },
get query() { return query; }, set query(value) { query = value; }
  });
  const {recentRenderer, renderLanes, COST_TIP, isApprovalReview} = recentNavigation;
  // ---- Top bar ---------------------------------------------------------------------------------------
  // The same on every page: the menu button (phones), the title, and at most two actions. A detail page adds a crumb up a
  // level and a second line of labels. Labels are information, never a control: their full values are in the tooltip and in
  // the session menu. On a session, Find takes over the bar and the filters sit under it as chips.
  const {errorNavigation, viewerBar, drawSessionBar, syncBarLine, dropErrors, modelIdOf, kindText, observeTitle, centre, panel, renderTopbar, machineLine, sessionLine, errOn, markError, errorsLive} = createSessionChrome({
get HARNESS() { return HARNESS; },
get ACCOUNT() { return ACCOUNT; }, set ACCOUNT(value) { ACCOUNT = value; },
get wideMode() { return wideMode; }, set wideMode(value) { wideMode = value; },
get setWideMode() { return setWideMode; },
get findOpen() { return findOpen; }, set findOpen(value) { findOpen = value; },
get STATE() { return STATE; },
get phone() { return phone; },
get analyticsRange() { return analyticsRange; }, set analyticsRange(value) { analyticsRange = value; },
get find() { return find; }, set find(value) { find = value; },
get show() { return show; }, set show(value) { show = value; },
get countOf() { return countOf; },
get I() { return I; },
get goSession() { return goSession; },
get render() { return render; },
get $() { return $; },
get SHOW_ALL() { return SHOW_ALL; },
get SESS() { return SESS; },
get currentScroll() { return currentScroll; },
get restoreScroll() { return restoreScroll; },
get refreshAnalytics() { return refreshAnalytics; },
get navigation() { return navigation; },
get shellChrome() { return shellChrome; },
get scope() { return scope; },
get lineageOf() { return lineageOf; },
get scroller() { return scroller; },
get edge() { return edge; },
get scrollProgrammatically() { return scrollProgrammatically; },
get syncJump() { return syncJump; },
get saveHistoryScroll() { return saveHistoryScroll; },
get TX() { return TX; },
get TXM() { return TXM; },
get TOK() { return TOK; }, set TOK(value) { TOK = value; },
get SIDEBAR_ONLY() { return SIDEBAR_ONLY; },
get capture() { return capture; },
get restore() { return restore; },
get opener() { return opener; },
get resetPagerInput() { return resetPagerInput; },
get stopOpeningEndPin() { return stopOpeningEndPin; },
get fetchTx() { return fetchTx; },
get dropTx() { return dropTx; },
get spread() { return spread; },
get tail() { return tail; },
get TURNS() { return TURNS; },
get hasTurn() { return hasTurn; },
get descendantsOf() { return descendantsOf; },
get sessionChildren() { return sessionChildren; },
get costForSessions() { return costForSessions; },
get costForSession() { return costForSession; },
get MACHINE() { return MACHINE; },
get hostOf() { return hostOf; },
get costText() { return costText; },
get onMachine() { return onMachine; },
get MACHINE_UP() { return MACHINE_UP; },
get movedOff() { return movedOff; },
get MACHINE_LAST() { return MACHINE_LAST; },
get clock() { return clock; },
get syncLayoutPrefs() { return syncLayoutPrefs; },
get viewerEl() { return viewerEl; }, set viewerEl(value) { viewerEl = value; },
get dialogs() { return dialogs; },
get disposed() { return disposed; }, set disposed(value) { disposed = value; },
get skipPop() { return skipPop; }, set skipPop(value) { skipPop = value; },
get pendingSessionOpen() { return pendingSessionOpen; }, set pendingSessionOpen(value) { pendingSessionOpen = value; },
get LIVE() { return LIVE; },
get refresh() { return refresh; },
get TURN() { return TURN; },
get harnessSnapshot() { return harnessSnapshot; },
get branchOf() { return branchOf; },
get dur() { return dur; },
get costSnapshot() { return costSnapshot; },
get afterPop() { return afterPop; }, set afterPop(value) { afterPop = value; },
get goTrace() { return goTrace; }
  });
  const {harnessSnapshot, costSnapshot, showsFooter, renderHome, renderMachines, renderMachine, renderTrace, renderSession} = createScreenViews({
get costForSession() { return costForSession; },
get costMissing() { return costMissing; },
get costText() { return costText; },
get costForSessions() { return costForSessions; },
get HARNESS() { return HARNESS; },
get asMoney() { return asMoney; },
get sessionChildren() { return sessionChildren; },
get kindText() { return kindText; },
get STATE() { return STATE; },
get MACHINE() { return MACHINE; },
get MACHINE_UP() { return MACHINE_UP; },
get HARNESSES() { return HARNESSES; },
get darkTheme() { return darkTheme; },
get TURNS() { return TURNS; },
get H() { return H; },
get ago() { return ago; },
get nameOf() { return nameOf; },
get oneLine() { return oneLine; },
get HOLDS() { return HOLDS; },
get SESS() { return SESS; },
get I() { return I; },
get answersOf() { return answersOf; },
get traceRoot() { return traceRoot; },
get goSession() { return goSession; },
get observeTitle() { return observeTitle; },
get goTrace() { return goTrace; },
get inbox() { return inbox; },
get isResult() { return isResult; },
get markSeenResults() { return markSeenResults; },
get render() { return render; },
get working() { return working; },
get orderList() { return orderList; },
get orderScope() { return orderScope; },
get pageSig() { return pageSig; },
get navigation() { return navigation; },
get pageState() { return pageState; },
get byLast() { return byLast; },
get onMachine() { return onMachine; },
get movedOff() { return movedOff; },
get movesOf() { return movesOf; },
get clock() { return clock; },
get MACHINE_LAST() { return MACHINE_LAST; },
get ADMIN() { return ADMIN; }, set ADMIN(value) { ADMIN = value; },
get go() { return go; },
get byState() { return byState; },
get STARTS() { return STARTS; },
get domain() { return domain; },
get NOW() { return NOW; }, set NOW(value) { NOW = value; },
get TURN() { return TURN; },
get sentenceHost() { return sentenceHost; },
get machineLabel() { return machineLabel; },
get hcls() { return hcls; },
get hostOf() { return hostOf; },
get sentenceSnapshot() { return sentenceSnapshot; },
get turnEnd() { return turnEnd; },
get statWord() { return statWord; },
get SEEN_RESULTS() { return SEEN_RESULTS; },
get machineLabels() { return machineLabels; },
get transcriptEntries() { return transcriptEntries; },
get TX() { return TX; },
get transcriptSnapshot() { return transcriptSnapshot; },
get verb() { return verb; },
get openStepViewer() { return openStepViewer; },
get openScript() { return openScript; },
get openImage() { return openImage; },
get stopOpeningEndPin() { return stopOpeningEndPin; },
get opener() { return opener; },
get centre() { return centre; },
get scope() { return scope; },
get loadPager() { return loadPager; },
get jumpToLatest() { return jumpToLatest; }
  });
  const {transcriptEntries, transcriptSnapshot, verb, verbNow} = createTranscriptView({
get SESS() { return SESS; },
get TURNS() { return TURNS; },
get TX() { return TX; },
get find() { return find; }, set find(value) { find = value; },
get show() { return show; }, set show(value) { show = value; },
get I() { return I; },
get turnEnd() { return turnEnd; },
get STATE() { return STATE; },
get clock() { return clock; },
get sentenceSnapshot() { return sentenceSnapshot; },
get harnessSnapshot() { return harnessSnapshot; },
get running() { return running; },
get NOW() { return NOW; }, set NOW(value) { NOW = value; },
get isGap() { return isGap; },
get attachmentSnapshot() { return attachmentSnapshot; },
get HID() { return HID; },
get childSnapshot() { return childSnapshot; },
get answersOf() { return answersOf; },
get TXM() { return TXM; },
get originHandoff() { return originHandoff; },
get pagerSnapshot() { return pagerSnapshot; },
get MACHINE() { return MACHINE; },
get showsFooter() { return showsFooter; },
get footerSnapshot() { return footerSnapshot; }
  });
  // The whole tool call. A phone gets a full-screen sheet and a wider screen a dialog; either way it is a history entry,
  // so the back gesture closes it without leaving the page.
  let viewerEl: HTMLDialogElement | null = null; let skipPop = false;
  // What a step shows opened: what was asked first (the command, the file, the input), then what came back. A failed command
  // shows the end of its output, where the failure is; anything else shows the start. "View all" opens the whole call.
  // The whole call, in a full sheet: what the preview cut, fetched from the server when it is longer than the preview.
  function openStepViewer(e: ToolEntry, v: string, ic: string, inLabel: string = "Input") {
    if (disposed) return;
    if (e.more?.length && e.slot != null && !e.full) { const open = (f: Partial<Omit<ToolData, 'bg'>>) => openStepViewer({ ...e, ...f, full: true }, v, ic, inLabel); fullOf(e).then(open, () => open({ fullFailed: true })); return; }
    const status = e.live ? "Running · " + e.secs : e.unfinished ? "No result" : e.ok ? "Done · " + e.secs : e.ok === null ? "Exit unknown · " + e.secs : "Failed · " + e.secs;
    const { body, show: open } = panel(v + " " + e.arg, { cls: "full", sub: e.name + " · " + status, label: v + " " + e.arg });
    body.classList.add("viewer-b");
    renderFullTool(body, e, inLabel, SESS[e.sid ?? ""]?.state === "wait");
    open();
  }

  // Images a prompt attached: thumbnails above its text, each opening the image whole in the viewer sheet. The page never holds
  // their bytes: /api/tx names each image by its line, block and content version, /api/attachment serves it, and the <img> is
  // made here with its src set to that URL. Its box is sized from the width and height /api/tx read from the image's header
  // (at most 200×160, its shape kept), so nothing moves when it loads; one whose size isn't known gets a fixed box. An image the
  // logs don't hold (a path, a redacted copy) is a quiet chip, and so is one that fails to load.
  const THUMB_W = 200, THUMB_H = 160;
  const IMAGE_KIND: Record<string,string> = { "image/png": "PNG", "image/jpeg": "JPEG", "image/gif": "GIF", "image/webp": "WebP" };
  const sizeText = (n: number) => n >= 1048576 ? (n / 1048576).toFixed(1) + " MB" : n >= 1024 ? Math.round(n / 1024) + " KB" : n + " bytes";
  function attachmentSnapshot(e: Entry): Attachment[] {
    const images = (e.img ?? []).map((a, i) => {
      const k = a.w != null && a.h != null && a.w > 0 && a.h > 0 ? Math.min(1, THUMB_W / a.w, THUMB_H / a.h) : null;
      return { unavailable: !!a.na, url: "/api/attachment?sid=" + enc(e.sid ?? "") + "&o=" + enc(a.o) + "&b=" + enc(a.b) + "&v=" + enc(a.v ?? ""),
        label: "Attached image " + (i + 1) + " (" + (IMAGE_KIND[a.type ?? ""] ?? "image") + ", " + sizeText(a.size ?? 0) + ")",
        width: k ? Math.max(1, Math.round(a.w! * k)) : undefined, height: k ? Math.max(1, Math.round(a.h! * k)) : undefined };
    });
    return images;
  }
  function openImage(url: string, label: string, from: HTMLButtonElement) {
    const image = createImageViewer(url, label, {
      opened(d) { viewerEl = d; document.documentElement.classList.add("viewer-open"); try { history.pushState({ ...navigation.route, sheet: 1 }, ""); } catch {} },
      closed(d) { dialogs.delete(d); if (disposed) return;
        document.documentElement.classList.remove("viewer-open");
        if (viewerEl === d) { viewerEl = null; if (history.state?.sheet) { skipPop = true; history.back(); } }
        if (LIVE.pending) refresh();
        (from.isConnected ? from : [...document.querySelectorAll<HTMLElement>("button.attach")].find((x) => x.querySelector("img")?.getAttribute("src") === url))?.focus();
      },
    });
    dialogs.set(image.dialog, image); image.show();
  }

  function openScript(e: ToolEntry) {
    const done = (fields: Partial<Omit<ToolData, 'bg'>>) => openStepViewer({ ...e, ...fields, full: true }, "View script", "run", "Script");
    api("/api/entry?sid=" + enc(e.sid ?? "") + "&slot=" + e.slot + "&as=script")
      .then((value) => { const result = object(value); done({ scriptText: optional(result.text, text), scriptTruncated: result.truncated === true }); })
      .catch(() => done({ scriptFailed: true }));
  }

  // A brief or message clamped to three lines; "Show more" opens it in place, and only appears when it is cut.
  function childSnapshot(h: Handoff, c: Session): ChildView {
    const calls = countOf(c, "calls"), holder = HOLDS.get(h.id);
    return { kind: "child", handoff: h.id, id: c.id, turn: STARTS.get(h.id)?.id, name: c.name, state: c.state, stateLabel: STATE[c.state] ?? c.state,
      meta: [kindText(c), shortModel(c.model), dur(c.start, c.state === "work" ? null : c.last), (calls ?? "—") + (calls === 1 ? " step" : " steps"), costText(costForSession(c.id, true))].join(" · "), mark: harnessSnapshot(c.harness), brief: preview(h.brief ?? ""), result: h.result || undefined, failed: h.status === "err", activity: !h.result && c.state === "work" && c.activity ? [verbNow(c.activity[0]), c.activity[1]] : undefined, trace: holder?.id, traceLabel: holder ? "Open run view for " + nameOf(h.from) : undefined, chevron: I.chev };
  }
  // The tips on a session footer's items, as plain text (the tooltip sets it with textContent): calls by tool, times, cost by kind.
  const callsTip = (s: Session) => { const parts = Object.entries(s.tool_calls ?? {}).sort((a, b) => b[1] - a[1] || (a[0] < b[0] ? -1 : 1)).map(([name, n]) => name + " " + n), failed = countOf(s, "errors"); if (failed) parts.push(failed + " failed"); return parts.join(" · "); };
  const timeTip = (s: Session, h: Handoff | undefined, finished: boolean) => "Started " + clock(s.start) + " · last activity " + clock(s.last) + (finished ? " · finished " + clock(h?.done ?? s.last) : "");
  function costTip(cost: Cost) {
    const groups = Object.entries(cost.by_model ?? {}).map(([id, m]) => { const k = m.usd_by_kind ?? {}, kinds = [["Input", k.input], ["Output", k.output], ["Cache read", k.cache_read], ["Cache write", (Number(k.cache_write_5m) || 0) + (Number(k.cache_write_1h) || 0)]]; if (Number(k.web_search) >= 0.005) kinds.push(["Web search", k.web_search]); return { id, text: kinds.map(([label, usd]) => label + " " + asMoney(Number(usd) || 0)).join(" · ") }; });
    return (groups.length > 1 ? groups.map((g) => g.id + ": " + g.text).join("; ") : groups.map((g) => g.text).join("")) + (groups.length ? ". " : "") + COST_TIP;
  }
  // What a footer shows: its text, its tips, its state, and whether it has the button to the parent.
  // The line a session page ends in, for a child (h: its origin) and any other session alike: the state, the calls, the time and
  // the API-equivalent cost, each with its breakdown as a tip. A returned child keeps "Returned to <parent>" and its button.
  function footerSnapshot(s: Session, h: Handoff | undefined): FooterView {
    const done = s.state === "done" || s.state === "err", finished = h ? done || h.status === "done" || h.status === "err" : done;
    const state = h ? finished ? s.state === "err" || h.status === "err" ? "err" : "done" : "work" : s.state;
    const cost = costForSession(s.id), priced = cost.usd != null && !costMissing(cost).length && cost.usd >= 0.005, items: FooterView["items"][number][] = [];
    if (!h || !finished) items.push({ kind: "calls", text: callsText(countOf(s, "calls")), tip: callsTip(s) });
    items.push({ kind: "time", text: dur(s.start, state === "work" ? null : s.last), tip: timeTip(s, h, finished) });
    if (priced) items.push({ kind: "cost", text: asMoney(cost.usd ?? 0), tip: costTip(cost) });
    return { state, stateLabel: STATE[state] ?? state, text: h && finished ? "Returned to " + nameOf(h.from) + " · " + STATE[state] : STATE[state], items, parent: h && finished ? { id: h.from, turn: HOLDS.get(h.id)?.id, name: nameOf(h.from) } : undefined };
  }

  // ---- Render --------------------------------------------------------------------------------------------------------
  // Persistent controls. A control the reader may be operating (a filter, the search field, a toggle group) is built once for
  // the page shown and kept by every redraw of that page: it is never taken out of the document, so its focus, an open
  // dropdown and the caret in a text field survive. `slot` finds or builds it in its box; the caller then hands it the
  // page's current data and handlers through `ctx` (a handler is read from `ctx` when the control fires, so it never calls
  // the closure of an earlier draw). A navigation to another route drops every slot.
  const SLOTS = new Map<string,ControlSlot>();
  function slot(key: string, box: HTMLElement, build: (ctx: SlotContext) => HTMLElement) {
    let s = SLOTS.get(key);
    if (!s || s.route !== navigation.route || s.box !== box || !box.contains(s.el)) { s?.ctx.destroy?.(); const ctx: SlotContext = {sync() {}, onChange() {}}; s = { route: navigation.route, box, ctx, el: build(ctx) }; SLOTS.set(key, s); }
    return s;
  }
  // Empties `box` of everything but the slots the route being drawn already holds in it.
  function clearBox(box: HTMLElement, r: ApplicationRoute | null = null) {
    for (const [key, s] of SLOTS) if (s.route !== r) { s.ctx.destroy?.(); SLOTS.delete(key); }
    if (ownsScreen(box)) { if (screenKind(box) === r?.v && !(r?.v === "machines" && viewerHost) && !NATIVE_PAGE) return; releaseScreen(box); }
    const kept = new Set([...SLOTS.values()].filter((s) => s.box === box).map((s) => s.el));
    for (const n of [...box.childNodes]) if (!(n instanceof HTMLElement && kept.has(n))) n.remove();
  }
  // Adds nodes to `box` in order, after `clearBox`: a node already at the next place stays where it is, and any other
  // goes in before it. `put.done()` drops what is left over.
  function placer(box: HTMLElement) {
    let cur = box.firstChild;
    const put = (...nodes: Node[]) => { for (const n of nodes) { if (n === cur) cur = cur?.nextSibling ?? null; else box.insertBefore(n, cur); } };
    put.done = () => { while (cur) { const next = cur.nextSibling; cur.remove(); cur = next; } };
    return put;
  }
  function render() {
    if (disposed) return;

    const focusSearch = focusSessionsSearchOnRender === navigation.route; focusSessionsSearchOnRender = null;
    if (SIDEBAR_ONLY) { domain.invalidate(); tick(); navigation.rendered = navigation.route; renderNav(); renderLanes(); return; } // the embedding page draws its own page and bar
    if (NATIVE_PAGE || (navigation.route.v === "machines" && viewerHost)) {
      tick(); navigation.rendered = navigation.route;
      if (navigation.content && !navigation.content.element.isConnected) { clearBox($("#page"), navigation.route); $("#page").append(navigation.content.element); }
      document.title = (NATIVE_PAGE?.title ?? "Machines") + " · Semon";
      renderTopbar(NATIVE_PAGE?.title ?? "Machines"); syncLayoutPrefs(); syncBarLine(); renderNav(); renderLanes(); renderDrawerAccount(); syncJump(); return;
    }
    // The page first, then the bar: the bar's summary (a trace's counts, a search's matches) comes from the page.
    if (viewerHost) document.title = ({ home: "Home", sessions: "Sessions", analytics: "Analytics", machines:"Machines", machine:"Machine", session:"Session", trace:"Trace" }[navigation.route.v] ?? "Semon") + " · Semon";
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
    const lanesKept = destination.lanesFor && destination.lanesFor.r === r && destination.lanesFor.version === LIVE.version; destination.lanesFor = null;
    syncLayoutPrefs(); syncBarLine(); renderNav(); if (!lanesKept) renderLanes(); renderDrawerAccount(); syncJump(); ordPageState = null;
  }

  // ---- Analytics: the server computes each range (/api/analytics) -----------------------------------------------------------
  // The model holds only its own window (a day), so the page asks the server for the range it shows: 24 h, 7 d or 30 d, with
  // the filters. The answer has every figure, chart column and list the page draws, and the page does no range math. It is
  // asked for when the page opens, when the range or a filter changes, after every model update (the server answers 304
  // while nothing changed), and every 10 s while the page shows, as time moves the range. Answers are kept per range and
  // filters, so switching back draws at once while the page asks again.
  const {fetchAnalytics, scheduleAnalytics, refreshAnalytics, renderAnalytics, matchesSessionFacets, renderFacetFilters} = createAnalytics({
get dialogs() { return dialogs; },
get analyticsRange() { return analyticsRange; }, set analyticsRange(value) { analyticsRange = value; },
get sessionFilters() { return sessionFilters; },
get enc() { return enc; },
get scope() { return scope; },
get disposed() { return disposed; }, set disposed(value) { disposed = value; },
get ended() { return ended; },
get navigation() { return navigation; },
get LIVE() { return LIVE; },
get visible() { return visible; },
get viewerEl() { return viewerEl; }, set viewerEl(value) { viewerEl = value; },
get accountChrome() { return accountChrome; },
get capture() { return capture; },
get render() { return render; },
get restore() { return restore; },
get SIDEBAR_ONLY() { return SIDEBAR_ONLY; },
get SESS() { return SESS; },
get MACHINE() { return MACHINE; },
get HARNESS() { return HARNESS; },
get slot() { return slot; },
get currentScroll() { return currentScroll; },
get skipPop() { return skipPop; }, set skipPop(value) { skipPop = value; },
get refresh() { return refresh; },
get $() { return $; },
get panel() { return panel; },
get asMoney() { return asMoney; },
get TURNS() { return TURNS; },
get pendingSessionOpen() { return pendingSessionOpen; }, set pendingSessionOpen(value) { pendingSessionOpen = value; },
get afterPop() { return afterPop; }, set afterPop(value) { afterPop = value; },
get goTrace() { return goTrace; },
get goSession() { return goSession; },
get HARNESS_SHORT() { return HARNESS_SHORT; },
get harnessSnapshot() { return harnessSnapshot; },
get urlOf() { return urlOf; },
get COST_TIP() { return COST_TIP; },
get clock() { return clock; },
get analyticsMeasure() { return analyticsMeasure; }, set analyticsMeasure(value) { analyticsMeasure = value; },
get observeTitle() { return observeTitle; },
get restoreScroll() { return restoreScroll; },
get query() { return query; }, set query(value) { query = value; },
get groupBy() { return groupBy; }, set groupBy(value) { groupBy = value; },
get go() { return go; }
  });
  // ---- Sessions: every top-level session and its child runs ---------------------------------------------------------------
  const laneOf = (sid: string) => { const seen = new Set<string>(); while (parentOf(sid) && !seen.has(sid)) { seen.add(sid); sid = parentOf(sid)!; } return sid; };
  function childRuns(sid: string) {
    const kids = Object.values(SESS).filter((x) => x.id !== sid && laneOf(x.id) === sid && (showApprovalReviews || !isApprovalReview(x))), sub = kids.filter((x) => x.kind === "Subagent").length, cdx = kids.filter((x) => x.kind === "Codex run").length, other = kids.length - sub - cdx;
    return [sub ? sub + (sub === 1 ? " subagent" : " subagents") : null, cdx ? cdx + (cdx === 1 ? " Codex run" : " Codex runs") : null, other ? other + (other === 1 ? " other run" : " other runs") : null].filter(Boolean).join(" · ");
  }
  function renderSessions(page: HTMLElement, focusSearch: boolean = false) {
    const all = Object.values(SESS).filter((s) => (showApprovalReviews || !isApprovalReview(s)) && matchesSessionFacets(s));
    const lanes = all.filter((s) => sessMatch(s, query)), order = orderScope("page", pageSig(), navigation.route, pageState());
    let groups: [string,Session[],number][];
    if (groupBy === "recent") groups = [["", orderList(order, "recent", lanes, byLast), lanes.length]];
    else { const keysOf: Record<string,(s: Session) => string> = { machine: (s) => MACHINE[s.machine], project: (s) => s.repo ?? "No repo (roles)", harness: (s) => HARNESS[s.harness] }; const key = keysOf[groupBy] ?? ((s: Session) => s.name);
      const keys = [...new Set(lanes.map(key))].sort((a, b) => Number(a.startsWith("No repo")) - Number(b.startsWith("No repo")) || a.localeCompare(b));
      groups = keys.map((k) => { const rows = lanes.filter((s) => key(s) === k); return [k, orderList(order, "g:" + k, rows, byLast), rows.length]; }); }
    const rows = groups.filter(([, items, total]) => items.length || !total).map(([title, items, total]) => ({ title, total,
      harness: groupBy === "harness" ? harnessSnapshot(Object.keys(HARNESS).find((k) => HARNESS[k] === title) ?? "") : undefined,
      rows: items.map((s) => {
        const fields: {className: string; text: string; priority: number; tip?: string; icon?: string}[] = [{ className: "row-duration", text: dur(s.start, s.state === "work" || s.state === "wait" ? null : s.last), priority: 1, tip: "Duration", icon: I.duration }];
        if (Object.keys(MACHINE).length > 1) fields.push({ className: "row-machine host", text: shortHost(s), priority: 2, tip: "Machine: " + hostOf(s), icon: I.machine });
        if (s.repo) fields.push({ className: "repo-short", text: s.repo, priority: 3, tip: "Repo: " + s.repo, icon: I.repo });
        const counts = childRuns(s.id); if (counts) fields.push({ className: "row-counts", text: counts.replace(/Codex runs/g, "runs"), priority: 4 });
        return { id: s.id, name: s.name, state: s.state, stateLabel: STATE[s.state] ?? s.state, age: ago(s.last), harness: harnessSnapshot(s.harness),
          model: shortModel(s.model ?? s.modelId), modelTip: "Model: " + modelIdOf(s), delegation: parentOf(s.id) ? I.stack : undefined, fields };
      }) }));
    renderSessionsScreen(page, { total: all.length, working: all.filter((s) => s.state === "work").length, waiting: all.filter((s) => s.state === "wait").length,
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
  function closeDrawer(quiet: boolean | undefined = undefined) { shellChrome?.closeDrawer(quiet); }
  // Sidebar-only consumers retain their existing server shell.js owner.
  if (SIDEBAR_ONLY) scope.listen(window, "semon:drawer-open", () => orderApply("side"));
    if (!SIDEBAR_ONLY) scope.listen(document, "keydown", (e) => { if (e.key === "Escape" && accountSheet) accountChrome.escape(); else if (e.key === "Escape" && !viewerEl) { closeDrawer(); closeAccountMenu(); $(".session-menu")?.remove(); $("#more-btn")?.setAttribute("aria-expanded", "false"); } if (e.key === "/" && !/INPUT|TEXTAREA/.test(document.activeElement?.tagName ?? "") && !(document.activeElement instanceof HTMLElement && document.activeElement.isContentEditable) && !viewerEl) { e.preventDefault(); if (navigation.route.v === "session") { findOpen = true; render(); $("#find")?.focus(); } else if (navigation.route.v === "sessions") { $("#sq")?.focus(); } else { const r: ApplicationRoute = { v: "sessions", q: query }; focusSessionsSearchOnRender = r; go(r); } } });
  scope.listen(phone, "change", () => {
    closeDrawer(true); syncLayoutPrefs(); recentNavigation.expandedAll = null; renderLanes();
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
      const response = await api("/api/model?delta=1&since=" + enc(LIVE.late ? "" : LIVE.version ?? ""), undefined, true);
      if (!response) return;
      let model; try { model = applyModelDelta(response); } catch { await api("/api/model?delta=1").then(update); return; }
      await update(model);
    },
    failed(error) { return !!viewerHost?.modelFailed?.(error instanceof ApiError ? error.status : 0); }, ended,
  });
  const LIVE = liveController.state;
  // The turn index as last drawn, to tell which turns an update changed.
  const remember = (_m: ModelWire) => { LIVE.turns = new Map(Object.values(TURNS).flat().map((x) => [x.id, turnKey(x)])); };
  // What an update can change in a turn record or a handoff, cheaply (not the text, which a record never rewrites).
  const turnKey = (x: Turn) => [x.start?.id, x.end?.st, x.end?.why, x.end?.h, x.sent.map(h => h.id).join(","), x.last ? 1 : 0].join("|");
  const handKey = (h: Handoff) => [h.status, h.to, h.done, h.result?.length, h.kind === "toyou" ? h.answer?.length : undefined, h.kind === "toyou" ? h.answers?.length : undefined, h.declined ? 1 : 0].join("|");
  // rendered route is owned by navigation; // the route the page shows
  const visible = () => document.visibilityState === "visible";
  const schedule = (ms: number) => liveController.schedule(ms);
  // An embedding page can cancel `semon:ended` to draw its own note in place of this one.
  function ended(status: number) {
    if (disposed) return;
    liveController.stop(); if ($(".livenote, .livenote-side")) return;
    if (!window.dispatchEvent(new CustomEvent("semon:ended", { cancelable: true, detail: { status } }))) return;
    const n = createStatusNote(SIDEBAR_ONLY ? "Sessions stopped updating: reload the page" : "Session ended: reload with the printed URL", SIDEBAR_ONLY ? "ghead livenote-side" : "livenote");
    if (SIDEBAR_ONLY) $("#lanes").after(n); else document.body.append(n); // on an embedding page, under the list that stopped
  }
  // A 403 or a dropped connection fails the update (and backs off); anything else skips that one transcript.
  const soft = <T>(p: Promise<T>) => p.catch((e: unknown) => { if (e instanceof ApiError && (e.status === 403 || e.status === 0)) throw e; });
  // The transcript on screen: a session page's own. Any other loaded transcript is dropped from TX (the last few opened are kept in
  // TXCACHE, and brought up to date when opened again). A child run's card is drawn from the model, so its transcript is not loaded.
  const viewed = () => { const v = new Set<string>(); if (navigation.route.v === "session") v.add(("id" in navigation.route ? navigation.route.id : "")); return v; };
  // What a child card shows, so an update knows which cards changed: the run's name, state, kind, model, steps and current call.
  const cardKeys = () => new Map(H.filter((h) => h.kind === "spawn" && SESS[h.to]).map((h) => { const c = SESS[h.to]; return [h.id, [c.name, c.state, c.kind, c.model, countOf(c, "calls"), c.activity?.join("|"), h.status, h.result].join("\u0001")]; }));
  const {reload, tail, applyModelDelta, update} = createLiveUpdates({
get modelStore() { return modelStore; },
get H() { return H; },
get handKey() { return handKey; },
get LIVE() { return LIVE; },
get cardKeys() { return cardKeys; },
get SESS() { return SESS; },
get TXCACHE() { return TXCACHE; },
get originHandoff() { return originHandoff; },
get navigation() { return navigation; },
get adopt() { return adopt; },
get remember() { return remember; },
get STALE_BRIEFS() { return STALE_BRIEFS; },
get TX() { return TX; },
get viewed() { return viewed; },
get spread() { return spread; },
get dropTx() { return dropTx; },
get soft() { return soft; },
get TXM() { return TXM; },
get TOK() { return TOK; }, set TOK(value) { TOK = value; },
get shrank() { return shrank; },
get refresh() { return refresh; },
get refreshAnalytics() { return refreshAnalytics; },
get errorsLive() { return errorsLive; },
get TURNS() { return TURNS; },
get isGap() { return isGap; },
get HOLDS() { return HOLDS; },
get api() { return api; },
get enc() { return enc; },
get txEntry() { return txEntry; },
get fetchTx() { return fetchTx; }
  });
  // Draws the new model on the screen shown, unless a navigation is still loading (it draws when done), the sheet is open
  // (it draws when the sheet closes), or what the screen shows left the model (it stays as it was).
  function refresh(dirty: ReadonlySet<string> | null = null) {
    if (disposed) return;
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
  const viewport = createViewport({
get phone() { return phone; },
get $() { return $; },
get queuePagerObservers() { return queuePagerObservers; },
get navigation() { return navigation; },
get scrollProgrammatically() { return scrollProgrammatically; },
get saveHistoryScroll() { return saveHistoryScroll; },
get resetPagerInput() { return resetPagerInput; },
get disconnectPagerObservers() { return disconnectPagerObservers; },
get scope() { return scope; },
get viewerEl() { return viewerEl; }, set viewerEl(value) { viewerEl = value; },
get scrollRevision() { return scrollRevision; }, set scrollRevision(value) { scrollRevision = value; },
get syncBarLine() { return syncBarLine; },
get shellChrome() { return shellChrome; },
get holdProgrammaticScroll() { return holdProgrammaticScroll; },
get tick() { return tick; },
get SESS() { return SESS; },
get TURN() { return TURN; },
get renderSession() { return renderSession; },
get drawSessionBar() { return drawSessionBar; },
get paintPager() { return paintPager; },
get renderNav() { return renderNav; },
get renderLanes() { return renderLanes; },
get ticker() { return ticker; },
get LIVE() { return LIVE; },
get TXM() { return TXM; },
get fetchTx() { return fetchTx; },
get goSession() { return goSession; },
get SIDEBAR_ONLY() { return SIDEBAR_ONLY; },
get readerScrollInput() { return readerScrollInput; },
get programmaticScrollPending() { return programmaticScrollPending; }, set programmaticScrollPending(value) { programmaticScrollPending = value; }
  });
  const {scrollController, stopOpeningEndPin, scroller, capture, restore, syncJump, startOpeningEndPin, clearNewEntries, edge, opener, jumpToLatest, patchSession} = viewport;
  // Every second: each running step uses its own start; a group shows its oldest running call.
  // A clock that stands still (the checks pin it) changes nothing.
  const running = (ms: number) => { const x = Math.max(0, Math.floor(ms / 1000)); return x < 60 ? x + "s" : Math.floor(x / 60) + "m " + (x % 60) + "s"; };
  function ticker() {
    if (!visible() || Date.now() === fetchedAt) return;
    tick();
    if (navigation.route.v === "session" && navigation.rendered === navigation.route) updateSessionClock($("#page"), NOW, Object.fromEntries(Object.values(SESS).filter((s) => s.activity?.[3] != null).map((s) => [s.id, s.activity![3]!])));
    else if (navigation.route.v === "home" && navigation.rendered === navigation.route) renderHome($("#page"));
  }

  // An embedding page's sidebar: the row its data-viewer-nav names (home, sessions or machines) is current.
  if (SIDEBAR_ONLY) { const nav = app.dataset.viewerNav; navigation.route = navigation.historyRoute({ v: nav }, {v: "home"}); }
  if (viewerHost) {
    ACCOUNT = accountOf(viewerHost.account);
    if (NATIVE_PAGE) navigation.route = navigation.historyRoute({ v: NATIVE_PAGE.nav }, {v: 'home'});
    else if (navigation.content) navigation.route = { v: "machines" };
    if (NATIVE_PAGE || navigation.content) render();
  }
  boot();
  return application;
}
