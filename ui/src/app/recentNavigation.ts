import { HARNESS } from './registry';
import { I } from './registry';
import { HARNESSES } from './registry';
import { STATE } from './registry';
import type { ViewerModelStore } from '../state/model';
import type { NavigationController } from '../navigation/routes';
import type { EffectScope } from './effects';
import type { createDomain } from '../domain/calculations';
import type { createOrderingControls } from './orderingControls';
import type { createDestination } from './destination';
import type { createLayout } from './layout';
import type { createPaging } from './paging';
import type { createNavigationView } from './navigationView';
import type { createToolViews } from './toolViews';
import type { createHistoryScroll } from './historyScroll';
import type { createLiveModel } from './liveModel';
import type { createApplicationRefresh } from './applicationRefresh';
import type { createSessionChrome } from './sessionChrome';
import { shortModel } from '../domain/format';
import type { Session } from '../domain/types';
import type { OrderScope, RecentItem, RecentSnapshot } from '../lib';
import { createKidsSheet, createRecentRenderer, setGeometry } from '../lib';
import type { ApplicationRoute } from '../navigation/routes';
interface TreePref {
  open: boolean;
  at: number;
}
interface RecentNavigationHost {
  scope: EffectScope;
  showApprovalReviews: boolean;
  navigation: NavigationController;
  $: <T extends HTMLElement = HTMLElement>(s: string, r?: ParentNode) => T;
  phone: MediaQueryList;
  ago: (t: number) => string;
  pendingSessionOpen: string | null;
  sidebarOnly: boolean;
  disposed: boolean;
  dialogs: Map<HTMLDialogElement, { destroy(): void }>;
  dur: (a: number, b: number | null | undefined) => string;
  darkTheme: () => boolean;
  query: string;

  sessionChrome: Pick<ReturnType<typeof createSessionChrome>, 'modelIdOf'>;

  applicationRefreshOwner: Pick<ReturnType<typeof createApplicationRefresh>, 'refresh'>;

  liveModelOwner: Pick<ReturnType<typeof createLiveModel>, 'LIVE'>;

  historyScrollOwner: Pick<ReturnType<typeof createHistoryScroll>, 'currentScroll'>;

  toolViewsOwner: Pick<ReturnType<typeof createToolViews>, 'viewerEl' | 'skipPop'>;

  navigationViewOwner: Pick<ReturnType<typeof createNavigationView>, 'sessMatch'>;

  pagingOwner: Pick<ReturnType<typeof createPaging>, 'scrollProgrammatically'>;

  layoutOwner: Pick<ReturnType<typeof createLayout>, 'saveTreePref' | 'treePrefs' | 'railMode'>;

  destination: Pick<ReturnType<typeof createDestination>, 'goSession'>;

  orderingControlsOwner: Pick<
    ReturnType<typeof createOrderingControls>,
    'byLast' | 'orderList' | 'ORD' | 'ordState' | 'orderScope' | 'ordIdle' | 'ordIdleArm'
  >;

  modelStore: Pick<ViewerModelStore, 'sessions' | 'machines'>;

