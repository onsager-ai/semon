import { render } from 'preact';
import { createMeasuredLayout } from './layout';
import { claimScreen, Glyph, Harness, screenText } from './screens';
import type { HarnessMark } from './screens';
import { MarkdownContent, Inline } from './richtext';
import { Sentence } from './sentence';
import type { SentencePart, SentenceHost } from './sentence';
export interface Hop {
  key: string; className: string; nodeClass: string; icon: string; handoff?: string; turn?: string; parts: readonly SentencePart[]; time?: string;
  brief?: string; answers?: readonly string[]; result?: string;
  meta?: { state: string; stateLabel: string; text: string; chip?: string; chipClass?: string; tip?: string; harness?: HarnessMark; note?: string; session?: string; turn?: string; name?: string };
}
export interface AgentRow {
  kind: 'agent' | 'more' | 'heading'; id: string; depth: number; label: string; name: string; relay?: boolean; critical?: boolean; harnessClass?: string;
  model?: string; state?: string; stateLabel?: string; tokens?: string; duration?: string; cost?: string; costTip?: string;
  segments?: readonly { left: number; width: number; kind: string }[]; spawns?: readonly number[]; waits?: readonly { left: number; width: number; tip: string }[];
  edge?: { parent: string; started: number; done: number; returned: boolean }; turn?: string; fold?: string;
}
export interface AgentChart { summary: readonly string[]; start: string; end: string; ticks: readonly { left: number; label: string; nearEnd: boolean }[]; legend: string; incomplete: boolean; rows: readonly AgentRow[] }
export interface TraceSnapshot { empty: boolean; summary?: string; hops: readonly Hop[]; agents?: AgentChart }
export interface TraceHost extends SentenceHost { fold(id: string): void; committed(): void }
interface Edge { parent: string; child: string; className: string; path: string }
interface TraceState {
  layout: ReturnType<typeof createMeasuredLayout>; observer: ResizeObserver; frame: number; disposed: boolean;
  briefs: Map<string, { node: HTMLElement; more: HTMLButtonElement }>; open: Set<string>; clipped: Set<string>;
  chart: HTMLDivElement | null; axis: HTMLDivElement | null; edges: SVGSVGElement | null; paths: Edge[]; hot: string | null; paint(): void; measure(): void;
}
const states = new WeakMap<HTMLElement, TraceState>();
export function renderTraceScreen(root: HTMLElement, snapshot: TraceSnapshot, host: TraceHost) {
  let state = states.get(root);
  if (!state) {
    state = { layout: createMeasuredLayout(), frame: 0, disposed: false, briefs: new Map(), open: new Set(), clipped: new Set(), chart: null, axis: null, edges: null, paths: [], hot: null, paint() {}, measure() {}, observer: new ResizeObserver(() => state?.measure()) };
    states.set(root, state); const owner = state;
    claimScreen(root, 'trace', () => { owner.disposed = true; owner.observer.disconnect(); cancelAnimationFrame(owner.frame); owner.layout.destroy(); if (owner.edges) render(null, owner.edges); states.delete(root); });
    document.fonts.ready.then(() => { if (!owner.disposed) { cancelAnimationFrame(owner.frame); owner.frame = requestAnimationFrame(() => owner.measure()); } });
  }
  const owner = state;
  const active = (action: () => void) => { if (!owner.disposed && root.isConnected) action(); };
  function paintEdges() {
    if (!owner.edges || owner.disposed) return;
    render(<>{owner.paths.map((edge, i) => <path key={i} data-parent={edge.parent} data-child={edge.child} class={edge.className + (owner.hot === edge.parent || owner.hot === edge.child ? ' hot' : '')} d={edge.path} />)}</>, owner.edges);
  }
  function hot(id: string | null) { owner.hot = id; paintEdges(); }
  owner.measure = () => {
    if (owner.disposed || !root.isConnected) return;
    let changed = false;
    for (const [key, pair] of owner.briefs) {
      if (owner.open.has(key) || !pair.node.clientHeight) continue;
      const clipped = pair.node.scrollHeight > pair.node.clientHeight + 1;
      if (owner.clipped.has(key) !== clipped) { changed = true; if (clipped) owner.clipped.add(key); else owner.clipped.delete(key); }
    }
    if (changed) { owner.paint(); return; }
    if (owner.axis) { const bounds = owner.axis.getBoundingClientRect(); let previousRight = bounds.left - 8; for (const label of owner.axis.querySelectorAll<HTMLElement>('.agent-axis-tick > span')) { label.hidden = false; const rect = label.getBoundingClientRect(); if (rect.left < previousRight + 8 || rect.right > bounds.right) label.hidden = true; else previousRight = rect.right; } }
    if (!owner.chart || !owner.edges) return;
    const chart = owner.chart, rect = chart.getBoundingClientRect(); owner.edges.setAttribute('viewBox', '0 0 ' + rect.width + ' ' + rect.height); owner.paths = [];
    if (window.matchMedia('(min-width: 761px)').matches && rect.width && rect.height) {
      const rows = new Map([...chart.querySelectorAll<HTMLElement>('[data-agent-id]')].map(row => [row.dataset.agentId!, row]));
      for (const row of snapshot.agents?.rows ?? []) {
        if (!row.edge) continue; const parent = rows.get(row.edge.parent), child = rows.get(row.id); if (!parent || !child) continue;
        const parentTrack = parent.querySelector<HTMLElement>('.agent-track'), childTrack = child.querySelector<HTMLElement>('.agent-track'); if (!parentTrack || !childTrack) continue;
        const pr = parent.getBoundingClientRect(), cr = child.getBoundingClientRect(), ptr = parentTrack.getBoundingClientRect(), ctr = childTrack.getBoundingClientRect();
        const yp = pr.top - rect.top + pr.height / 2, yc = cr.top - rect.top + cr.height / 2, at = ptr.left - rect.left + ptr.width * row.edge.started / 100, done = ctr.left - rect.left + ctr.width * row.edge.done / 100;
        const cls = 'agent-edge' + (row.critical ? ' critical' : '');
        const add = (path: string, className = cls) => owner.paths.push({ parent: row.edge!.parent, child: row.id, path, className });
        add('M ' + at + ' ' + yp + ' V ' + yc); add('M ' + at + ' ' + (yp - 4) + ' V ' + (yp + 4));
        if (row.edge.returned) { add('M ' + done + ' ' + yc + ' V ' + (yp + 4)); add('M ' + (done - 3) + ' ' + (yp + 4) + ' L ' + done + ' ' + yp + ' L ' + (done + 3) + ' ' + (yp + 4), 'agent-edge-arrow' + (row.critical ? ' critical' : '')); }
      }
    }
    paintEdges();
  };
  owner.paint = () => {
    if (owner.disposed) return;
    owner.observer.disconnect(); owner.briefs.clear(); owner.layout.reset();
    const left = (value: number) => owner.layout.className('left', value, '%'), width = (value: number) => owner.layout.className('width', value, '%'), indent = (depth: number) => owner.layout.className('width', Math.min(3, depth) * 12, 'px');
    const agents = snapshot.agents;
    render(<>{snapshot.summary && <p class="trace-summary">{screenText(snapshot.summary)}</p>}<div class="ph sr"><h1>Trace</h1></div>{snapshot.empty ? <p class="empty">This turn isn't in the logs on this machine.</p> : <>{agents && <section class="agents-panel" aria-label="Agents"><h2 class="agents-title">Agents</h2><div class="agents-summary">{agents.summary.map((item, i) => <span key={i}>{screenText(item)}</span>)}</div><div class="agents-axis"><div class="agent-axis-track" ref={node => { owner.axis = node; }}><span class="agent-axis-time start">{agents.start}</span><span class="agent-axis-time end">{agents.end}</span>{agents.ticks.map((tick, i) => <span key={i} class={'agent-axis-tick' + (tick.nearEnd ? ' near-end' : '') + ' ' + left(tick.left)}><span>{tick.label}</span></span>)}</div></div><div class="agents-legend"><span class="agents-legend-swatch" /><span>{agents.legend}</span>{agents.incomplete && <span>{screenText('· Wait history incomplete')}</span>}</div><div class="agents-chart" ref={node => { owner.chart = node; }}>{agents.rows.map(row => row.kind === 'heading' ? <div key={row.id} class="agent-relayed-heading">Relayed to</div> : row.kind === 'more' ? <button key={row.id} class="agent-row agent-more" type="button" data-depth={Math.min(3, row.depth)} aria-label={row.label} onClick={() => active(() => host.fold(row.fold ?? row.id))}><span class="agent-label"><span class="agent-identity"><span class={'agent-indent ' + indent(row.depth)} /><span class="agent-name">{row.name}</span></span></span><span class="agent-duration" /><span class="agent-cost" data-tip={row.costTip}>{row.cost}</span><span class="agent-track" /></button> : <button key={row.id} class="agent-row" type="button" data-agent-id={row.id} data-depth={Math.min(3, row.depth)} data-critical={String(!!row.critical)} aria-label={row.label} onClick={() => active(() => host.session(row.id, row.turn))} onPointerEnter={() => hot(row.id)} onPointerLeave={() => hot(null)} onFocus={() => hot(row.id)} onBlur={() => hot(null)}><span class="agent-label"><span class="agent-identity"><span class={'agent-indent ' + indent(row.depth)} /><span class={'agent-harness-dot ' + row.harnessClass} aria-hidden="true" />{row.relay && <Glyph path="M4 7h13l-3-3M20 17H7l3 3" className="agent-relay-icon" />}<span class="agent-name">{screenText(row.name)}</span></span><span class="agent-meta"><span class="agent-model">{screenText(row.model ?? '')}</span><span class={'agent-status stat ' + row.state} data-tip={row.stateLabel}><span class={'dot ' + row.state} role="img" aria-label={row.stateLabel} data-tip={row.stateLabel} /></span><span class="agent-tokens" data-tip="Session totals, including this row's spawned agents">{row.tokens}</span></span></span><span class="agent-duration">{row.duration}</span><span class="agent-cost" data-tip={row.costTip}>{row.cost}</span><span class="agent-track" aria-hidden="true">{agents.ticks.map((tick, i) => <span key={'grid' + i} class={'agent-gridline ' + left(tick.left)} aria-hidden="true" />)}{row.segments?.map((segment, i) => <span key={'segment' + i} class={'agent-segment ' + segment.kind + ' ' + left(segment.left) + ' ' + width(segment.width)} />)}{row.spawns?.map((position, i) => <span key={'spawn' + i} class={'agent-spawn-tick ' + left(position)} aria-hidden="true" />)}{row.waits?.map((wait, i) => <span key={'wait' + i} class={'agent-wait ' + left(wait.left) + ' ' + width(wait.width)} data-tip={wait.tip} />)}</span></button>)}<svg class="agent-edges" aria-hidden="true" preserveAspectRatio="none" ref={node => { owner.edges = node; }} /></div></section>}
      <div class="flow">{snapshot.hops.map(hop => <div key={hop.key} class={'hop ' + hop.className} data-h={hop.handoff} data-turn={hop.turn}><div class={'node ' + hop.nodeClass}><Glyph path={hop.icon} className="" /></div><div class="body"><div class="sent"><Sentence parts={hop.parts} host={host} />{hop.time && <span class="tm">{hop.time}</span>}</div>{hop.brief && <><div class={'brief md' + (owner.open.has(hop.key) ? ' open' : owner.clipped.has(hop.key) ? ' clipped' : '')} ref={node => { if (node) { const more = owner.briefs.get(hop.key)?.more; if (more) owner.briefs.set(hop.key, { node, more }); } }}><MarkdownContent text={hop.brief} /></div><button class="more" type="button" hidden={!owner.clipped.has(hop.key) && !owner.open.has(hop.key)} aria-expanded={owner.open.has(hop.key)} ref={node => { const brief = node?.previousElementSibling; if (node && brief instanceof HTMLElement) owner.briefs.set(hop.key, { node: brief, more: node }); }} onClick={() => active(() => { if (owner.open.has(hop.key)) owner.open.delete(hop.key); else owner.open.add(hop.key); owner.paint(); })}>{owner.open.has(hop.key) ? 'Show less' : 'Show more'}</button></>}{hop.answers && <div class={"result answer" + (hop.answers.length ? "" : " none")}>{!hop.answers.length ? <><b>Answered</b>{" · reply not in these logs"}</> : <><b>{hop.answers.length === 1 ? "You answered: " : "You answered:"}</b>{hop.answers.length === 1 ? hop.answers[0] : <ol>{hop.answers.map((answer, i) => <li key={i}>{screenText(answer)}</li>)}</ol>}</>}</div>}{hop.result && <div class="result"><span class="rl">Result:</span><span><Inline text={hop.result} /></span></div>}{hop.meta && <div class="meta"><span class={'stat ' + hop.meta.state}>{hop.meta.state === 'work' ? <span class="spin" /> : <span class={'dot ' + hop.meta.state} role="img" aria-label={hop.meta.stateLabel} />}{hop.meta.text}</span>{hop.meta.chip && <span class={'chip-h ' + hop.meta.chipClass} data-tip={hop.meta.tip}>{hop.meta.harness && <Harness mark={hop.meta.harness} lead={false} />}{screenText(hop.meta.chip)}</span>}{hop.meta.note && <span class="gone">{screenText(hop.meta.note)}</span>}{hop.meta.session && <button class="open" type="button" onClick={() => active(() => host.session(hop.meta!.session!, hop.meta!.turn))}>{screenText('Open in ' + hop.meta.name + ' ›')}</button>}</div>}</div></div>)}</div>
    </>}</>, root);
    if (owner.chart) owner.observer.observe(owner.chart); for (const pair of owner.briefs.values()) owner.observer.observe(pair.node);
    cancelAnimationFrame(owner.frame); owner.frame = requestAnimationFrame(owner.measure);
  };
  owner.paint(); host.committed();
}

export function measureTraceScreen(root: HTMLElement) { states.get(root)?.measure(); }
