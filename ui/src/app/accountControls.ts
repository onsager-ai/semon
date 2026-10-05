import type { ViewerHost } from '../viewer-host';
import type { Account } from '../lib/account';
import type { EffectScope } from '../app/effects';
import type { createToolViews } from './toolViews';
import type { NavigationController } from '../navigation/routes';
import type { createHistoryScroll } from './historyScroll';
import type { createLiveModel } from './liveModel';
import type { createApplicationRefresh } from './applicationRefresh';
import type { createDestination } from './destination';
import type { createOrderingControls } from './orderingControls';
import type { createLayout } from './layout';
import type { createNavigationView } from './navigationView';
import type { AccountChromeHost } from '../lib';
import { createAccountChrome, createShellChrome, setGeometry } from '../lib';
interface AccountControlsHost {
  viewerHost: ViewerHost | null;
  navigation: NavigationController;
  accountSheet: boolean;
  disposed: boolean;
  toolViewsOwner: ReturnType<typeof createToolViews>;
  scope: EffectScope;
  sidebarOnly: boolean;
  phone: MediaQueryList;
  afterPop: (() => void) | null;
  account: Account | null;
  $: <T extends HTMLElement = HTMLElement>(s: string, r?: ParentNode) => T;

  navigationViewOwner: Pick<ReturnType<typeof createNavigationView>, 'renderNav'>;

  layoutOwner: Pick<
    ReturnType<typeof createLayout>,
    'setRailMode' | 'app' | 'setWideMode' | 'railMode' | 'wideMode'
  >;

  orderingControlsOwner: Pick<
    ReturnType<typeof createOrderingControls>,
    'orderApply' | 'ORD_DRAWER_MS'
  >;

  destination: Pick<ReturnType<typeof createDestination>, 'go'>;

  applicationRefreshOwner: Pick<ReturnType<typeof createApplicationRefresh>, 'refresh'>;

  liveModelOwner: Pick<ReturnType<typeof createLiveModel>, 'LIVE'>;

  historyScrollOwner: Pick<ReturnType<typeof createHistoryScroll>, 'currentScroll'>;
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
            {
              ...host.navigation.route,
              sheet: 1,
              scrollTop: host.historyScrollOwner.currentScroll(),
            },
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
      if (host.liveModelOwner.LIVE.pending && !navigating)
        host.scope.timeout(() => {
          if (
            host.liveModelOwner.LIVE.pending &&
            !host.toolViewsOwner.viewerEl &&
            !accountChrome.open
          )
            host.applicationRefreshOwner.refresh();
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
  const shellChrome = host.sidebarOnly
    ? null
    : createShellChrome({
        account: accountHost,
        navigate(destination) {
          if (host.viewerHost?.nativeNavigation?.some((item) => item.key === destination.key))
            return false;
          host.destination.go(host.navigation.historyRoute({ v: destination.key }, { v: 'home' }));
          return true;
        },
        drawerOpened() {
          host.orderingControlsOwner.orderApply('side');
        },
        drawerClosed() {
          host.scope.timeout(() => {
            if (host.phone.matches && !document.body.classList.contains('drawer-open'))
              host.orderingControlsOwner.orderApply('side');
          }, host.orderingControlsOwner.ORD_DRAWER_MS);
        },
        railChanged() {
          host.layoutOwner.setRailMode(!host.layoutOwner.railMode);
          host.navigationViewOwner.renderNav();
        },
      });
  if (shellChrome) shellChrome.mount(host.layoutOwner.app);
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
    return host.account
      ? accountChrome.mount({
          account: host.account,
          compact,
          wide: host.layoutOwner.wideMode,
          onWideChange: () => host.layoutOwner.setWideMode(!host.layoutOwner.wideMode),
        })
      : null;
  }
  function renderDrawerAccount() {
    if (shellChrome) {
      shellChrome.drawerAccount(
        host.account
          ? {
              account: host.account,
              compact: true,
              wide: host.layoutOwner.wideMode,
              onWideChange: () => host.layoutOwner.setWideMode(!host.layoutOwner.wideMode),
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
    if (!host.account) return;
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
