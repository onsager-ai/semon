import { render } from 'preact';
import { useEffect, useRef } from 'preact/hooks';
import { nativePresentation } from './LegacyNative.presentation';
import { createOverviewPages } from '../src/app/overviewPages';
import { EffectScope } from '../src/app/effects';
import { createOrdering } from '../src/lib/ordering';
import { createShellChrome } from '../src/lib/shell';
import { createRecentRenderer } from '../src/lib/recent';
import { projectShellNavigation } from '../src/lib/navigation';
import { createViewerBar, measureViewerBar } from '../src/lib/viewer-bar';
import { renderSessionScreen, updateSessionJump, type TranscriptHost } from '../src/lib/transcript';
import { releaseScreen, Glyph, screenText } from '../src/lib/screens';
import { renderCausalTrace } from '../src/app/tracePresentation';
import { reachableResultIds } from '../src/domain/trace';
import { renderCatalogList, catalogFilterDescription } from '../src/lib/catalog';
import { createCatalogBrowser } from '../src/app/catalogBrowser';
import { createPanelChrome, type PanelChrome } from '../src/lib/panel';
import { renderSessionMenu } from '../src/lib/session-menu';
import { catalogSessionRow, catalogSessionGroups, selectedSessionLabels, selectedSessionMenu, selectedSessionState } from '../src/app/sessionPresentation';
import { I, STATE } from '../src/app/registry';
import { catalogSessionKey, compareCatalogKeys } from '../src/lib/catalogIdentity';
import { catalogCountWitness, completeCatalogCount } from '../src/state/catalog-count';
import { setGeometry } from '../src/lib/geometry';

const idle = () => {};
export interface NativeRouteArgs { frame: string; scope: 'main' | 'two-machines' | 'pushed' }

