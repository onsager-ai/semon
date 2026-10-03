import type { AccountChromeHost } from '../lib';
import { createAccountChrome, createShellChrome, setGeometry } from '../lib';
interface AccountControlsHost {
  navigation: import('../navigation/routes').NavigationController;
  currentScroll: () => number;
  accountSheet: boolean;
  disposed: boolean;
  toolViewsOwner: ReturnType<typeof import('./toolViews').createToolViews>;
  LIVE: import('../lib/live').LiveState;
  scope: import('../app/effects').EffectScope;
  refresh: (dirty?: ReadonlySet<string> | null) => void;
  SIDEBAR_ONLY: boolean;
  go: (
    r: import('../navigation/routes').ApplicationRoute,
    fromHistory?: boolean,
    prepared?: boolean,
    nextContent?: import('../viewer-host').ViewerContent | null,
  ) => void;
  orderApply: (name: string) => void;
  phone: MediaQueryList;
  ORD_DRAWER_MS: number;
  setRailMode: (on: boolean) => void;
  layoutOwner: ReturnType<typeof import('./layout').createLayout>;
  renderNav: () => void;
  app: HTMLElement;
  afterPop: (() => void) | null;
  ACCOUNT: import('../lib/account').Account | null;
  setWideMode: (on: boolean) => void;
  $: <T extends HTMLElement = HTMLElement>(s: string, r?: ParentNode) => T;
}
/** Owns accountControls behavior through explicit application ports. */
export function createAccountControls(host: AccountControlsHost) {
  const accountHost: AccountChromeHost = {
    place(widget, trigger) {
      const at = trigger.getBoundingClientRect();
      setGeometry(widget, 'accountLeft', at.left);
      setGeometry(widget, 'accountWidth', at.width);
      setGeometry(widget, 'accountBottom', Math.max(0, innerHeight - at.top + 6));
    },
    opened(compact) {
      if (compact)
        try {
          history.pushState(
            { ...host.navigation.route, sheet: 1, scrollTop: host.currentScroll() },
            '',
          );
          host.accountSheet = true;
        } catch {}
    },
    closed({ keepEntry, navigating }) {
      if (host.disposed) return;
      if (host.accountSheet) {
        host.accountSheet = false;
        if (!keepEntry && history.state?.sheet) {
          host.toolViewsOwner.skipPop = true;
          history.back();
        }
      }
      if (host.LIVE.pending && !navigating)
        host.scope.timeout(() => {
          if (host.LIVE.pending && !host.toolViewsOwner.viewerEl && !accountChrome.open)
            host.refresh();
        }, 0);
    },
    navigate(href) {
      if (!host.accountSheet) return false;
      leaveAccountSheet(() => location.assign(href));
      return true;
    },
    submit(form) {
      if (!host.accountSheet) return false;
      leaveAccountSheet(() => form.submit());
      return true;
    },
  };
  const shellChrome = host.SIDEBAR_ONLY
    ? null
    : createShellChrome({
        account: accountHost,
        navigate(destination) {
          host.go(host.navigation.historyRoute({ v: destination.key }, { v: 'home' }));
          return true;
        },
        drawerOpened() {
          host.orderApply('side');
        },
        drawerClosed() {
          host.scope.timeout(() => {
            if (host.phone.matches && !document.body.classList.contains('drawer-open'))
              host.orderApply('side');
          }, host.ORD_DRAWER_MS);
        },
        railChanged() {
          host.setRailMode(!host.layoutOwner.railMode);
          host.renderNav();
        },
      });
  if (shellChrome) shellChrome.mount(host.app);
  const accountChrome = shellChrome?.account ?? createAccountChrome(accountHost);
  function closeAccountMenu(
    keepEntry: boolean | undefined = undefined,
    navigating: boolean | undefined = undefined,
  ) {
    accountChrome.close({ keepEntry, navigating });
  }
  // Step over the phone sheet before leaving so Back lands on the page, not a removed menu.
  function leaveAccountSheet(go: () => void) {
    host.accountSheet = false;
    if (history.state?.sheet) {
      host.toolViewsOwner.skipPop = true;
      host.afterPop = go;
      history.back();
    } else go();
  }
  function accountWidget(compact: boolean) {
    return host.ACCOUNT
      ? accountChrome.mount({
          account: host.ACCOUNT,
          compact,
          wide: host.layoutOwner.wideMode,
          onWideChange: () => host.setWideMode(!host.layoutOwner.wideMode),
        })
      : null;
  }
  function renderDrawerAccount() {
    if (shellChrome) {
      shellChrome.drawerAccount(
        host.ACCOUNT
          ? {
              account: host.ACCOUNT,
              compact: true,
              wide: host.layoutOwner.wideMode,
              onWideChange: () => host.setWideMode(!host.layoutOwner.wideMode),
            }
          : null,
      );
      return;
    }
    const old = host.$('#account-drawer');
    if (old) {
      accountChrome.unmount(old);
      old.remove();
    }
    if (!host.ACCOUNT) return;
    const widget = accountWidget(true);
    if (!widget) return;
    widget.id = 'account-drawer';
    host.$('#sidebar').append(widget);
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

  return { shellChrome, accountChrome, closeAccountMenu, renderDrawerAccount };
}