  domain: Pick<
    ReturnType<typeof createDomain>,
    | 'sessionChildren'
    | 'parentOf'
    | 'kidRank'
    | 'descendantsOf'
    | 'defaultTreeOpen'
    | 'childParts'
    | 'shortHost'
    | 'hostOf'
    | 'urgentDescendant'
  >;
}
/** Owns recentNavigation behavior through explicit application ports. */
export function createRecentNavigation(host: RecentNavigationHost) {
  const isApprovalReview = (s: Session) => s.kind === 'Approval review';
  const visibleInNavigation = (s: Session, path = routedPath()) =>
    host.showApprovalReviews ||
    !isApprovalReview(s) ||
    s.id === path.current ||
    path.ancestors.has(s.id);
  // Hide review rows while forwarding their visible descendants to the nearest shown parent; roots are promoted the same way.
  const navigationTree = (path = routedPath()) => {
    const canonical = host.domain.sessionChildren(),
      roots: Session[] = [],
      children = new Map<string, Session[]>(),
      visited = new Set<string>();
    const visit = (session: Session, parent: string | null) => {
      if (visited.has(session.id)) return;
      visited.add(session.id);
      let nearest = parent;
      if (visibleInNavigation(session, path)) {
        if (parent) {
          if (!children.has(parent)) children.set(parent, []);
          children.get(parent)!.push(session);
        } else roots.push(session);
        nearest = session.id;
      }
      for (const child of canonical.get(session.id) ?? []) visit(child, nearest);
    };
    for (const root of Object.values(host.modelStore.sessions).filter(
      (s) => s.lane && !host.domain.parentOf(s.id),
    ))
      visit(root, null);
    roots.sort((...args: Parameters<typeof host.orderingControlsOwner.byLast>) =>
      host.orderingControlsOwner.byLast(...args),
    );
    for (const kids of children.values())
      kids.sort((...args: Parameters<typeof host.orderingControlsOwner.byLast>) =>
        host.orderingControlsOwner.byLast(...args),
      );
    return { roots, children };
  };
  // Children in the order their handoffs were sent; the sidebar list stays newest-first.
  const COST_TIP =
    "What these tokens would cost at API rates. Subscriptions (Claude Max, ChatGPT plans) aren't billed this way.";
  // The icon is the only place this text is; it takes keyboard focus so the tip is reachable without a pointer.
  // An open parent lists its waiting children, then its running ones (at most 8), then the newest finished ones until three rows are
  // listed. "All N" opens the rest: a sheet on a phone, the whole list in the tree on a wide screen.
  const TREE_ACTIVE = 8,
    TREE_ROWS = 3;
  // The open session and the sessions above it. Only the open one is marked current; its ancestors are opened in the tree for this render
  // (nothing is saved) and are always listed, so the current row can always be found.
  function routedPath() {
    const current =
        host.navigation.route.v === 'session'
          ? 'id' in host.navigation.route
            ? host.navigation.route.id
            : ''
          : host.navigation.route.v === 'trace'
            ? host.navigation.route.sid
            : null,
      ancestors = new Set<string>();
    for (
      let id = current && host.modelStore.sessions[current] ? host.domain.parentOf(current) : null;
      id && host.modelStore.sessions[id] && id !== current && !ancestors.has(id);
      id = host.domain.parentOf(id)
    )
      ancestors.add(id);
    return { current, ancestors };
  }
  // Those ancestors open once per navigation, held in memory: a parent collapsed after that stays collapsed until the next one.
  let forcedOpen: { route: ApplicationRoute | null; ids: Set<string> } = {
    route: null,
    ids: new Set(),
  };
  function forcedOpenIds() {
    if (forcedOpen.route !== host.navigation.route)
      forcedOpen = { route: host.navigation.route, ids: routedPath().ancestors };
    return forcedOpen.ids;
  }
  // Waiting is 0, running 1, finished 2. A finished child with a waiting or running session below it ranks as that session does.
  // The one parent whose whole list is open in the tree (wide screens only). Nothing saves it: a reload starts with the short lists.
  // The parents above it stay listed and open (`expandedPath`) and everything below it is listed in full and open (`expandedUnder`), so
  // its "All N" and the rows it reveals agree.
  let expandedAll: string | null = null,
    revealedFor: ApplicationRoute | null = null;
  let expandedPath = new Set<string>(),
    expandedUnder = new Set<string>();
  let sideOrder: (OrderScope<ApplicationRoute> & { seen: Set<string> }) | null = null;
  const ancestorsOf = (id: string) => {
    const out = new Set<string>();
    for (
      let p = id && host.modelStore.sessions[id] ? host.domain.parentOf(id) : null;
      p && host.modelStore.sessions[p] && p !== id && !out.has(p);
      p = host.domain.parentOf(p)
    )
      out.add(p);
    return out;
  };
  // Fills a parent's group and says whether it holds the parent's whole list, which sticks the parent's row (stickRow).
  function treeGroupSnapshot(
    parent: Session,
    kids: Session[],
    children: ReadonlyMap<string, Session[]>,
    depth: number,
    rail: boolean,
    open: boolean,
  ): { items: RecentItem[]; all?: number; full: boolean } {
    const { current, ancestors } = routedPath(),
      rank = new Map(kids.map((c) => [c.id, host.domain.kidRank(c, children)]));
    const byRank = (a: Session, b: Session) => rank.get(a.id)! - rank.get(b.id)! || b.last - a.last,
      sorted = [...kids].sort(byRank),
      keep = new Set<string>();
    for (const c of sorted) if (rank.get(c.id)! < 2 && keep.size < TREE_ACTIVE) keep.add(c.id);
    for (const c of sorted)
      if (c.id === current || ancestors.has(c.id) || expandedPath.has(c.id)) keep.add(c.id);
    for (const c of sorted) if (keep.size < TREE_ROWS) keep.add(c.id);
    const listed = sorted.filter((c) => keep.has(c.id)),
      hidden = kids.length - listed.length;
    if (!hidden && expandedAll === parent.id) expandedAll = null; // nothing is left to open: "Show fewer" would have nothing to fold
    const full = hidden > 0 && !rail && (expandedAll === parent.id || expandedUnder.has(parent.id));
    // The rows keep the order they had (see "Stable order"); a child new to the list is held, unless it is the open session or leads to it.
    // A collapsed parent's children are nobody's to see: drawn sorted, not counted. A list new to a screen that has a snapshot is held
    // whole, with no "All N" row either, so that nothing appears in the tree.
    const key = (full ? 'a:' : 'k:') + parent.id,
      unseen =
        !!sideOrder?.keep &&
        open &&
        !full &&
        !sideOrder.reseed &&
        sideOrder.seen.has(parent.id) &&
        !sideOrder.prev.has(key);
    const shown = sideOrder
      ? host.orderingControlsOwner.orderList(sideOrder, key, full ? sorted : listed, byRank, {
          must: new Set([...(current ? [current] : []), ...ancestors, ...expandedPath]),
          quiet: !open,
          seed: full || sideOrder.reseed || !sideOrder.seen.has(parent.id),
        })
      : full
        ? sorted
        : listed;
    const items = shown.map((child) => buildLaneSnapshot(child, depth + 1, children, rail));
    return {
      items,
      all:
        kids.length === shown.length || full || unseen
          ? undefined
          : host.domain.descendantsOf(parent.id, children).length,
      full: full && expandedAll === parent.id,
    };
  }
  let recentSnapshot: RecentSnapshot = { items: [], empty: false };
  const recentRenderer = createRecentRenderer(host.$('#lanes'), {
    open: (id) => host.destination.goSession(id),
    toggle(id, value) {
      if (!value) forcedOpenIds().delete(id);
      host.layoutOwner.saveTreePref(id, value);
      if (!value && (expandedAll === id || expandedPath.has(id))) {
        expandedAll = null;
        renderLanes();
        host
          .$('#lanes .treeitem[data-id="' + CSS.escape(id) + '"] > .tree-row .tree-toggle')
          ?.focus();
      } else {
        const change = (items: readonly RecentItem[]): RecentItem[] =>
          items.map((item) => ({
            ...item,
            open: item.id === id ? value : item.open,
            children: item.children ? change(item.children) : undefined,
          }));
        recentSnapshot = { ...recentSnapshot, items: change(recentSnapshot.items) };
        recentRenderer.update(recentSnapshot);
      }
    },
    all(id, trigger) {
      if (!host.modelStore.sessions[id]) return;
      if (host.phone.matches) {
        openKidsSheet(host.modelStore.sessions[id], trigger);
        return;
      }
      expandedAll = id;
      renderLanes();
      host.$('#lanes .treeitem[data-id="' + CSS.escape(id) + '"] > .tree-row .tree-fewer')?.focus();
    },
    fewer(id) {
      expandedAll = null;
      renderLanes();
      const row = host.$('#lanes .srow[data-id="' + CSS.escape(id) + '"]');
      host.pagingOwner.scrollProgrammatically(() => row?.scrollIntoView({ block: 'nearest' }));
      row?.focus({ preventScroll: true });
    },
  });
  // A phone's "All N": every session below the parent in one sheet, waiting first, then running, then finished, newest first in each.
  function openKidsSheet(parent: Session, trigger: HTMLElement) {
    const all = host.domain.descendantsOf(parent.id, navigationTree().children),
      bucket = (s: Session) => (s.state === 'wait' ? 0 : s.state === 'work' ? 1 : 2);
    let picked: string | null = null;
    const rows = [...all]
      .sort((a, b) => b.last - a.last)
      .map((s) => {
        const above = host.domain.parentOf(s.id);
        return {
          id: s.id,
          name: s.name,
          state: s.state,
          stateLabel: STATE[s.state] ?? s.state,
          age: host.ago(s.last),
          bucket: bucket(s),
          under:
            above && above !== parent.id && host.modelStore.sessions[above]
              ? host.modelStore.sessions[above].name
              : undefined,
        };
      });
    const sheet = createKidsSheet(parent.name, rows, {
      matches(id, query) {
        return host.navigationViewOwner.sessMatch(host.modelStore.sessions[id], query);
      },
      select(id) {
        picked = id;
        host.pendingSessionOpen = id;
        sheet.dialog.close();
      },
      opened(d) {
        host.toolViewsOwner.viewerEl = d;
        document.documentElement.classList.add('viewer-open');
        if (!host.sidebarOnly)
          try {
            history.pushState(
              {
                ...host.navigation.route,
                sheet: 1,
                scrollTop: host.historyScrollOwner.currentScroll(),
              },
              '',
            );
          } catch {}
      },
      closed(d) {
        host.dialogs.delete(d);
        if (host.disposed) return;
        document.documentElement.classList.remove('viewer-open');
        if (host.toolViewsOwner.viewerEl === d) {
          host.toolViewsOwner.viewerEl = null;
          if (!host.sidebarOnly && history.state?.sheet) {
            host.toolViewsOwner.skipPop = true;
            history.back();
          } else if (host.pendingSessionOpen) {
            const id = host.pendingSessionOpen;
            host.pendingSessionOpen = null;
            host.destination.goSession(id);
          }
        }
        if (!picked)
          (host.$('#lanes .tree-all[data-id="' + CSS.escape(parent.id) + '"]') ?? trigger).focus();
        if (host.liveModelOwner.LIVE.pending) host.applicationRefreshOwner.refresh();
      },
    });
    host.dialogs.set(sheet.dialog, sheet);
    sheet.show();
  }
  function buildLaneSnapshot(
    s: Session,
    depth: number,
    children: ReadonlyMap<string, Session[]>,
    rail: boolean,
  ): RecentItem {
    const kids = children.get(s.id) ?? [],
      allKids = host.domain.descendantsOf(s.id, children);
    const { current, ancestors } = routedPath(),
      saved = host.layoutOwner.treePrefs[s.id];
    const open =
      forcedOpenIds().has(s.id) ||
      expandedPath.has(s.id) ||
      (typeof saved?.open === 'boolean'
        ? saved.open
        : host.domain.defaultTreeOpen(s.id, children) || expandedUnder.has(s.id));
    const group =
      kids.length && !rail ? treeGroupSnapshot(s, kids, children, depth, rail, open) : null;
    const parts = allKids.length ? host.domain.childParts(allKids) : [],
      harness = Object.hasOwn(HARNESSES, s.harness) ? HARNESSES[s.harness] : null;
    const flag =
      kids.length && !rail
        ? allKids.some((x) => x.state === 'wait')
          ? 'wait'
          : allKids.some((x) => x.state === 'err')
            ? 'err'
            : null
        : null;
    const fields: NonNullable<RecentItem['fields']>[number][] = [
      {
        className: 'row-duration',
        text: host.dur(s.start, s.state === 'work' || s.state === 'wait' ? null : s.last),
        priority: 1,
        tip: 'Duration',
        icon: I.duration,
      },
    ];
    if (Object.keys(host.modelStore.machines).length > 1)
      fields.push({
        className: 'row-machine host',
        text: host.domain.shortHost(s),
        priority: 2,
        tip: 'Machine: ' + host.domain.hostOf(s),
        icon: I.machine,
      });
    if (s.repo)
      fields.push({
        className: 'repo-short',
        text: s.repo,
        priority: 3,
        tip: 'Repo: ' + s.repo,
        icon: I.repo,
      });
    return {
      id: s.id,
      name: s.name,
      label: [
        s.name,
        STATE[s.state] ?? s.state,
        HARNESS[s.harness] ?? s.harness,
        host.domain.shortHost(s),
        ...parts,
      ].join(', '),
      state: s.state,
      stateLabel: STATE[s.state] ?? s.state,
      age: host.ago(s.last),
      model: shortModel(s.model ?? s.modelId),
      modelTip: 'Model: ' + host.sessionChrome.modelIdOf(s),
      harness: harness
        ? {
            id: s.harness,
            name: harness.name,
            light: harness.icon.light,
            dark: harness.icon.dark,
            darkTheme: host.darkTheme(),
          }
        : undefined,
      fields,
      current: current === s.id ? 'page' : rail && ancestors.has(s.id) ? 'true' : undefined,
      rail,
      childState:
        rail && allKids.some((x) => x.state === 'work' || x.state === 'wait')
          ? (host.domain.urgentDescendant(s.id, children) ?? 'work')
          : undefined,
      flag: flag
        ? {
            state: flag,
            tip:
              allKids.filter((x) => x.state === 'wait').length +
              ' needs you · ' +
              allKids.filter((x) => x.state === 'err').length +
              ' failed',
          }
        : undefined,
      open,
      depth,
      children: group?.items,
      all: group?.all,
      stuck: !!group?.full && open,
    };
  }
  // The focused control in the tree, so a redraw (a live update, a fold) can put focus back on it or, failing that, on its parent's row.
  function laneFocus() {
    const a = document.activeElement instanceof HTMLElement ? document.activeElement : null,
      item = a?.closest<HTMLElement>('.treeitem');
    if (!a || !host.$('#lanes').contains(a)) return null;
    const kind =
      ['srow', 'tree-all', 'tree-fewer', 'tree-toggle'].find((c) => a.classList.contains(c)) ??
      (a === item ? 'treeitem' : null);
    const id =
      kind === 'tree-toggle'
        ? a.dataset.treeToggle
        : kind === 'srow' || kind === 'tree-all'
          ? a.dataset.id
          : item?.dataset.id;
    return kind && id ? { kind, id, visible: a.matches(':focus-visible') } : null;
  }
  function restoreLaneFocus(f: ReturnType<typeof laneFocus>) {
    if (!f || document.activeElement !== document.body) return;
    const q = (sel: string) => host.$('#lanes ' + sel),
      id = CSS.escape(f.id);
    const target =
      f.kind === 'srow'
        ? q('.srow[data-id="' + id + '"]')
        : f.kind === 'tree-all'
          ? q('.tree-all[data-id="' + id + '"]')
          : f.kind === 'tree-fewer'
            ? q('.treeitem[data-id="' + id + '"] > .tree-row .tree-fewer')
            : f.kind === 'tree-toggle'
              ? q('.tree-toggle[data-tree-toggle="' + id + '"]')
              : q('.treeitem[data-id="' + id + '"]');
    (target ?? q('.srow[data-id="' + id + '"]'))?.focus({
      preventScroll: true,
      focusVisible: f.visible,
    });
  }
  function renderLanes() {
    const rail = host.layoutOwner.railMode && !host.phone.matches,
      prevSide = host.orderingControlsOwner.ORD.get('side');
    if (rail || prevSide?.rail !== rail) host.orderingControlsOwner.ORD.delete('side'); // the rail draws sorted, and so does a change into or out of it
    const sideState = host.orderingControlsOwner.ordState('side'),
      focus = laneFocus(),
      path = routedPath(),
      { current, ancestors } = path,
      { roots: lanes, children: everyone } = navigationTree(path);
    sideOrder = Object.assign(host.orderingControlsOwner.orderScope('side', '', null, sideState), {
      seen: new Set<string>(),
    });
    sideOrder.rail = rail;
    // The rows drawn now, before the list is emptied: which parents the reader can see, and which sessions were drawn at all.
    const box = host.$('#lanes'),
      drawn = [...box.querySelectorAll<HTMLElement>('.treeitem')];
    sideOrder.seen = new Set(drawn.map((r) => r.dataset.id!));
    const seeing = new Set(
      drawn
        .filter((r) => r.querySelector(':scope > .tree-row .srow')?.getClientRects().length)
        .map((r) => r.dataset.id),
    );
    // A session the reader can see, with no children in the last draw, whose first child arrives while the tree is held, stays as it
    // was: a toggle and a count on its row would move the rows below it. Its new children are held (and counted) as a whole. A parent
    // nobody sees, or one just put in the tree, shows its children with it.
    let children = everyone;
    if (sideOrder.keep) {
      const held = [...everyone.keys()].filter(
        (p) => !sideOrder!.prevKids.has(p) && seeing.has(p) && p !== current && !ancestors.has(p),
      );
      if (held.length) {
        children = new Map([...everyone].filter(([p]) => !held.includes(p)));
        sideOrder.n += held.reduce((n, p) => n + everyone.get(p)!.length, 0);
      }
    }
    sideOrder.kids = new Set(children.keys());
    if (
      expandedAll &&
      (host.phone.matches || host.layoutOwner.railMode || !host.modelStore.sessions[expandedAll])
    )
      expandedAll = null;
    // Opening or folding a parent's whole list, or crossing into or out of the rail, changes which lists are drawn: those the reader
    // just brought back are drawn sorted, not held as new.
    sideOrder.exp = JSON.stringify([expandedAll, host.layoutOwner.railMode && !host.phone.matches]);
    sideOrder.reseed = sideOrder.keep && sideOrder.prevExp !== sideOrder.exp;
    expandedPath = expandedAll ? ancestorsOf(expandedAll) : new Set<string>();
    expandedUnder = expandedAll
      ? new Set(host.domain.descendantsOf(expandedAll, children).map((x) => x.id))
      : new Set<string>();
    recentSnapshot = {
      items: host.orderingControlsOwner
        .orderList(
          sideOrder,
          'lanes',
          lanes,
          (...args: Parameters<typeof host.orderingControlsOwner.byLast>) =>
            host.orderingControlsOwner.byLast(...args),
          {
            limit: 8,
            must: new Set([...(current ? [current] : []), ...ancestors]),
          },
        )
        .slice(0, 8)
        .map((s) => buildLaneSnapshot(s, 0, children, rail)),
      empty: !lanes.length,
    };
    recentRenderer.update(recentSnapshot);
    if (!sideOrder.n) {
      host.scope.clearTimeout(host.orderingControlsOwner.ordIdle);
      host.orderingControlsOwner.ordIdle = undefined;
    } else if (!host.orderingControlsOwner.ordIdle) host.orderingControlsOwner.ordIdleArm(); // (counted from when something was first held)
    const q = host.$<HTMLInputElement>('#q');
    if (q && document.activeElement !== q) q.value = host.query;
    restoreLaneFocus(focus);
    // A stuck row covers the top of the sidebar: what is scrolled into view (the open session, after a navigation) stays clear of it.
    const stuck = box.querySelector<HTMLElement>('.tree-row.stuck'),
      navigated = revealedFor !== host.navigation.route;
    revealedFor = host.navigation.route;
    setGeometry(
      host.$('#side-list') ?? host.$('#sidebar'),
      'scrollPaddingTop',
      stuck ? stuck.offsetHeight + 8 : null,
    );
    if (stuck && navigated)
      host.pagingOwner.scrollProgrammatically(() =>
        box.querySelector('.srow[aria-current="page"]')?.scrollIntoView({ block: 'nearest' }),
      );
  }

  return {
    recentRenderer,
    get expandedAll() {
      return expandedAll;
    },
    set expandedAll(value: string | null) {
      expandedAll = value;
    },
    renderLanes,
    COST_TIP,
    isApprovalReview,
  };
}