// A presentation composition: no application router, transport, auth or native
// control adapter. Every response is a fixed exact-head CI fixture publication.
export function NativeRouteOwner({ frame, scope }: NativeRouteArgs) {
  const app = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!app.current) return;
    const owner = app.current, fixture = nativePresentation(scope, frame), route = fixture.route;
    const effects = new EffectScope(), ordering = createOrdering<object>();
    const chrome = createShellChrome({ account: { place: idle, opened: idle, closed: idle,
      navigate: () => true, submit: () => true }, navigate: () => true,
      drawerOpened: idle, drawerClosed: idle, railChanged: idle });
    chrome.mount(owner);
    const page = chrome.slots.content;
    const recentRoot = chrome.slots.recent.querySelector<HTMLElement>('#lanes');
    if (!recentRoot) throw Error('Native shell Recent owner missing');
    let panel: PanelChrome | undefined;
    const dialogs = new Set<{ destroy(): void }>();
    const recent = createRecentRenderer(recentRoot, {
      open: idle, toggle: (id, open) => overview.toggleRecent(id, open),
      all: (id, trigger) => overview.allRecent(id, true, trigger), fewer: (id) => overview.allRecent(id, false),
    });
    const overview = createOverviewPages({
      root: page, effects,
      orderScope: (name, signature = '') => ordering.begin(name, signature, null, { inView: false, touched: false }),
      reviews: () => false, api: fixture.api, route: () => route.v,
      navigationRoute: () => route, selectedMachine: () => route.v === 'machine' ? route.id : null,
      admin: () => null, machine: idle, chrome: updateChrome,
      panelOpened: (value) => dialogs.add(value), panelClosed: (value) => dialogs.delete(value),
      session: idle, trace: idle, sessions: idle,
    });
    const bar = createViewerBar();
    const catalogBrowser = createCatalogBrowser({ canOpen: () => true, opened: idle, closed: idle, changed: idle, clearFilters: idle });
    const selected = route.v === 'session' ? fixture.selected().metadata : undefined;
    const selectedRoot = selected ? document.createElement('div') : null;
    if (selectedRoot) {
      selectedRoot.className = 'catalog-session';
      page.append(selectedRoot);
    }
    const trace = route.v === 'trace' ? fixture.trace() : undefined;
    const meta = selected ?? trace?.metadata;
    const barHost = { ancestor: idle, crumb: idle, find: idle, closeFind: idle, query: idle,
      filter: idle, errors: idle, closeErrors: idle, step: idle, range: idle, menu: openMenu };
    function openMenu(trigger: HTMLButtonElement) {
      if (!meta) throw Error('Missing native selected metadata');
      panel?.destroy();
      const turn = selected ? [...page.querySelectorAll<HTMLElement>('[data-turn]')].find((node) =>
        node.getBoundingClientRect().bottom > document.querySelector<HTMLElement>('#topbar')!.getBoundingClientRect().bottom)?.dataset.turn : undefined;
      const context = selected ? fixture.selected().page.relationship_context : undefined;
      const hasTrace = context?.turns.some((row) => row.id === turn && row.sent.some((id) =>
        context.handoffs.some(({ edge }) => edge.id === id && edge.kind !== 'move'))) ?? false;
      const snapshot = selectedSessionMenu(meta, fixture.label(fixture.source!), false, hasTrace);
      panel = createPanelChrome({ title: meta.name, sub: snapshot.status,
        label: 'Session menu for ' + meta.name, className: 'anchored session-menu' }, {
        opened() { trigger.setAttribute('aria-expanded', 'true'); },
        closed() { if (panel) render(null, panel.body); trigger.setAttribute('aria-expanded', 'false');
          if (trigger.isConnected) trigger.focus({ preventScroll: true }); },
      });
      renderSessionMenu(panel.body, snapshot, { action: idle, session: idle, wide: () => false });
      panel.show();
    }
    const traceActions = document.createElement('div');
    traceActions.className = 'viewer-bar-actions';
    if (trace) {
      const button = document.createElement('button');
      button.type = 'button'; button.id = 'more-btn'; button.className = 'ibtn';
      button.setAttribute('aria-label', 'Session menu'); button.setAttribute('aria-haspopup', 'dialog');
      button.setAttribute('aria-expanded', 'false');
      render(<Glyph path={I.more} className="" />, button);
      effects.listen(button, 'click', () => openMenu(button));
      traceActions.append(button);
    }
    // Exact caller-owned composition from catalogViewer.chrome(). Native records
    // supply parsed publications only; captured DOM is never replayed as markup.
    const title = document.createElement('div');
    title.className = 'ttl';
    const sources = document.createElement('a');
    sources.className = 'ibtn catalog-sources';
    sources.href = '/sessions?choose_source=1';
    sources.setAttribute('aria-label', 'Choose a machine');
    sources.setAttribute('data-tip', 'Choose a machine');
    render(<Glyph path={I.machine} className="" />, sources);
    effects.listen(sources, 'click', (event) => event.preventDefault());
    function updateChrome() {
      const waiting = overview.waitingCount();
      const machineRecent = !['home', 'analytics', 'machines', 'machine'].includes(route.v);
      chrome.update(projectShellNavigation(undefined, route.v === 'machine' ? 'machines' :
        route.v === 'session' || route.v === 'trace' ? 'sessions' : route.v,
        {}, waiting === undefined ? {} : { home: waiting }), false);
      recent.update(machineRecent ? overview.recentSnapshot(fixture.capabilities?.source_key,
        fixture.catalogRows, route.v === 'trace' ? route.sid : selected?.key, false, selected) :
        overview.recentSnapshot(undefined, [], undefined, false));
      const pinned = chrome.slots.recent.querySelector<HTMLElement>('.tree-row.stuck');
      setGeometry(chrome.slots.recent, 'scrollPaddingTop', pinned ? pinned.offsetHeight + 8 : null);
      const scopeNote = overview.recentScope(machineRecent);
      const recentHeading = chrome.slots.recent.querySelector<HTMLElement>('.side-h');
      if (recentHeading) {
        recentHeading.textContent = 'Recent';
        if (scopeNote) recentHeading.setAttribute('data-tip', scopeNote);
        else recentHeading.removeAttribute('data-tip');
      }
      if (scopeNote) recentRoot!.setAttribute('aria-description', scopeNote);
      else recentRoot!.removeAttribute('aria-description');
      title.classList.toggle('catalog-title', route.v !== 'machine' && route.v !== 'trace');
      if (trace && route.v === 'trace') {
        const session = trace.sessions[route.sid];
        render(<div class="l1">
          <button type="button" class="crumb" aria-label={'Back to ' + (session?.name ?? 'session')} onClick={idle}>
            {session?.name ?? 'Session'}
          </button><span class="crumb-sep">›</span><span class="t">Trace</span>
        </div>, title);
      } else if (route.v === 'machine') {
        const machine = overview.machine();
        render(<>
          <div class="l1"><button type="button" class="crumb" aria-label="Back to Machines" onClick={idle}>Machines</button>
            <span class="crumb-sep">›</span><span class="t">{machine?.label ?? 'Machine'}</span></div>
          {machine && <div class="meta-line">
            <span class={'lab state ' + (machine.up ? 'done' : 'err')}>
              <span class={'dot ' + (machine.up ? (machine.working ? 'work' : 'idle') : 'err')} role="img" aria-label={machine.up ? 'Up' : 'Not responding'} />
              <span>{machine.up ? 'Up' : 'Not responding'}</span>
            </span>
            <span class="lab" data-drop="1"><span>{screenText(machine.working + ' working · ' + machine.sessions + (machine.sessions === 1 ? ' session' : ' sessions'))}</span></span>
          </div>}
        </>, title);
      } else {
        render(null, title);
        const label = document.createElement('span');
        label.className = 't catalog-session-name l1';
        label.textContent = route.v === 'home' ? 'Home' : route.v === 'analytics' ? 'Analytics' : route.v === 'machines' ? 'Machines' : selected?.name ?? 'Sessions';
        title.replaceChildren(label);
      }
      const phone = matchMedia('(max-width: 760px)').matches;
      const ancestors = selected?.details?.family?.ancestors ?? [];
      const slots = selected ? bar.update({
        mode: 'normal', name: selected.name, session: selected.key, state: selectedSessionState(selected),
        stateLabel: STATE[selectedSessionState(selected)] ?? 'Recorded history',
        stateTip: 'Status: ' + (STATE[selectedSessionState(selected)] ?? 'Recorded history'),
        showState: !(phone && ancestors.length),
        ancestors: (phone ? [] : ancestors).map((row) => ({ id: row.key, name: row.name })),
        labels: selectedSessionLabels(selected, fixture.label(fixture.source!)),
        trace: false, analytics: false, days: 7, query: '', count: '', filter: 'all',
        failed: selected.details?.errors ?? 0,
        signals: Object.values(selected.details?.signals ?? {}).reduce((sum, n) => sum + n, 0),
        errorMode: '', errorText: '', errorDisabled: true,
        icons: { search: I.search, more: I.more, back: I.back, x: I.x, up: I.up, dn: I.dn },
      }, barHost) : undefined;
      chrome.topbar({
        titleSlot: slots?.titleSlot ?? title,
        actions: route.v === 'analytics' ? [overview.actions] : slots ? [slots.actions] :
          route.v === 'sessions' && fixture.capabilities?.source_candidates ? [sources] : trace ? [traceActions] : [],
        session: !!selected,
        lead: trace ? { label: 'Back to ' + (trace.metadata?.name ?? 'session'), icon: I.chev, back: idle } : { label: 'Open menu', icon: I.menu },
        account: null,
      });
      chrome.drawerAccount(null);
      if (selected || route.v === 'machine')
        effects.frame(() => measureViewerBar(document.querySelector<HTMLElement>('#topbar')!));
    }
    effects.listen(window, 'resize', () => {
      if (selected || route.v === 'machine') measureViewerBar(document.querySelector<HTMLElement>('#topbar')!);
    });
    const transcriptHost: TranscriptHost = { session: idle, machine: idle, sender: idle, committed: idle,
      trace: idle, toolAll: idle, script: idle, image: idle, background: idle, pager: idle,
      jump() { const main = document.querySelector('#main'); if (phone()) window.scrollTo(0, document.documentElement.scrollHeight); else if (main) main.scrollTop = main.scrollHeight; } };
    const phone = () => matchMedia('(max-width: 760px)').matches;
    if (selected) {
      const snapshot = fixture.transcript();
      overview.markResults(fixture.source!, fixture.selected().page.relationship_context.handoffs
        .filter((row) => row.edge.kind === 'toyou' && row.edge.ask === 'result' && row.edge.from === selected.key).map((row) => row.edge.id));
      renderSessionScreen(selectedRoot!, snapshot, transcriptHost);
      const jumpTarget = document.createElement('div'); jumpTarget.className = 'jump-wrap'; jumpTarget.hidden = true;
      const main = document.querySelector<HTMLElement>('#main')!;
      const syncJump = () => {
        const gap = phone() ? document.documentElement.scrollHeight - scrollY - innerHeight : main.scrollHeight - main.scrollTop - main.clientHeight;
        updateSessionJump(selectedRoot!, gap > 80 || (fixture.selected().entries.at(-1)?.slot ?? -1) + 1 < fixture.selected().page.projection.total, 0, false, jumpTarget);
      };
      effects.listen(window, 'scroll', syncJump, { passive: true });
      effects.listen(main, 'scroll', syncJump, { passive: true });
      effects.frame(syncJump);
    } else if (trace) {
      const results = reachableResultIds(trace.root, trace.starts);
      overview.markResults(trace.source, results); trace.readResults = new Set(results);
      const expanded = new Map<string, Set<string>>();
      const draw = () => renderCausalTrace(page, trace, Date.now(), expanded, { session: idle, fold: draw });
      draw();
    } else if (route.v === 'sessions') {
      const rows = fixture.catalogRows;
      const descendants = (root: typeof rows[number]) => rows.filter((row) => {
        if (row.presentation_kind === 'Approval review') return false;
        const visited = new Set<string>(); let parent = row.parent;
        while (parent && !visited.has(parent)) {
          if (parent === root.key && row.machine?.key === root.machine?.key) return true;
          visited.add(parent); parent = rows.find((candidate) => candidate.key === parent && candidate.machine?.key === row.machine?.key)?.parent ?? null;
        }
        return false;
      });
      const items = rows.filter((row) => row.presentation_kind !== 'Approval review')
        .sort((a, b) => (b.last ?? 0) - (a.last ?? 0) || compareCatalogKeys(a.key, b.key) || compareCatalogKeys(a.machine?.key ?? '', b.machine?.key ?? ''))
        .map((row) => ({ ...row, id: catalogSessionKey(row), presentation: catalogSessionRow(row, row.parent ? [] : descendants(row)) }));
      const catalogPage = fixture.catalogPage, capabilities = fixture.capabilities;
      if (!catalogPage || !capabilities) throw Error('Missing native Catalog publication');
      const observation = { cached: 'Cached history', updating: 'Discovering history', stale: 'History observation is stale', incomplete: 'History is incomplete', unavailable: 'History observation is unavailable' }[catalogPage.freshness];
      const sourceLabel = !capabilities.source_key && capabilities.global_union ? 'All machines' : fixture.label(capabilities.source_key);
      const filter = catalogFilterDescription(sourceLabel, observation, capabilities.metadata_search, capabilities.global_union);
      catalogBrowser.update({ harness: '', repo: '', model: '', q: '' }, rows, capabilities.source_key, filter, [], false);
      renderCatalogList(page, { items, sourceLabel: catalogPage.machine_info.label ?? (capabilities.source_key || 'This source'), updating: false, note: '',
        harness: '', repo: '', model: '', query: '', more: catalogPage.next_cursor !== null, metadataSearch: capabilities.metadata_search, globalUnion: capabilities.global_union,
        completeCount: completeCatalogCount({ page: catalogPage, items: rows,
          witness: catalogCountWitness(catalogPage, JSON.stringify(['', '', '', '']), true),
          source: capabilities.source_key, filters: JSON.stringify(['', '', '', '']), searchRequested: false,
          updating: false, more: catalogPage.next_cursor !== null }),
        filterDescription: filter.description, searchQualification: filter.searchQualification,
        groups: catalogSessionGroups(items).map((group) => ({ ...group, expanded: false })),
        reviews: false, hasReviews: rows.some((row) => row.presentation_kind === 'Approval review'),
        observation, observationState: catalogPage.freshness, discovering: catalogPage.completeness.state === 'partial',
        searchPartial: catalogPage.search?.partial ?? false, searchIndexIncomplete: catalogPage.search ? !catalogPage.search.index_complete : false,
        browser: { facets: catalogBrowser.element, groupBy: 'recent',
          working: items.filter((row) => row.status?.state === 'work').length,
          waiting: items.filter((row) => row.status?.state === 'wait').length, groups: [{ title: '', items }] },
      }, { session: idle, filters: idle, clearFilters: idle, more: idle, retry: idle });
    }
    updateChrome();
    owner.dataset.fixture = scope; owner.dataset.frame = frame;
    void overview.refresh().then(() => { owner.dataset.nativeReady = 'true'; });
    return () => {
      panel?.destroy(); for (const dialog of dialogs) dialog.destroy();
      overview.destroy(); effects.destroy(); catalogBrowser.destroy();
      if (selectedRoot) {
        releaseScreen(selectedRoot); render(null, selectedRoot); selectedRoot.remove();
      } else {
        releaseScreen(page); render(null, page);
      }
      render(null, title); render(null, sources);
      render(null, traceActions.firstElementChild ?? traceActions);
      recent.destroy(); bar.destroy(); chrome.destroy();
    };
  }, [frame, scope]);
  return <div class="app" ref={app} data-storybook-route="legacy" />;
}
