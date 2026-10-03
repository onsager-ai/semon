interface TreePref {
  open: boolean;
  at: number;
}
interface LayoutHost {
  SIDEBAR_ONLY: boolean;
  $: <T extends HTMLElement = HTMLElement>(s: string, r?: ParentNode) => T;
  phone: MediaQueryList;
  navigation: import('../navigation/routes').NavigationController;
  accountChrome: import('../lib/account-chrome').AccountChrome;
  ORD: Map<
    string,
    import('../lib/ordering').OrderScope<import('../navigation/routes').ApplicationRoute>
  >;
  recentNavigation: {
    recentRenderer: import('../lib/recent').RecentRenderer;
    expandedAll: string | null;
    renderLanes: () => void;
    COST_TIP: string;
    isApprovalReview: (s: import('../domain/types').Session) => boolean;
  };
  renderLanes: () => void;
  scope: import('../app/effects').EffectScope;
}
/** Owns layout behavior through explicit application ports. */
export function createLayout(host: LayoutHost) {
  let wideMode = false,
    railMode = false;
  let treePrefs: Record<string, TreePref> = {};
  try {
    wideMode = localStorage.getItem('semon.wide') === '1';
  } catch {}
  try {
    railMode = !host.SIDEBAR_ONLY && localStorage.getItem('semon.rail') === '1';
  } catch {} // the rail is the viewer's own layout: an embedding page keeps its sidebar whole
  try {
    const saved = JSON.parse(localStorage.getItem('semon.tree') ?? '{}');
    if (saved && typeof saved === 'object' && !Array.isArray(saved))
      treePrefs = pruneTreePrefs(saved);
  } catch {}
  const app = host.$('.app');
  const syncLayoutPrefs = () => {
    if (host.SIDEBAR_ONLY) return;
    app.classList.toggle('rail', railMode && !host.phone.matches);
    host
      .$('#page')
      .classList.toggle(
        'wide-mode',
        wideMode && !host.phone.matches && host.navigation.route.v === 'session',
      );
  };
  function setWideMode(on: boolean) {
    wideMode = on;
    try {
      localStorage.setItem('semon.wide', on ? '1' : '0');
    } catch {}
    syncLayoutPrefs();
    host.$('.wide-toggle')?.setAttribute('aria-pressed', String(on));
    host.accountChrome.updateWide(on);
  }
  function setRailMode(on: boolean) {
    railMode = on;
    host.ORD.delete('side');
    try {
      localStorage.setItem('semon.rail', on ? '1' : '0');
    } catch {}
    syncLayoutPrefs();
    host.recentNavigation.expandedAll = null;
    host.renderLanes();
    const b = host.$('#rail-toggle');
    b?.setAttribute('aria-expanded', String(!on));
    b?.setAttribute('aria-label', on ? 'Expand sidebar' : 'Collapse sidebar');
    b?.setAttribute('data-tip', on ? 'Expand sidebar' : 'Collapse sidebar');
  }
  // A parent's saved choice is whether it is `open`. Saves from before the sidebar's "All N" row also held `more`, which nothing reads now:
  // it is dropped on load, along with any entry that has no `open`, and the next save writes the pruned list.
  function pruneTreePrefs(saved: unknown) {
    const kept: Record<string, TreePref> = {};
    if (!saved || typeof saved !== 'object') return kept;
    for (const [id, pref] of Object.entries(saved))
      if (pref && typeof pref === 'object' && 'open' in pref && typeof pref.open === 'boolean')
        kept[id] = { open: pref.open, at: 'at' in pref ? Number(pref.at) || 0 : 0 };
    return kept;
  }
  function saveTreePref(id: string, open: boolean) {
    treePrefs[id] = { open, at: Date.now() };
    treePrefs = Object.fromEntries(
      Object.entries(treePrefs)
        .sort((a, b) => (b[1]?.at ?? 0) - (a[1]?.at ?? 0))
        .slice(0, 500),
    );
    try {
      localStorage.setItem('semon.tree', JSON.stringify(treePrefs));
    } catch {}
  }
  // An embedding page's sidebar has no rail and no toggle for it (shell::session_sidebar): the toggle is then a detached button.
  const railToggle = host.SIDEBAR_ONLY
    ? (host.$('#rail-toggle') ?? document.createElement('button'))
    : document.createElement('button');
  railToggle.setAttribute('aria-expanded', String(!railMode));
  railToggle.setAttribute('data-tip', railMode ? 'Expand sidebar' : 'Collapse sidebar');
  railToggle.setAttribute('aria-label', railMode ? 'Expand sidebar' : 'Collapse sidebar');
  host.scope.listen(railToggle, 'click', () => setRailMode(!railMode));
  syncLayoutPrefs();

  return {
    app,
    setRailMode,
    get railMode() {
      return railMode;
    },
    set railMode(value: boolean) {
      railMode = value;
    },
    get wideMode() {
      return wideMode;
    },
    set wideMode(value: boolean) {
      wideMode = value;
    },
    setWideMode,
    saveTreePref,
    get treePrefs() {
      return treePrefs;
    },
    set treePrefs(value: Record<string, TreePref>) {
      treePrefs = value;
    },
    syncLayoutPrefs,
  };
}
