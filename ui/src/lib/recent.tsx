import { render } from 'preact';
import { safePath } from './account';

/** Presentation only. Hosts retain model, order, expansion, selection and scroll state. */
export interface RecentItem {
  id: string;
  name: string;
  label: string;
  state: string;
  stateLabel: string;
  age: string;
  model: string;
  modelTip: string;
  harness?: { id: string; name: string; light: string; dark: string; darkTheme: boolean };
  fields: readonly { className: string; text: string; priority: number; tip: string; icon: string }[];
  current?: 'page' | 'true';
  rail: boolean;
  childState?: string;
  flag?: { state: string; tip: string };
  open: boolean;
  children?: readonly RecentItem[];
  depth: number;
  all?: number;
  stuck: boolean;
}
export interface RecentSnapshot { items: readonly RecentItem[]; empty: boolean }
export interface RecentHost {
  open(id: string): void;
  toggle(id: string, open: boolean): void;
  all(id: string, trigger: HTMLButtonElement): void;
  fewer(id: string): void;
}
export interface RecentRenderer {
  update(snapshot: RecentSnapshot): void;
  fit(): void;
  destroy(): void;
}
const CHEVRON = 'M9 6l6 6-6 6';
function Icon({ path }: { path: string }) {
  return <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d={path} /></svg>;
}
/** One disjoint root. Detached controls and scheduled measurements stop on destruction. */
export function createRecentRenderer(root: HTMLElement, host: RecentHost): RecentRenderer {
  let destroyed = false, frame = 0;
  const active = (node: HTMLElement) => !destroyed && node.isConnected && root.contains(node);
  function fit() {
    if (destroyed) return;
    for (const meta of root.querySelectorAll<HTMLElement>('.session-row-meta')) {
      if (!meta.isConnected || !meta.clientWidth) continue;
      const values = [...meta.querySelectorAll<HTMLElement>('[data-drop]')];
      for (const value of values) value.hidden = meta.clientWidth < 280 && value.classList.contains('row-duration');
      for (const value of values.sort((a, b) => Number(b.dataset.drop) - Number(a.dataset.drop))) {
        if (meta.scrollWidth <= meta.clientWidth + 1) break;
        value.hidden = true;
      }
    }
  }
  function Item({ item }: { item: RecentItem }) {
    const kids = item.children, expandable = !!kids && !item.rail;
    const harness = item.harness;
    return <div class="treeitem" role="treeitem" data-id={item.id} aria-label={item.name} tabIndex={0} aria-expanded={expandable ? item.open : undefined}
      onKeyDown={event => {
        if (!active(event.currentTarget)) return;
        const target = event.target as HTMLElement;
        if (target !== event.currentTarget && target !== event.currentTarget.querySelector(':scope > .tree-row .srow') && target !== event.currentTarget.querySelector(':scope > .tree-row .tree-toggle')) return;
        if (expandable && (event.key === 'ArrowLeft' || event.key === 'ArrowRight')) {
          const open = event.key === 'ArrowRight';
          if (open !== item.open) { event.preventDefault(); host.toggle(item.id, open); }
        } else if ((event.key === 'Enter' || event.key === ' ') && target === event.currentTarget) { event.preventDefault(); host.open(item.id); }
      }}>
      <div class={'tree-row' + (expandable ? ' has-toggle' : '') + (item.stuck ? ' stuck' : '')}>
        <button class={'srow' + (item.current === 'true' ? ' on-path' : '')} type="button" data-id={item.id} data-session-row="compact" data-tip={item.rail ? item.name : undefined} aria-label={item.label} aria-current={item.current}
          onClick={event => { if (active(event.currentTarget)) host.open(item.id); }}>
          <span class="session-row-main srow-main">
            <span class={'dot ' + item.state} role="img" aria-label={item.stateLabel} data-tip={item.rail ? undefined : item.stateLabel} />
            <span class="nm" data-tip={item.name} data-tip-clipped="">{item.name}</span>
            {item.flag && <span class={'kid-flag ' + item.flag.state} data-tip={item.flag.tip} aria-hidden="true" />}
            <span class="ag">{item.age}</span>
            {item.childState && <span class={'dot ' + item.childState + ' child-dot'} role="img" aria-label={item.childState} aria-hidden="true" />}
          </span>
          <span class="session-row-meta srow-meta for">
            {harness && <span class="hicon hi-sidebar" data-harness={harness.id} role="img" aria-label={harness.name} data-tip={harness.name}>
              {harness.light === harness.dark ? <img src={harness.light} alt="" draggable={false} decoding="async" /> : <>
                <img class="hi-light" loading={harness.darkTheme ? 'lazy' : undefined} src={harness.light} alt="" draggable={false} decoding="async" />
                <img class="hi-dark" loading={harness.darkTheme ? undefined : 'lazy'} src={harness.dark} alt="" draggable={false} decoding="async" />
              </>}
            </span>}
            <span class="row-model" data-tip={item.modelTip}>{item.model}</span>
            {item.fields.map(field => <span key={field.priority} class={field.className + ' row-field'} data-drop={field.priority} data-tip={field.tip}><Icon path={field.icon} /><span class="field-value">{field.text}</span></span>)}
          </span>
        </button>
        {expandable && <button class="tree-toggle" type="button" data-tree-toggle={item.id} aria-label={(item.open ? 'Collapse ' : 'Expand ') + item.name} aria-expanded={item.open}
          onClick={event => { event.stopPropagation(); if (active(event.currentTarget)) host.toggle(item.id, !item.open); }}><Icon path={CHEVRON} /></button>}
        {item.stuck && <button class="tree-fewer" type="button" aria-label={'Show fewer sessions under ' + item.name}
          onClick={event => { event.stopPropagation(); if (active(event.currentTarget)) host.fewer(item.id); }}><span>Show fewer</span><Icon path={CHEVRON} /></button>}
      </div>
      {expandable && <div class="tree-group" data-depth={Math.min(item.depth + 1, 4)} role="group" aria-label={'Sessions spawned by ' + item.name}>
        {kids.map(child => <Item key={child.id} item={child} />)}
        {item.all !== undefined && <button class="tree-all" type="button" data-id={item.id} role="treeitem" aria-haspopup={window.matchMedia('(max-width: 760px)').matches ? 'dialog' : undefined} aria-label={'All ' + item.all + ' sessions under ' + item.name}
          onClick={event => { event.stopPropagation(); if (active(event.currentTarget)) host.all(item.id, event.currentTarget); }}><span>{'All ' + item.all}</span><Icon path={CHEVRON} /></button>}
      </div>}
    </div>;
  }
  window.addEventListener('resize', fit, { passive: true });
  return {
    update(snapshot) {
      if (destroyed) throw new Error('Recent renderer is destroyed');
      const ids = new Set<string>();
      function validate(items: readonly RecentItem[]) {
        for (const item of items) {
          if (!item.id || ids.has(item.id)) throw new Error('Invalid Recent identity');
          ids.add(item.id);
          if (item.harness && (!safePath(item.harness.light) || !safePath(item.harness.dark))) throw new Error('Invalid Recent harness path');
          if (item.children) validate(item.children);
        }
      }
      validate(snapshot.items);
      render(<>{snapshot.items.map(item => <Item key={item.id} item={item} />)}{snapshot.empty && <p class="ghead" role="none">No sessions match</p>}</>, root);
      cancelAnimationFrame(frame); frame = requestAnimationFrame(fit);
    },
    fit,
    destroy() { if (destroyed) return; destroyed = true; cancelAnimationFrame(frame); window.removeEventListener('resize', fit); render(null, root); },
  };
}
