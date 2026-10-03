import { render } from 'preact';
import { screenText, Glyph, Harness } from './screens';
import type { HarnessMark } from './screens';
export interface MenuAction { key: string; className?: string; text: string; icon?: string; dot?: string; note?: string; checked?: boolean }
export interface MenuDetail { label: string; value: string; mono?: boolean; harness?: HarnessMark }
export interface MenuRun { id: string; depth: number; label: string; name: string; kind: string; state: string; stateLabel: string; cost: string }
export interface MenuTokenModel { id: string; rows: readonly { label: string; count: string; exact?: string; cost: string }[] }
export interface SessionMenuSnapshot {
  actions: readonly MenuAction[]; path: readonly { id: string; name: string; harness: string; harnessName: string }[];
  status: string; state: string; stateLabel: string; details: readonly MenuDetail[];
  cost: { figure: string; caption: string; note: string; details: readonly MenuDetail[]; mismatch?: string; runs: readonly MenuRun[]; models: readonly MenuTokenModel[]; includesRuns: boolean };
  notice: string; icons: { chevron: string; copy: string }; command: string;
}
export interface SessionMenuHost { action(key: string): void; session(id: string): void; wide(): boolean }
export function renderSessionMenu(root: HTMLElement, snapshot: SessionMenuSnapshot, host: SessionMenuHost) {
  let tokens = false, allRuns = false, copyLabel = 'Copy resume command', wide = host.wide();
  const live = () => root.isConnected;
  function details(rows: readonly MenuDetail[]) { return <dl class="kv">{rows.map((row, i) => <><dt key={'dt' + i}>{screenText(row.label)}</dt><dd key={'dd' + i} class={row.mono ? 'mono' : undefined}>{row.harness ? <span><Harness mark={row.harness} size={16} />{screenText(row.value)}</span> : screenText(row.value)}</dd></>)}</dl>; }
  function paint() {
    render(<>{snapshot.path.length > 1 && <><div class="menu-status" role="presentation"><span class={'dot ' + snapshot.state} role="img" aria-label={snapshot.stateLabel} aria-hidden="true" /><span>{screenText(snapshot.status)}</span></div><div class="menu-list menu-path-menu" role="menu"><div class="menu-path-group" role="group" aria-labelledby="menu-path-heading"><div class="menu-section-heading" id="menu-path-heading">Session path</div>{snapshot.path.slice(0, -1).map((ancestor, i) => <button key={ancestor.id} class="menu-item menu-path-item" type="button" role="menuitem" aria-label={i === snapshot.path.length - 2 ? 'Up to ' + ancestor.name : undefined} onClick={() => { if (live()) host.session(ancestor.id); }}><span class={'menu-path-chevron' + (i === snapshot.path.length - 2 ? '' : ' blank')} aria-hidden={i === snapshot.path.length - 2 ? undefined : true} /><span class="menu-path-name">{screenText(ancestor.name)}</span><span class={'hname h-' + ancestor.harness}>{screenText(ancestor.harnessName)}</span></button>)}</div></div><div class="menu-separator" role="separator" /></>}
      <section class="panel-sec"><div class="menu-list" role="menu">{snapshot.actions.map(action => <button key={action.key} class={'menu-item' + (action.className ? ' ' + action.className : '')} type="button" role={action.checked === undefined ? 'menuitem' : 'menuitemcheckbox'} aria-checked={action.checked === undefined ? undefined : wide} onClick={() => {
        if (!live()) return;
        if (action.key === 'copy') { navigator.clipboard?.writeText(snapshot.command).then(() => { if (live()) { copyLabel = 'Copied'; paint(); } }, () => { if (live()) { copyLabel = snapshot.command; paint(); } }); }
        else { host.action(action.key); if (action.key === 'wide') { wide = host.wide(); paint(); } }
      }}>{action.icon && <Glyph path={action.icon} />}{action.dot && <span class={'dot ' + action.dot} aria-hidden="true" />}<span>{screenText(action.key === 'copy' ? copyLabel : action.text)}</span>{action.checked !== undefined && <span class="switch" />}{action.note && <span class="menu-note">{action.note}</span>}</button>)}</div></section>
      <section class="panel-sec"><h3>Details</h3>{details(snapshot.details)}</section>
      <section class="panel-sec cost"><h3>Cost</h3><div class="cost-fig"><span class="cost-big">{snapshot.cost.figure}</span><span class="cost-cap">{snapshot.cost.caption}</span></div><p class="cost-note">{screenText(snapshot.cost.note)}</p>{snapshot.cost.details.length > 0 && details(snapshot.cost.details)}{snapshot.cost.mismatch && <p class="cost-note">{snapshot.cost.mismatch}</p>}{snapshot.cost.runs.length > 0 && <div class="runs" aria-label="Runs and their cost">{snapshot.cost.runs.map((run, i) => <button key={run.id} class={'run-row depth' + Math.min(run.depth, 1)} type="button" hidden={!allRuns && i >= 5} aria-label={run.label} onClick={() => { if (live()) host.session(run.id); }}><span class={'dot ' + run.state} role="img" aria-label={run.stateLabel} data-tip={run.stateLabel} /><span class="nm">{screenText(run.name)}<span class="kind">{screenText('· ' + run.kind)}</span></span><span class="v">{run.cost}</span><Glyph path={snapshot.icons.chevron} className="chev" /></button>)}{!allRuns && snapshot.cost.runs.length > 5 && <button class="link" type="button" onClick={() => { if (live()) { allRuns = true; paint(); } }}>{'Show ' + (snapshot.cost.runs.length - 5) + ' more'}</button>}</div>}
        <button class="disclose" type="button" aria-expanded={tokens} onClick={() => { if (live()) { tokens = !tokens; paint(); } }}><span>{screenText('Tokens by model' + (snapshot.cost.includesRuns ? ' · incl. runs' : ''))}</span><Glyph path={snapshot.icons.chevron} className="chev" /></button><div class="tokens" hidden={!tokens}>{snapshot.cost.models.map(model => <><div key={model.id} class="tok-model">{screenText(model.id)}</div>{model.rows.map(row => <div key={model.id + row.label} class="tok-line"><span>{row.label}</span><span data-tip={row.exact ? row.exact + ' tokens' : undefined}>{row.count}{row.exact && <span class="sr-only">{' (' + row.exact + ')'}</span>}</span><span>{row.cost}</span></div>)}</>)}</div>
      </section><p class="third-party">{snapshot.notice}</p></>, root);
  }
  paint();
}
