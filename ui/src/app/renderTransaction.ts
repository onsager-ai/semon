import type { DocumentRendererHost } from './documentRenderer';
import type { ApplicationRoute } from '../navigation/routes';
/** Route preparation precedes the synchronous page/bar commit; measurement follows it. */
export function createRenderTransaction(
  host: Pick<
    DocumentRendererHost,
    | 'pagingOwner'
    | 'accountControlsOwner'
    | 'viewport'
    | 'orderingControlsOwner'
    | 'transportOwner'
    | 'destination'
    | 'liveModelOwner'
    | 'layoutOwner'
    | 'sessionChrome'
    | 'navigationViewOwner'
    | 'recentNavigation'
  >,
) {
  return {
    begin() {
      host.pagingOwner.resetPagerInput();
      host.pagingOwner.holdProgrammaticScroll();
      host.accountControlsOwner.closeAccountMenu();
      host.viewport.stopOpeningEndPin();
      host.orderingControlsOwner.ordPageState = host.orderingControlsOwner.ordState('page');
      host.transportOwner.tick();
    },
    complete(r: ApplicationRoute) {
      const lanesKept =
        host.destination.lanesFor &&
        host.destination.lanesFor.r === r &&
        host.destination.lanesFor.version === host.liveModelOwner.LIVE.version;
      host.destination.lanesFor = null;
      host.layoutOwner.syncLayoutPrefs();
      host.sessionChrome.syncBarLine();
      host.navigationViewOwner.renderNav();
      if (!lanesKept) host.recentNavigation.renderLanes();
      host.accountControlsOwner.renderDrawerAccount();
      host.viewport.syncJump();
      host.orderingControlsOwner.ordPageState = null;
    },
  };
}
