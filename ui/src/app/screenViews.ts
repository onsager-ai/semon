import { compactCount,machineShorts,preview } from "../domain/format";
import { createTraceCalculations } from "../domain/trace";
import type { Entry,Handoff,Session,TokenKind,Turn } from '../domain/types';
import type { HarnessMark,InboxRow,LiveRow,MenuRun,SentencePart,SessionMenuSnapshot } from '../lib';
import { renderHomeScreen,renderMachineScreen,renderMachinesScreen,renderSessionScreen,renderTraceScreen } from "../lib";
import type { Hop } from '../lib/trace';
type ToolEntry = Extract<Entry, {k: 'tool'}> & {full?: boolean; scriptLoaded?: boolean};
interface ScreenViewsHost {
  costForSession: (sid: string, includeRuns?: boolean) => Required<import("../domain/types").Cost>;
  costMissing: (cost: import("../domain/types").Cost) => string[];
  costText: (cost: import("../domain/types").Cost) => string;
  costForSessions: (sessions: Iterable<import("../domain/types").Session>) => Required<import("../domain/types").Cost>;
  HARNESS: { [k: string]: string; };
  asMoney: (usd: number) => string;
  sessionChildren: () => Map<string, import("../domain/types").Session[]>;
  kindText: (s: import("../domain/types").Session) => string;
  STATE: Record<string, string>;
  MACHINE: Record<string, string>;
  MACHINE_UP: Record<string, boolean>;
  HARNESSES: Record<string, { name: string; short: string; icon: { light: string; dark: string; }; }>;
  darkTheme: () => boolean;
  TURNS: Record<string, import("../domain/types").Turn[]>;
  H: import("../domain/types").Handoff[];
  ago: (t: number) => string;
  nameOf: (id: string) => string;
  oneLine: (s: string) => string;
  HOLDS: Map<string, import("../domain/types").Turn>;
  SESS: Record<string, import("../domain/types").Session>;
  I: Record<string, string>;
  answersOf: (h: import("../domain/types").Handoff) => string[] | null;
  traceRoot: (t: import("../domain/types").Turn) => import("../domain/types").Turn;
  goSession: (id: string, turn?: string | undefined) => void;
  observeTitle: () => void;
  goTrace: (turn: string) => void;
  inbox: () => import("../domain/types").Handoff[];
  isResult: (h: import("../domain/types").Handoff) => boolean;
  markSeenResults: (handoffs: Iterable<import("../domain/types").Handoff>) => void;
  render: () => void;
  working: () => import("../domain/types").Session[];
  orderList: <Row extends { id: string; }, Tie extends object>(scope: import("../lib/ordering").OrderScope<Tie>, key: string, items: readonly Row[], compare: (a: Row, b: Row) => number, { must, limit, quiet, seed }?: { must?: ReadonlySet<string> | null | undefined; limit?: number | undefined; quiet?: boolean | undefined; seed?: boolean | undefined; }) => Row[];
  orderScope: (name: string, sig: string, tie: import("../navigation/routes").ApplicationRoute | null, state: { inView: boolean; touched: boolean; }) => import("../lib/ordering").OrderScope<import("../navigation/routes").ApplicationRoute>;
  pageSig: () => string;
  navigation: import("../navigation/routes").NavigationController;
  pageState: () => { inView: boolean; touched: boolean; };
  byLast: (a: import("../domain/types").Session, b: import("../domain/types").Session) => number;
  onMachine: (m: string) => import("../domain/types").Session[];
  movedOff: (m: string) => import("../domain/types").Session[];
  movesOf: (m: string) => import("../domain/types").Handoff[];
  clock: (t: number) => string;
  MACHINE_LAST: Record<string, number>;
  ADMIN: { href: string; label: string; } | null;
  go: (r: import("../navigation/routes").ApplicationRoute, fromHistory?: boolean, prepared?: boolean, nextContent?: import("../viewer-host").ViewerContent | null) => void;
  byState: (a: import("../domain/types").Session, b: import("../domain/types").Session) => number;
  STARTS: Map<string, import("../domain/types").Turn>;
  domain: import("../domain/calculations").DomainController;
  NOW: number;
  TURN: Map<string, import("../domain/types").Turn>;
  sentenceHost: import("../lib/sentence").SentenceHost;
  machineLabel: (s: import("../domain/types").Session | null | undefined, scope?: Iterable<string | import("../domain/types").Session> | undefined) => string;
  hcls: (id: string) => string;
  hostOf: (s: import("../domain/types").Session) => string;
  sentenceSnapshot: (h: import("../domain/types").Handoff, viewer: string | null | undefined, links?: boolean) => import("../lib/sentence").SentenceSnapshot;
  turnEnd: (t: import("../domain/types").Turn) => { st: import("../domain/types").SessionState; text: string; } | null;
  statWord: (h: import("../domain/types").Handoff) => string | undefined;
  SEEN_RESULTS: Set<string>;
  machineLabels: (scope?: Iterable<string | import("../domain/types").Session>) => Map<string, string>;
  transcriptEntries: (entries: import("../domain/types").Entry[], sid: string) => import("../domain/types").Entry[];
  TX: Record<string, import("../domain/types").Entry[]>;
  transcriptSnapshot: (sid: string, opts?: { only?: ReadonlySet<string> | undefined; }) => import("../lib/transcript").SessionSnapshot;
  verb: (name: string) => string[];
  openStepViewer: (e: ToolEntry, v: string, ic: string, inLabel?: string) => void;
  openScript: (e: ToolEntry) => void;
  openImage: (url: string, label: string, from: HTMLButtonElement) => void;
  stopOpeningEndPin: () => void;
  opener: (n: HTMLElement) => HTMLButtonElement | null;
  centre: (node: HTMLElement) => void;
  scope: import("../app/effects").EffectScope;
  loadPager: (button: HTMLButtonElement, manual: boolean) => Promise<void>;
  jumpToLatest: () => void;
}
/** Owns screenViews behavior through explicit application ports. */
export function createScreenViews(host: ScreenViewsHost) {
  const MENU_KINDS: [string, TokenKind[]][] = [["Input", ["input"]], ["Output", ["output"]], ["Cache write", ["cache_write_5m", "cache_write_1h"]], ["Cache read", ["cache_read"]], ["Web search", ["web_search"]]];
  const COST_NOTE = "What these tokens would cost at API rates. Subscriptions aren't billed this way.";
  function costSnapshot(s: Session, kids: Session[]): SessionMenuSnapshot["cost"] {
    const own = host.costForSession(s.id), all = host.costForSession(s.id, true), missing = host.costMissing(all), details = [];
    if (kids.length) details.push({ label: "This session", value: host.costText(own) }, { label: kids.length === 1 ? "Its run" : "Its " + kids.length + " runs", value: host.costText(host.costForSessions(kids)) });
    const reports = s.reported_runs ?? [], reported = reports.filter((r) => Number.isFinite(r.cost_usd));
    if (reports.length) details.push({ label: host.HARNESS[s.harness] + "'s own figure", value: reported.length ? host.asMoney(reported.reduce((n, r) => n + (r.cost_usd ?? 0), 0)) + (reported.length === 1 ? ", last run" : ", last " + reported.length + " runs") : "not reported" });
    const check = [...(s.cost_check ?? [])].reverse().find((c) => c.ok === false && Number.isFinite(c.computed_usd) && Number.isFinite(c.reported_usd));
    const mismatch = check ? "Semon's estimate for that run is " + (check.reported_usd === 0 ? 100 : Math.round(Math.abs((check.computed_usd ?? 0) - (check.reported_usd ?? 0)) / Math.abs(check.reported_usd ?? 0) * 100)) + "% " + ((check.computed_usd ?? 0) > (check.reported_usd ?? 0) ? "above" : "below") + " " + host.HARNESS[s.harness] + "'s figure: API rates differ from what a plan is charged." : undefined;
    const children = host.sessionChildren(), runs: MenuRun[] = [], walk = (id: string, depth: number) => { for (const c of [...(children.get(id) ?? [])].sort((a, b) => b.last - a.last)) { const cost = host.costText(host.costForSession(c.id)); runs.push({ id: c.id, depth, label: "Open " + c.name + ", " + host.kindText(c) + ", " + host.STATE[c.state] + ", " + cost, name: c.name, kind: host.kindText(c), state: c.state, stateLabel: host.STATE[c.state], cost }); walk(c.id, depth + 1); } }; if (kids.length) walk(s.id, 0);
    const models = Object.entries(all.by_model ?? {}).map(([id, model]) => {
      const priced = model.usd != null && !missing.includes(id), rows = [];
      for (const [label, keys] of MENU_KINDS) { const tokens = keys.reduce((n, k) => n + (Number(model.tokens?.[k]) || 0), 0), usd = keys.reduce((n, k) => n + (Number(model.usd_by_kind?.[k]) || 0), 0); if (tokens === 0 && (!priced || usd < 0.005)) continue; rows.push({ label, count: tokens ? compactCount(tokens) : "", exact: tokens ? tokens.toLocaleString() : undefined, cost: priced ? host.asMoney(usd) : "—" }); }
      return { id, rows };
    });
    return { figure: host.costText(kids.length ? all : own), caption: kids.length ? "this session and its " + kids.length + (kids.length === 1 ? " run" : " runs") : "this session",
      note: COST_NOTE + (missing.length ? " No price for " + missing.join(", ") + "." : ""), details, mismatch, runs, models, includesRuns: !!kids.length };
  }

  // ---- Home: what needs you, then what is running ------------------------------------------------------
  const upCount = () => Object.keys(host.MACHINE).filter((m) => host.MACHINE_UP[m]).length;
  let allAnswered = false;
  function harnessSnapshot(id: string): HarnessMark | undefined {
    const h = Object.hasOwn(host.HARNESSES, id) ? host.HARNESSES[id] : null;
    return h ? { id, name: h.name, light: h.icon.light, dark: h.icon.dark, darkTheme: host.darkTheme() } : undefined;
  }
  function liveSnapshot(s: Session, showMachine: boolean): LiveRow {
    const cur = (host.TURNS[s.id] ?? []).at(-1), msg = cur?.start?.brief ?? cur?.u?.text;
    const inb = cur?.start ?? (cur?.u ? { from: "you" } : host.H.find((h) => h.to === s.id && h.kind !== "move"));
    return { id: s.id, name: s.name, state: s.state, stateLabel: host.STATE[s.state] ?? s.state,
      status: s.state === "work" ? host.HARNESS[s.harness] : host.ago(s.last), harness: s.state === "work" ? harnessSnapshot(s.harness) : undefined,
      detail: [showMachine ? host.MACHINE[s.machine] : null, inb ? (inb.from === "you" ? "for you" : "for " + host.nameOf(inb.from)) : null, msg ? host.oneLine(msg) : null].filter(Boolean).join(" · "),
      activity: s.state === "work" && s.activity ? [s.activity[0],s.activity[1],s.activity[2]] : undefined };
  }
  function inboxSnapshot(h: Handoff, quiet: boolean = false): InboxRow {
    const sid = h.kind === "move" ? h.to : h.from, t = host.HOLDS.get(h.id), s = host.SESS[sid];
    const parts: SentencePart[] = [], part = (className: string, text: string, tip?: string) => parts.push({ className, text, tip });
    let path = host.I.more;
    if (h.kind === "ask") { path = host.I.ask; part("who", host.nameOf("you")); part("verb", " asked "); part("who", host.nameOf(h.to)); }
    else if (h.kind === "spawn" || h.kind === "relay") { path = host.I.out; part("who", host.nameOf(h.from)); part("verb", h.kind === "spawn" ? " handed off to " + (host.SESS[h.to]?.kind === "Subagent" ? "subagent" : host.SESS[h.to]?.kind ?? "") + " " : " relayed to "); part("who", host.nameOf(h.to)); }
    else if (h.kind === "move") {
      path = host.I.move; const short = machineShorts([h.fromMachine, h.toMachine].map((id) => [id, host.MACHINE[id] ?? id]));
      part("verb", "Semon moved "); part("who", host.nameOf(h.to)); part("verb", " from "); part("verb mach", short.get(h.fromMachine) ?? h.fromMachine, "Machine: " + (host.MACHINE[h.fromMachine] ?? h.fromMachine)); part("verb", " to "); part("verb mach", short.get(h.toMachine) ?? h.toMachine, "Machine: " + (host.MACHINE[h.toMachine] ?? h.toMachine));
    } else if (h.kind === "toyou") {
      path = h.status === "done" && (h.ask === "question" || h.ask === "decision") ? host.I.done : h.ask === "question" ? host.I.qc : h.ask === "decision" ? host.I.decide : host.I.result;
      part("who", host.nameOf(h.from)); part("verb", { question: " asked you", result: " sent you a result", decision: " needs your decision" }[h.ask]);
    }
    const answer = quiet ? host.answersOf(h) : null, root = t ? host.traceRoot(t) : null, msg = root?.start?.from === "you" ? root.start.brief : root?.u?.text;
    return { id: h.id, quiet, icon: quiet && h.kind === "toyou" ? host.I.done : path, parts, age: host.ago(h.at), preview: preview(h.brief ?? ""),
      answer: answer ? answer.length ? "You answered: " + answer.join(" · ") : "Answered · reply not in these logs" : undefined,
      origin: root ? msg ? { message: host.oneLine(msg) } : { text: "Started by " + host.nameOf(root.start ? root.start.from : root.sid) } : undefined,
      context: [host.HARNESS[s.harness], host.MACHINE[s.machine]].join(" · "), harness: harnessSnapshot(s.harness), trace: t?.out.length ? t.id : undefined };
  }
  const activityHost = {
    session: host.goSession, committed: host.observeTitle, trace: host.goTrace,
    inbox(id: string) { const h = host.H.find((h) => h.id === id) ?? host.inbox().find((h) => h.id === id); if (!h) return; if (host.isResult(h)) host.markSeenResults([h]); host.goSession(h.kind === "move" ? h.to : h.from, host.HOLDS.get(h.id)?.id); },
    answered() { allAnswered = true; host.render(); },
  };
  function renderHome(page: HTMLElement) {
    const open = host.inbox(), running = host.working(), many = Object.keys(host.MACHINE).length > 1;
    const rows = host.orderList(host.orderScope("page", host.pageSig(), host.navigation.route, host.pageState()), "working", running, host.byLast);
    const done = host.H.filter((h) => h.kind === "toyou" && h.status === "done").sort((a, b) => b.at - a.at);
    renderHomeScreen(page, { waiting: open.length, working: running.length, up: upCount(), machines: Object.keys(host.MACHINE).length,
      inbox: open.map((h) => inboxSnapshot(h, false)), live: rows.map((s) => liveSnapshot(s, many)),
      answered: (allAnswered ? done : done.slice(0, 3)).map((h) => inboxSnapshot(h, true)), totalAnswered: done.length, allAnswered }, activityHost);
  }
  // ---- Machines: where sessions run, and what happens when a machine goes away ------------------------------
  function renderMachines(page: HTMLElement) {
    const ms = Object.keys(host.MACHINE).sort((a, b) => Number(host.MACHINE_UP[a]) - Number(host.MACHINE_UP[b]));
    const rows = ms.map((m) => {
      const here = host.onMachine(m), w = here.filter((s) => s.state === "work").length, up = host.MACHINE_UP[m], state = !up ? "err" : w ? "work" : "idle";
      const mv = host.movedOff(m).length, mh = host.movesOf(m).find((h) => h.kind === "move" && h.fromMachine === m);
      return { id: m, name: host.MACHINE[m], state, stateLabel: host.STATE[state] ?? state, status: !up ? "offline" : w ? "up" : "idle",
        detail: up ? [w + " working", here.length + (here.length === 1 ? " session" : " sessions")].join(" · ") : ["Not responding" + (mh ? " since " + host.clock(mh.at) : host.MACHINE_LAST[m] != null ? " since " + host.clock(host.MACHINE_LAST[m]) : ""), mv ? mv + (mv === 1 ? " session" : " sessions") + " moved off" : null].filter(Boolean).join(" · ") };
    });
    renderMachinesScreen(page, { rows, up: upCount(), working: host.working().length, admin: host.ADMIN }, { machine(id) { host.go({ v: "machine", id }); }, admin(href) { location.assign(href); }, committed: host.observeTitle });
  }
  function renderMachine(page: HTMLElement, m: string) {
    const here = host.onMachine(m), off = host.movedOff(m), moves = host.movesOf(m);
    const ordered = host.orderList(host.orderScope("page", host.pageSig(), host.navigation.route, host.pageState()), "machine:" + m, here, host.byState);
    renderMachineScreen(page, { name: host.MACHINE[m], totalSessions: here.length, sessions: ordered.map((s) => liveSnapshot(s, false)),
      off: off.map((s) => ({ ...liveSnapshot(s, false), detail: "Now on " + host.MACHINE[s.machine] })), moves: moves.map((h) => inboxSnapshot(h, true)) }, activityHost);
  }

  // ---- Trace: one turn and what it set off -------------------------------------------------------------------------
  // The root is the turn. Each spawn or relay it sent leads to the turn that handoff started in the receiving session,
  // and on down from there; a message to you is a leaf. One rail, as everywhere: depth shows as a smaller node.
  // One observer measures expandable messages, and releases detached nodes after a redraw.
  const RUN_EXPANDED = new Map<string,Set<string>>();
  const { agentSnapshot } = createTraceCalculations({ sessions: host.SESS, turns: host.TURNS, starts: host.STARTS }, host.domain, () => host.NOW, RUN_EXPANDED, host.HARNESS, host.STATE);
  function renderTrace(page: HTMLElement, id: string) {
    const root = host.TURN.get(id);
    if (!root) { renderTraceScreen(page, { empty: true, hops: [] }, { ...host.sentenceHost, committed: host.observeTitle, fold() {} }); return; }
    const visited = new Set<string>(), read = (turn: Turn | undefined) => { if (!turn || visited.has(turn.id)) return; visited.add(turn.id); host.markSeenResults(turn.out); for (const h of turn.out) if (h.kind === "spawn" || h.kind === "relay") read(host.STARTS.get(h.id)); }; read(root);
    const seen = new Set([root.id]), sessions = new Set([root.sid]), scope = new Set([root.sid]), reached = new Set([root.id]); let n = 0;
    const reach = (turn: Turn) => { for (const h of turn.sent) { scope.add(h.kind === "toyou" ? h.from : h.to); const child = h.kind === "spawn" || h.kind === "relay" ? host.STARTS.get(h.id) : null; if (child && !reached.has(child.id)) { reached.add(child.id); reach(child); } } }; reach(root);
    const meta = (state: string, text: string, sid: string, turn: Turn | null | undefined, note?: string) => { const s = host.SESS[sid], label = host.machineLabel(s, scope); return { state, stateLabel: host.STATE[state] ?? state, text,
      chip: s ? [s.kind ?? host.HARNESS[s.harness], label].filter(Boolean).join(" · ") : undefined, chipClass: s ? host.hcls(sid) : undefined, tip: label ? "Machine: " + host.hostOf(s) : undefined,
      harness: s ? harnessSnapshot(s.harness) : undefined, note: note ?? undefined, session: s && !s.stub ? sid : undefined, turn: turn?.id, name: s?.name }; };
    const start = root.start, text = start ? start.brief : root.u?.text, initial = start ? host.sentenceSnapshot(start, null) : root.u ? host.sentenceSnapshot({ kind: "ask", id: root.id, from: "you", to: root.sid, at: root.at ?? host.NOW, status: "done", brief: "" }, null) : { icon: host.I.more, parts: [{ className: "who", text: host.SESS[root.sid].name }, { className: "verb", text: " · a turn whose start isn't in these logs" }] };
    const outcome = host.turnEnd(root), hops: Hop[] = [{ key: "root:" + root.id, className: "k-root", icon: initial.icon, parts: initial.parts, nodeClass: host.hcls(start ? start.from : root.u ? "you" : root.sid), turn: root.id, handoff: start?.id, time: start ? host.clock(start.at) : undefined, brief: text || undefined, meta: meta(outcome?.st ?? "idle", outcome?.text ?? "Nothing recorded", root.sid, root) }];
    const walk = (turn: Turn) => { for (const h of turn.sent) {
      const result = h.kind === "toyou" && h.ask === "result", child = h.kind === "spawn" || h.kind === "relay" ? host.STARTS.get(h.id) : null, target = h.kind === "toyou" ? h.from : h.to;
      const sentence = result ? { icon: host.I.result, parts: [{ className: "verb", text: host.statWord(h) ?? "" }] } : host.sentenceSnapshot(h, null);
      const hop: Hop = { key: h.id, className: "child k-" + h.kind + " s-" + h.status + (child || h.kind === "toyou" || h.kind === "move" ? "" : " stub"), icon: sentence.icon, parts: sentence.parts, nodeClass: host.hcls(target), handoff: h.id, turn: child?.id, time: host.clock(h.at) }; hops.push(hop);
      if (result) { n++; continue; }
      hop.brief = h.brief; hop.answers = host.answersOf(h) ?? undefined; hop.result = h.result || undefined;
      if (h.kind === "move") { hop.meta = meta("done", "Moved", h.to, turn); continue; }
      n++;
      if (h.kind === "toyou") { hop.meta = meta(host.isResult(h) ? host.SEEN_RESULTS.has(h.id) ? "read" : "new" : h.status === "done" ? "done" : h.status, host.statWord(h) ?? "", h.from, turn); continue; }
      sessions.add(h.to); const end = child && host.turnEnd(child); hop.meta = meta(end ? end.st ?? "idle" : h.status === "done" ? "done" : h.status, end ? end.text ?? "Nothing recorded" : host.statWord(h) ?? "", h.to, child, child ? undefined : "Its turn isn't in these logs");
      if (child && !seen.has(child.id)) { seen.add(child.id); walk(child); }
    } }; walk(root);
    renderTraceScreen(page, { empty: false, hops, agents: agentSnapshot(root), summary: [sessions.size + (sessions.size === 1 ? " session" : " sessions"), n + (n === 1 ? " handoff" : " handoffs"), [...host.machineLabels(scope).values()].join(", ")].filter(Boolean).join(" · ") }, { ...host.sentenceHost, committed: host.observeTitle,
      fold(id) { RUN_EXPANDED.get(root.id)?.add(id); renderTrace(page, root.id); },
    });
    return [sessions.size + (sessions.size === 1 ? " session" : " sessions"), n + (n === 1 ? " handoff" : " handoffs"), [...host.machineLabels(scope).values()].join(", ")].filter(Boolean).join(" · ");
  }
  // ---- Session page --------------------------------------------------------------------------------------------------
  function renderSession(page: HTMLElement, sid: string, opts: {only?: ReadonlySet<string>} = {}) {
    host.markSeenResults(host.H.filter((h) => host.isResult(h) && h.from === sid));
    const raw = new Map(host.transcriptEntries(host.TX[sid] ?? [], sid).map((e) => [e.slot != null ? sid + "#slot:" + e.slot : e.key, e]));
    renderSessionScreen(page, host.transcriptSnapshot(sid, opts), { ...host.sentenceHost, committed: host.observeTitle, trace: host.goTrace,
      toolAll(key, label) { const e = raw.get(key); if (e?.k === "tool") { const [ic,v] = host.verb(e.name); host.openStepViewer(e, v, ic, label); } },
      script(key) { const e = raw.get(key); if (e?.k === "tool") host.openScript(e); }, image: host.openImage,
      background(call, trigger) { const target = trigger.closest('section[aria-label="Transcript"]')?.querySelector<HTMLElement>('.step[data-tid="' + CSS.escape(call) + '"]'); if (!target) return; host.stopOpeningEndPin(); for (let parent = target.parentElement; parent; parent = parent.parentElement) { const toggle = host.opener(parent); if (toggle?.getAttribute("aria-expanded") === "false") toggle.click(); } host.centre(target); target.classList.add("flash"); host.scope.timeout(() => target.classList.remove("flash"), 1500); },
      pager(button) { host.loadPager(button, true); }, jump: host.jumpToLatest,
    });
  }
  // Whether a session page ends in its status line: a child's when it is running or has returned, any other session's always.
  // renderSession and patchSession share it.
  const showsFooter = (s: Session, origin: Handoff | undefined) => origin ? s.state === "work" || s.state === "done" || s.state === "err" || origin.status === "done" || origin.status === "err" : !s.stub && !s.role && s.state in host.STATE;


  return {harnessSnapshot, costSnapshot, showsFooter, renderHome, renderMachines, renderMachine, renderTrace, renderSession};
}
