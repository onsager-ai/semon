import { render } from 'preact';
import { Glyph, screenText } from './screens';
export interface BarLabel { key: string; text: string; tip?: string; drop: number; className?: string; state?: string; stateLabel?: string; effort?: string; action?: 'errors' | 'runs' }
export interface ViewerBarSnapshot {
  mode: 'normal' | 'find' | 'errors'; name: string; session?: string; state?: string; stateLabel?: string; stateTip?: string; showState: boolean;
  ancestors: readonly { id: string; name: string }[]; crumb?: string; labels: readonly BarLabel[]; trace: boolean; analytics: boolean; days: number;
  query: string; count: string; filter: string; failed: number; signals: number;
  errorMode: string; errorText: string; errorDisabled: boolean;
  icons: Readonly<Record<'search' | 'more' | 'back' | 'x' | 'up' | 'dn', string>>;
}
export interface ViewerBarHost {
  ancestor(id: string): void; crumb(): void; find(): void; closeFind(): void; query(value: string): void; filter(value: string): void;
  menu(trigger: HTMLButtonElement, runs?: boolean): void; errors(): void; closeErrors(): void; step(direction: number): void; range(days: number): void;
}
export function createViewerBar() {
  const title = document.createElement('div'), actions = document.createElement('div'), mode = document.createElement('div');
  title.className = 'ttl'; actions.className = 'viewer-bar-actions'; mode.className = 'viewer-bar-mode';
  let disposed = false, query = '', findRow: HTMLDivElement | null = null;
  function update(view: ViewerBarSnapshot, host: ViewerBarHost) {
    if (disposed) throw new Error('Viewer bar is destroyed');
    const active = (node: HTMLElement, action: () => void) => { if (!disposed && node.isConnected) action(); };
    const icon = (name: keyof ViewerBarSnapshot['icons']) => <Glyph path={view.icons[name]} className="" />;
    const state = (value: string, label: string) => <span class={'dot ' + value} role="img" aria-label={label} />;
    if (query.toLowerCase() !== view.query) query = view.query;
    render(view.mode === 'normal' ? <><div class="l1">{view.ancestors.map(item => <span key={item.id} class="viewer-crumb"><button class="crumb" type="button" aria-label={'Open ' + item.name} onClick={event => active(event.currentTarget, () => host.ancestor(item.id))}>{screenText(item.name)}</button><span class="crumb-sep">›</span></span>)}{view.crumb && <><button class="crumb" type="button" aria-label={'Back to ' + view.crumb} onClick={event => active(event.currentTarget, host.crumb)}>{screenText(view.crumb)}</button><span class="crumb-sep">›</span></>}{view.showState && view.state && <span class="l1-state" data-tip={view.stateTip}>{state(view.state, view.stateLabel ?? view.state)}</span>}<span class="t" data-tip={view.name} data-tip-clipped="">{screenText(view.name)}</span></div>{view.labels.length > 0 && <div class="meta-line">{view.labels.map(label => { const contents = <>{label.state && state(label.state, label.stateLabel ?? label.state)}{label.state ? <span>{screenText(label.text)}</span> : screenText(label.text)}{label.effort && <span class="meta-effort"><span class="meta-sep">·</span>{label.effort}</span>}</>; const cls = 'lab' + (label.action ? ' lab-btn' : '') + (label.className ? ' ' + label.className : ''); return label.action ? <button key={label.key} class={cls} type="button" data-drop={label.drop} data-tip={label.tip} aria-label={label.tip} onClick={event => active(event.currentTarget, () => label.action === 'errors' ? host.errors() : host.menu(document.querySelector<HTMLButtonElement>('#more-btn') ?? event.currentTarget, true))}>{contents}</button> : <span key={label.key} class={cls} data-drop={label.drop} data-tip={label.tip}>{contents}</span>; })}</div>}</> : null, title);
    render(view.mode === 'normal' ? view.analytics ? <div class="analytics-range" role="group" aria-label="Analytics range">{[[1,'24 h'],[7,'7 d'],[30,'30 d']].map(([days,label]) => <button key={days} type="button" data-e={'analytics-range:' + days} aria-pressed={view.days === days} onClick={event => active(event.currentTarget, () => host.range(Number(days)))}>{label}</button>)}</div> : view.session ? <>{!view.trace && <button key={'find:' + view.session} class="ibtn" type="button" id="find-btn" aria-label="Find and filter" onClick={event => active(event.currentTarget, host.find)}>{icon('search')}</button>}<button key={'more:' + view.session} class="ibtn" type="button" id="more-btn" aria-label="Session menu: details, cost and actions" aria-haspopup="dialog" aria-expanded="false" onClick={event => active(event.currentTarget, () => host.menu(event.currentTarget))}>{icon('more')}</button></> : null : null, actions);
    render(view.mode === 'find' ? <><div class="find-row" ref={node => { findRow = node; }}><button class="ibtn" type="button" aria-label="Close find" onClick={event => active(event.currentTarget, host.closeFind)}>{icon('back')}</button><label class="search">{icon('search')}<input id="find" type="search" placeholder={'Find in ' + view.name} aria-label="Find in transcript" value={query} onInput={event => { query = event.currentTarget.value; active(event.currentTarget, () => host.query(query)); }} onKeyDown={event => { if (event.key === 'Escape') { event.stopPropagation(); active(event.currentTarget, host.closeFind); } }} /></label><span class="fcount" aria-live="polite">{view.count}</span></div><div class="find-chips" role="group" aria-label="Show">{[['all','All',undefined],['messages','Messages',undefined],['steps','Steps',undefined],...(view.failed ? [['failures','Failed steps',view.failed]] : []),...(view.signals ? [['signals','Signals',view.signals]] : [])].map(([key,label,count]) => <button key={key} class="chip" type="button" data-filter={key} aria-pressed={view.filter === key} onClick={event => active(event.currentTarget, () => host.filter(String(key)))}><span>{label}</span>{count !== undefined && <span class="n">{count}</span>}</button>)}</div></> : view.mode === 'errors' ? <div class="errnav-bar" role="group" aria-label={view.errorMode === 'signals' ? 'Session signals' : 'Failed steps'}><button class="ibtn" type="button" id="err-close" aria-label={'Close ' + view.errorMode} onClick={event => active(event.currentTarget, host.closeErrors)}>{icon('x')}</button><div class="find errnav"><span class="errs-dot" aria-hidden="true" /><span class="errnav-count">{view.errorText}</span></div>{([-1,1] as const).map(direction => <button key={direction} class="ibtn errnav-btn" type="button" id={direction < 0 ? 'err-prev' : 'err-next'} aria-label={(direction < 0 ? 'Previous ' : 'Next ') + (view.errorMode === 'signals' ? 'signal' : 'error')} disabled={view.errorDisabled} onClick={event => active(event.currentTarget, () => host.step(direction))}>{icon(direction < 0 ? 'up' : 'dn')}</button>)}</div> : null, mode);
    return { titleSlot: title, actions, mode, accountTarget: view.mode === 'find' ? findRow ?? undefined : undefined };
  }
  return { update, destroy() { if (disposed) return; disposed = true; for (const root of [title,actions,mode]) { render(null, root); root.remove(); } } };
}

/** Host transactions ask this owner to fit its metadata after a synchronous commit. */
export function measureViewerBar(root: HTMLElement) {
  const line = root.querySelector<HTMLElement>('.meta-line'); if (!line) return;
  const labels = [...line.querySelectorAll<HTMLElement>('.lab')]; for (const label of labels) label.hidden = false;
  const order = labels.filter(label => !label.classList.contains('state')).sort((a,b) => Number(b.dataset.drop ?? 0) - Number(a.dataset.drop ?? 0));
  for (const label of order) { if (line.scrollWidth <= line.clientWidth + 1) break; label.hidden = true; }
}
