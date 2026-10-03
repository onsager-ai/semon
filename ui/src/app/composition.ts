import { createDomain } from "../domain/calculations";
import { ago as formatAgo,clock as formatClock,dur as formatDuration } from "../domain/format";
import type { Entry,Session,TranscriptMeta } from '../domain/types';
import { releaseGeometry,releaseScreen } from "../lib";
import type { ApplicationRoute } from '../navigation/routes';
import { NavigationController } from "../navigation/routes";
import { ViewerModelStore } from "../state/model";
import { TranscriptStore } from "../state/transcript";
import type { ViewerHost } from '../viewer-host';
import { createAccountControls } from './accountControls';
import { createAnalytics } from './analytics';
import { createApplicationRefresh } from './applicationRefresh';
import { createBootstrap } from './bootstrap';
import { createDestination } from './destination';
import { createDocumentEvents } from './documentEvents';
import { createDocumentRenderer } from './documentRenderer';
import { EffectScope } from './effects';
import { createHistoryScroll } from './historyScroll';
import { createLayout } from './layout';
import { createLiveModel } from './liveModel';
import { createLiveUpdates } from './liveUpdates';
import { createNavigationView } from './navigationView';
import { createOrderingControls } from './orderingControls';
import { createPaging } from './paging';
import { createRecentNavigation } from './recentNavigation';
import { HARNESS,HARNESSES,HARNESS_SHORT,I,STATE } from './registry';
import { createScreenViews } from './screenViews';
import { createSeenPersistence } from './seenPersistence';
import { createSeenResults } from './seenResults';
import { createSentences } from './sentences';
import { createSessionChrome } from './sessionChrome';
import { createSessionList } from './sessionList';
import { createTicker } from './ticker';
import { createToolLoader } from './toolLoader';
import { createToolViews } from './toolViews';
import { createTranscriptRevalidation } from './transcriptRevalidation';
import { createTranscriptView } from './transcriptView';
import { createTransport } from './transport';
import type { ViewerApplication } from './viewer';
import { createViewport } from './viewport';
/** The document composition owns services; focused factories own behavior and mutable feature state. */
export class ViewerComposition {
  readonly scope: import("../app/effects").EffectScope;
  readonly dialogs: Map<HTMLDialogElement, { destroy(): void; }>;
  disposed: boolean;
  readonly application: import("../app/viewer").ViewerApplication;
  NOW: number;
  readonly modelStore: import("../state/model").ViewerModelStore;
  readonly MACHINE: Record<string, string>;
  readonly MACHINE_UP: Record<string, boolean>;
  readonly MACHINE_LAST: Record<string, number>;
  ADMIN: { href: string; label: string; } | null;
  ACCOUNT: import("../lib/account").Account | null;
  readonly viewerHost: import("../viewer-host").ViewerHost | null;
  readonly NATIVE_PAGE: { title: string; nav: string; } | undefined;
  NAV_MACHINES: string | null;
  readonly SIDEBAR_ONLY: boolean;
  readonly SESS: Record<string, import("../domain/types").Session>;
  readonly H: import("../domain/types").Handoff[];
  readonly transcripts: import("../state/transcript").TranscriptStore;
  readonly TX: Record<string, import("../domain/types").Entry[]>;
  readonly seenResultsOwner: ReturnType<typeof createSeenResults>;
  readonly HID: Map<string, import("../domain/types").Handoff>;
  readonly TURNS: Record<string, import("../domain/types").Turn[]>;
  readonly TURN: Map<string, import("../domain/types").Turn>;
  readonly STARTS: Map<string, import("../domain/types").Turn>;
  readonly HOLDS: Map<string, import("../domain/types").Turn>;
  readonly TXM: Record<string, import("../domain/types").TranscriptMeta>;
  readonly domain: ReturnType<typeof createDomain>;
  readonly nameOf: ViewerComposition['domain']['nameOf'] = (...args) => this.domain.nameOf(...args);
  readonly hcls: ViewerComposition['domain']['hcls'] = (...args) => this.domain.hcls(...args);
  readonly where: ViewerComposition['domain']['where'] = (...args) => this.domain.where(...args);
  readonly hostOf: ViewerComposition['domain']['hostOf'] = (...args) => this.domain.hostOf(...args);
  readonly machineLabels: ViewerComposition['domain']['machineLabels'] = (...args) => this.domain.machineLabels(...args);
  readonly machineLabel: ViewerComposition['domain']['machineLabel'] = (...args) => this.domain.machineLabel(...args);
  readonly branchOf: ViewerComposition['domain']['branchOf'] = (...args) => this.domain.branchOf(...args);
  readonly shortHost: ViewerComposition['domain']['shortHost'] = (...args) => this.domain.shortHost(...args);
  readonly parentOf: ViewerComposition['domain']['parentOf'] = (...args) => this.domain.parentOf(...args);
  readonly originHandoff: ViewerComposition['domain']['originHandoff'] = (...args) => this.domain.originHandoff(...args);
  readonly isResult: ViewerComposition['domain']['isResult'] = (...args) => this.domain.isResult(...args);
  readonly inbox: ViewerComposition['domain']['inbox'] = (...args) => this.domain.inbox(...args);
  readonly working: ViewerComposition['domain']['working'] = (...args) => this.domain.working(...args);
  readonly answersOf: ViewerComposition['domain']['answersOf'] = (...args) => this.domain.answersOf(...args);
  readonly statWord: ViewerComposition['domain']['statWord'] = (...args) => this.domain.statWord(...args);
  readonly hasTurn: ViewerComposition['domain']['hasTurn'] = (...args) => this.domain.hasTurn(...args);
  readonly oneLine: ViewerComposition['domain']['oneLine'] = (...args) => this.domain.oneLine(...args);
  readonly turnEnd: ViewerComposition['domain']['turnEnd'] = (...args) => this.domain.turnEnd(...args);
  readonly traceRoot: ViewerComposition['domain']['traceRoot'] = (...args) => this.domain.traceRoot(...args);
  readonly countOf: ViewerComposition['domain']['countOf'] = (...args) => this.domain.countOf(...args);
  readonly callsText: ViewerComposition['domain']['callsText'] = (...args) => this.domain.callsText(...args);
  readonly sessionChildren: ViewerComposition['domain']['sessionChildren'] = (...args) => this.domain.sessionChildren(...args);
  readonly childSessions: ViewerComposition['domain']['childSessions'] = (...args) => this.domain.childSessions(...args);
  readonly descendantsOf: ViewerComposition['domain']['descendantsOf'] = (...args) => this.domain.descendantsOf(...args);
  readonly asMoney: ViewerComposition['domain']['asMoney'] = (...args) => this.domain.asMoney(...args);
  readonly usageTotal: ViewerComposition['domain']['usageTotal'] = (...args) => this.domain.usageTotal(...args);
  readonly costForSessions: ViewerComposition['domain']['costForSessions'] = (...args) => this.domain.costForSessions(...args);
  readonly costForSession: ViewerComposition['domain']['costForSession'] = (...args) => this.domain.costForSession(...args);
  readonly costText: ViewerComposition['domain']['costText'] = (...args) => this.domain.costText(...args);
  readonly costMissing: ViewerComposition['domain']['costMissing'] = (...args) => this.domain.costMissing(...args);
  readonly urgentDescendant: ViewerComposition['domain']['urgentDescendant'] = (...args) => this.domain.urgentDescendant(...args);
  readonly childParts: ViewerComposition['domain']['childParts'] = (...args) => this.domain.childParts(...args);
  readonly defaultTreeOpen: ViewerComposition['domain']['defaultTreeOpen'] = (...args) => this.domain.defaultTreeOpen(...args);
  readonly kidRank: ViewerComposition['domain']['kidRank'] = (...args) => this.domain.kidRank(...args);
  readonly lineageOf: ViewerComposition['domain']['lineageOf'] = (...args) => this.domain.lineageOf(...args);
  readonly byState: ViewerComposition['domain']['byState'] = (...args) => this.domain.byState(...args);
  readonly onMachine: ViewerComposition['domain']['onMachine'] = (...args) => this.domain.onMachine(...args);
  readonly movedOff: ViewerComposition['domain']['movedOff'] = (...args) => this.domain.movedOff(...args);
  readonly movesOf: ViewerComposition['domain']['movesOf'] = (...args) => this.domain.movesOf(...args);
  readonly shortMoney: ViewerComposition['domain']['shortMoney'] = (...args) => this.domain.shortMoney(...args);
  readonly clock: (t: number) => string;
  readonly ago: (t: number) => string;
  readonly dur: (a: number, b: number | null | undefined) => string;
  readonly $: <T extends HTMLElement = HTMLElement>(s: string, r?: ParentNode) => T;
  readonly spaced: (t: unknown) => string;
  readonly darkTheme: () => boolean;
  readonly facetLine: (s: import("../domain/types").Session) => string;
  readonly seenPersistenceOwner: ReturnType<typeof createSeenPersistence>;
  readonly markSeenResults: ViewerComposition['seenPersistenceOwner']['markSeenResults'] = (...args) => this.seenPersistenceOwner.markSeenResults(...args);
  readonly sentencesOwner: ReturnType<typeof createSentences>;
  readonly sentenceSnapshot: ViewerComposition['sentencesOwner']['sentenceSnapshot'] = (...args) => this.sentencesOwner.sentenceSnapshot(...args);
  readonly isGap: (e: import("../domain/types").Entry) => boolean;
  readonly transportOwner: ReturnType<typeof createTransport>;
  readonly api: ViewerComposition['transportOwner']['api'] = (...args) => this.transportOwner.api(...args);
  readonly txEntry: ViewerComposition['transportOwner']['txEntry'] = (...args) => this.transportOwner.txEntry(...args);
  readonly fetchTx: ViewerComposition['transportOwner']['fetchTx'] = (...args) => this.transportOwner.fetchTx(...args);
  readonly enc: ViewerComposition['transportOwner']['enc'] = (...args) => this.transportOwner.enc(...args);
  readonly adopt: ViewerComposition['transportOwner']['adopt'] = (...args) => this.transportOwner.adopt(...args);
  readonly load: ViewerComposition['transportOwner']['load'] = (...args) => this.transportOwner.load(...args);
  readonly spread: ViewerComposition['transportOwner']['spread'] = (...args) => this.transportOwner.spread(...args);
  readonly tick: ViewerComposition['transportOwner']['tick'] = (...args) => this.transportOwner.tick(...args);
  readonly accountOf: ViewerComposition['transportOwner']['accountOf'] = (...args) => this.transportOwner.accountOf(...args);
  readonly STALE_BRIEFS: Set<string>;
  readonly TXCACHE: import("../lib/routes").TranscriptCache<import("../domain/types").Entry, import("../domain/types").TranscriptMeta>;
  readonly cacheTx: (sid: string, entries: import("../domain/types").Entry[], meta: import("../domain/types").TranscriptMeta) => void;
  readonly adoptCached: (r: { v: "session"; id: string; turn?: string | undefined; } & { scrollTop?: number | undefined; hostFocus?: import("../navigation/routes").HostFocus | undefined; q?: string | undefined; sheet?: number | undefined; }) => boolean;
  readonly transcriptRevalidationOwner: ReturnType<typeof createTranscriptRevalidation>;
  readonly revalidate: ViewerComposition['transcriptRevalidationOwner']['revalidate'] = (...args) => this.transcriptRevalidationOwner.revalidate(...args);
  readonly shrank: ViewerComposition['transcriptRevalidationOwner']['shrank'] = (...args) => this.transcriptRevalidationOwner.shrank(...args);
  readonly pagingOwner: ReturnType<typeof createPaging>;
  readonly resetPagerInput: ViewerComposition['pagingOwner']['resetPagerInput'] = (...args) => this.pagingOwner.resetPagerInput(...args);
  readonly scrollProgrammatically: ViewerComposition['pagingOwner']['scrollProgrammatically'] = (...args) => this.pagingOwner.scrollProgrammatically(...args);
  readonly clearPaging: ViewerComposition['pagingOwner']['clearPaging'] = (...args) => this.pagingOwner.clearPaging(...args);
  readonly dropTx: ViewerComposition['pagingOwner']['dropTx'] = (...args) => this.pagingOwner.dropTx(...args);
  readonly loadPager: ViewerComposition['pagingOwner']['loadPager'] = (...args) => this.pagingOwner.loadPager(...args);
  readonly pagerSnapshot: ViewerComposition['pagingOwner']['pagerSnapshot'] = (...args) => this.pagingOwner.pagerSnapshot(...args);
  readonly holdProgrammaticScroll: ViewerComposition['pagingOwner']['holdProgrammaticScroll'] = (...args) => this.pagingOwner.holdProgrammaticScroll(...args);
  readonly queuePagerObservers: ViewerComposition['pagingOwner']['queuePagerObservers'] = (...args) => this.pagingOwner.queuePagerObservers(...args);
  readonly disconnectPagerObservers: ViewerComposition['pagingOwner']['disconnectPagerObservers'] = (...args) => this.pagingOwner.disconnectPagerObservers(...args);
  readonly paintPager: ViewerComposition['pagingOwner']['paintPager'] = (...args) => this.pagingOwner.paintPager(...args);
  readonly readerScrollInput: ViewerComposition['pagingOwner']['readerScrollInput'] = (...args) => this.pagingOwner.readerScrollInput(...args);
  readonly toolLoaderOwner: ReturnType<typeof createToolLoader>;
  readonly fullOf: ViewerComposition['toolLoaderOwner']['fullOf'] = (...args) => this.toolLoaderOwner.fullOf(...args);
  readonly bootstrapOwner: ReturnType<typeof createBootstrap>;
  readonly urlOf: ViewerComposition['bootstrapOwner']['urlOf'] = (...args) => this.bootstrapOwner.urlOf(...args);
  readonly routeOf: ViewerComposition['bootstrapOwner']['routeOf'] = (...args) => this.bootstrapOwner.routeOf(...args);
  readonly boot: ViewerComposition['bootstrapOwner']['boot'] = (...args) => this.bootstrapOwner.boot(...args);
  readonly phone: MediaQueryList;
  readonly navigation: import("../navigation/routes").NavigationController;
  readonly layoutOwner: ReturnType<typeof createLayout>;
  readonly setRailMode: ViewerComposition['layoutOwner']['setRailMode'] = (...args) => this.layoutOwner.setRailMode(...args);
  readonly setWideMode: ViewerComposition['layoutOwner']['setWideMode'] = (...args) => this.layoutOwner.setWideMode(...args);
  readonly saveTreePref: ViewerComposition['layoutOwner']['saveTreePref'] = (...args) => this.layoutOwner.saveTreePref(...args);
  readonly syncLayoutPrefs: ViewerComposition['layoutOwner']['syncLayoutPrefs'] = (...args) => this.layoutOwner.syncLayoutPrefs(...args);
  groupBy: string;
  query: string;
  focusSessionsSearchOnRender: import("../navigation/routes").ApplicationRoute | null;
  analyticsRange: number;
  analyticsMeasure: string;
  showApprovalReviews: boolean;
  readonly sessionFilters: { repo: string; machine: string; harness: string; model: string; };
  pendingSessionOpen: string | null;
  accountSheet: boolean;
  afterPop: (() => void) | null;
  readonly SHOW_ALL: { messages: boolean; tools: boolean; thinking: boolean; };
  show: { messages: boolean; tools: boolean; thinking: boolean; };
  find: string;
  findOpen: boolean;
  readonly historyScrollOwner: ReturnType<typeof createHistoryScroll>;
  readonly saveHistoryScroll: ViewerComposition['historyScrollOwner']['saveHistoryScroll'] = (...args) => this.historyScrollOwner.saveHistoryScroll(...args);
  readonly quietTop: ViewerComposition['historyScrollOwner']['quietTop'] = (...args) => this.historyScrollOwner.quietTop(...args);
  readonly restoreScroll: ViewerComposition['historyScrollOwner']['restoreScroll'] = (...args) => this.historyScrollOwner.restoreScroll(...args);
  readonly restoreHostFocus: ViewerComposition['historyScrollOwner']['restoreHostFocus'] = (...args) => this.historyScrollOwner.restoreHostFocus(...args);
  readonly currentScroll: ViewerComposition['historyScrollOwner']['currentScroll'] = (...args) => this.historyScrollOwner.currentScroll(...args);
  readonly destination: ReturnType<typeof createDestination>;
  readonly goSession: ViewerComposition['destination']['goSession'] = (...args) => this.destination.goSession(...args);
  readonly go: ViewerComposition['destination']['go'] = (...args) => this.destination.go(...args);
  readonly openSender: ViewerComposition['destination']['openSender'] = (...args) => this.destination.openSender(...args);
  readonly revealTurn: ViewerComposition['destination']['revealTurn'] = (...args) => this.destination.revealTurn(...args);
  readonly revealEntryHash: ViewerComposition['destination']['revealEntryHash'] = (...args) => this.destination.revealEntryHash(...args);
  readonly openSessionAtEnd: ViewerComposition['destination']['openSessionAtEnd'] = (...args) => this.destination.openSessionAtEnd(...args);
  readonly goTrace: ViewerComposition['destination']['goTrace'] = (...args) => this.destination.goTrace(...args);
  readonly accountControlsOwner: ReturnType<typeof createAccountControls>;
  readonly closeAccountMenu: ViewerComposition['accountControlsOwner']['closeAccountMenu'] = (...args) => this.accountControlsOwner.closeAccountMenu(...args);
  readonly renderDrawerAccount: ViewerComposition['accountControlsOwner']['renderDrawerAccount'] = (...args) => this.accountControlsOwner.renderDrawerAccount(...args);
  readonly orderingControlsOwner: ReturnType<typeof createOrderingControls>;
  readonly orderApply: ViewerComposition['orderingControlsOwner']['orderApply'] = (...args) => this.orderingControlsOwner.orderApply(...args);
  readonly byLast: ViewerComposition['orderingControlsOwner']['byLast'] = (...args) => this.orderingControlsOwner.byLast(...args);
  readonly orderList: ViewerComposition['orderingControlsOwner']['orderList'] = (...args) => this.orderingControlsOwner.orderList(...args);
  readonly ordState: ViewerComposition['orderingControlsOwner']['ordState'] = (...args) => this.orderingControlsOwner.ordState(...args);
  readonly orderScope: ViewerComposition['orderingControlsOwner']['orderScope'] = (...args) => this.orderingControlsOwner.orderScope(...args);
  readonly ordIdleArm: ViewerComposition['orderingControlsOwner']['ordIdleArm'] = (...args) => this.orderingControlsOwner.ordIdleArm(...args);
  readonly pageSig: ViewerComposition['orderingControlsOwner']['pageSig'] = (...args) => this.orderingControlsOwner.pageSig(...args);
  readonly pageState: ViewerComposition['orderingControlsOwner']['pageState'] = (...args) => this.orderingControlsOwner.pageState(...args);
  readonly navigationViewOwner: ReturnType<typeof createNavigationView>;
  readonly renderNav: ViewerComposition['navigationViewOwner']['renderNav'] = (...args) => this.navigationViewOwner.renderNav(...args);
  readonly sessMatch: ViewerComposition['navigationViewOwner']['sessMatch'] = (...args) => this.navigationViewOwner.sessMatch(...args);
  readonly recentNavigation: ReturnType<typeof createRecentNavigation>;
  readonly renderLanes: ViewerComposition['recentNavigation']['renderLanes'] = (...args) => this.recentNavigation.renderLanes(...args);
  readonly isApprovalReview: ViewerComposition['recentNavigation']['isApprovalReview'] = (...args) => this.recentNavigation.isApprovalReview(...args);
  readonly _SessionChrome: ReturnType<typeof createSessionChrome>;
  readonly drawSessionBar: ViewerComposition['_SessionChrome']['drawSessionBar'] = (...args) => this._SessionChrome.drawSessionBar(...args);
  readonly syncBarLine: ViewerComposition['_SessionChrome']['syncBarLine'] = (...args) => this._SessionChrome.syncBarLine(...args);
  readonly dropErrors: ViewerComposition['_SessionChrome']['dropErrors'] = (...args) => this._SessionChrome.dropErrors(...args);
  readonly modelIdOf: ViewerComposition['_SessionChrome']['modelIdOf'] = (...args) => this._SessionChrome.modelIdOf(...args);
  readonly kindText: ViewerComposition['_SessionChrome']['kindText'] = (...args) => this._SessionChrome.kindText(...args);
  readonly observeTitle: ViewerComposition['_SessionChrome']['observeTitle'] = (...args) => this._SessionChrome.observeTitle(...args);
  readonly centre: ViewerComposition['_SessionChrome']['centre'] = (...args) => this._SessionChrome.centre(...args);
  readonly panel: ViewerComposition['_SessionChrome']['panel'] = (...args) => this._SessionChrome.panel(...args);
  readonly renderTopbar: ViewerComposition['_SessionChrome']['renderTopbar'] = (...args) => this._SessionChrome.renderTopbar(...args);
  readonly machineLine: ViewerComposition['_SessionChrome']['machineLine'] = (...args) => this._SessionChrome.machineLine(...args);
  readonly sessionLine: ViewerComposition['_SessionChrome']['sessionLine'] = (...args) => this._SessionChrome.sessionLine(...args);
  readonly errOn: ViewerComposition['_SessionChrome']['errOn'] = (...args) => this._SessionChrome.errOn(...args);
  readonly markError: ViewerComposition['_SessionChrome']['markError'] = (...args) => this._SessionChrome.markError(...args);
  readonly errorsLive: ViewerComposition['_SessionChrome']['errorsLive'] = (...args) => this._SessionChrome.errorsLive(...args);
  readonly _ScreenViews: ReturnType<typeof createScreenViews>;
  readonly harnessSnapshot: ViewerComposition['_ScreenViews']['harnessSnapshot'] = (...args) => this._ScreenViews.harnessSnapshot(...args);
  readonly costSnapshot: ViewerComposition['_ScreenViews']['costSnapshot'] = (...args) => this._ScreenViews.costSnapshot(...args);
  readonly showsFooter: ViewerComposition['_ScreenViews']['showsFooter'] = (...args) => this._ScreenViews.showsFooter(...args);
  readonly renderHome: ViewerComposition['_ScreenViews']['renderHome'] = (...args) => this._ScreenViews.renderHome(...args);
  readonly renderMachines: ViewerComposition['_ScreenViews']['renderMachines'] = (...args) => this._ScreenViews.renderMachines(...args);
  readonly renderMachine: ViewerComposition['_ScreenViews']['renderMachine'] = (...args) => this._ScreenViews.renderMachine(...args);
  readonly renderTrace: ViewerComposition['_ScreenViews']['renderTrace'] = (...args) => this._ScreenViews.renderTrace(...args);
  readonly renderSession: ViewerComposition['_ScreenViews']['renderSession'] = (...args) => this._ScreenViews.renderSession(...args);
  readonly _TranscriptView: ReturnType<typeof createTranscriptView>;
  readonly transcriptEntries: ViewerComposition['_TranscriptView']['transcriptEntries'] = (...args) => this._TranscriptView.transcriptEntries(...args);
  readonly transcriptSnapshot: ViewerComposition['_TranscriptView']['transcriptSnapshot'] = (...args) => this._TranscriptView.transcriptSnapshot(...args);
  readonly verb: ViewerComposition['_TranscriptView']['verb'] = (...args) => this._TranscriptView.verb(...args);
  readonly verbNow: ViewerComposition['_TranscriptView']['verbNow'] = (...args) => this._TranscriptView.verbNow(...args);
  readonly toolViewsOwner: ReturnType<typeof createToolViews>;
  readonly openStepViewer: ViewerComposition['toolViewsOwner']['openStepViewer'] = (...args) => this.toolViewsOwner.openStepViewer(...args);
  readonly openScript: ViewerComposition['toolViewsOwner']['openScript'] = (...args) => this.toolViewsOwner.openScript(...args);
  readonly openImage: ViewerComposition['toolViewsOwner']['openImage'] = (...args) => this.toolViewsOwner.openImage(...args);
  readonly attachmentSnapshot: ViewerComposition['toolViewsOwner']['attachmentSnapshot'] = (...args) => this.toolViewsOwner.attachmentSnapshot(...args);
  readonly childSnapshot: ViewerComposition['toolViewsOwner']['childSnapshot'] = (...args) => this.toolViewsOwner.childSnapshot(...args);
  readonly footerSnapshot: ViewerComposition['toolViewsOwner']['footerSnapshot'] = (...args) => this.toolViewsOwner.footerSnapshot(...args);
  readonly documentRendererOwner: ReturnType<typeof createDocumentRenderer>;
  readonly render: ViewerComposition['documentRendererOwner']['render'] = (...args) => this.documentRendererOwner.render(...args);
  readonly slot: ViewerComposition['documentRendererOwner']['slot'] = (...args) => this.documentRendererOwner.slot(...args);
  readonly _Analytics: ReturnType<typeof createAnalytics>;
  readonly fetchAnalytics: ViewerComposition['_Analytics']['fetchAnalytics'] = (...args) => this._Analytics.fetchAnalytics(...args);
  readonly scheduleAnalytics: ViewerComposition['_Analytics']['scheduleAnalytics'] = (...args) => this._Analytics.scheduleAnalytics(...args);
  readonly refreshAnalytics: ViewerComposition['_Analytics']['refreshAnalytics'] = (...args) => this._Analytics.refreshAnalytics(...args);
  readonly renderAnalytics: ViewerComposition['_Analytics']['renderAnalytics'] = (...args) => this._Analytics.renderAnalytics(...args);
  readonly matchesSessionFacets: ViewerComposition['_Analytics']['matchesSessionFacets'] = (...args) => this._Analytics.matchesSessionFacets(...args);
  readonly renderFacetFilters: ViewerComposition['_Analytics']['renderFacetFilters'] = (...args) => this._Analytics.renderFacetFilters(...args);
  readonly sessionListOwner: ReturnType<typeof createSessionList>;
  readonly renderSessions: ViewerComposition['sessionListOwner']['renderSessions'] = (...args) => this.sessionListOwner.renderSessions(...args);
  readonly documentEventsOwner: ReturnType<typeof createDocumentEvents>;
  readonly closeDrawer: ViewerComposition['documentEventsOwner']['closeDrawer'] = (...args) => this.documentEventsOwner.closeDrawer(...args);
  readonly liveModelOwner: ReturnType<typeof createLiveModel>;
  readonly remember: ViewerComposition['liveModelOwner']['remember'] = (...args) => this.liveModelOwner.remember(...args);
  readonly schedule: ViewerComposition['liveModelOwner']['schedule'] = (...args) => this.liveModelOwner.schedule(...args);
  readonly visible: ViewerComposition['liveModelOwner']['visible'] = (...args) => this.liveModelOwner.visible(...args);
  readonly ended: ViewerComposition['liveModelOwner']['ended'] = (...args) => this.liveModelOwner.ended(...args);
  readonly handKey: ViewerComposition['liveModelOwner']['handKey'] = (...args) => this.liveModelOwner.handKey(...args);
  readonly cardKeys: ViewerComposition['liveModelOwner']['cardKeys'] = (...args) => this.liveModelOwner.cardKeys(...args);
  readonly viewed: ViewerComposition['liveModelOwner']['viewed'] = (...args) => this.liveModelOwner.viewed(...args);
  readonly soft: ViewerComposition['liveModelOwner']['soft'] = (...args) => this.liveModelOwner.soft(...args);
  readonly _LiveUpdates: ReturnType<typeof createLiveUpdates>;
  readonly reload: ViewerComposition['_LiveUpdates']['reload'] = (...args) => this._LiveUpdates.reload(...args);
  readonly tail: ViewerComposition['_LiveUpdates']['tail'] = (...args) => this._LiveUpdates.tail(...args);
  readonly applyModelDelta: ViewerComposition['_LiveUpdates']['applyModelDelta'] = (...args) => this._LiveUpdates.applyModelDelta(...args);
  readonly update: ViewerComposition['_LiveUpdates']['update'] = (...args) => this._LiveUpdates.update(...args);
  readonly applicationRefreshOwner: ReturnType<typeof createApplicationRefresh>;
  readonly refresh: ViewerComposition['applicationRefreshOwner']['refresh'] = (...args) => this.applicationRefreshOwner.refresh(...args);
  readonly viewport: ReturnType<typeof createViewport>;
  readonly stopOpeningEndPin: ViewerComposition['viewport']['stopOpeningEndPin'] = (...args) => this.viewport.stopOpeningEndPin(...args);
  readonly scroller: ViewerComposition['viewport']['scroller'] = (...args) => this.viewport.scroller(...args);
  readonly capture: ViewerComposition['viewport']['capture'] = (...args) => this.viewport.capture(...args);
  readonly restore: ViewerComposition['viewport']['restore'] = (...args) => this.viewport.restore(...args);
  readonly syncJump: ViewerComposition['viewport']['syncJump'] = (...args) => this.viewport.syncJump(...args);
  readonly startOpeningEndPin: ViewerComposition['viewport']['startOpeningEndPin'] = (...args) => this.viewport.startOpeningEndPin(...args);
  readonly clearNewEntries: ViewerComposition['viewport']['clearNewEntries'] = (...args) => this.viewport.clearNewEntries(...args);
  readonly edge: ViewerComposition['viewport']['edge'] = (...args) => this.viewport.edge(...args);
  readonly opener: ViewerComposition['viewport']['opener'] = (...args) => this.viewport.opener(...args);
  readonly jumpToLatest: ViewerComposition['viewport']['jumpToLatest'] = (...args) => this.viewport.jumpToLatest(...args);
  readonly patchSession: ViewerComposition['viewport']['patchSession'] = (...args) => this.viewport.patchSession(...args);
  readonly tickerOwner: ReturnType<typeof createTicker>;
  readonly ticker: ViewerComposition['tickerOwner']['ticker'] = (...args) => this.tickerOwner.ticker(...args);
  readonly running: ViewerComposition['tickerOwner']['running'] = (...args) => this.tickerOwner.running(...args);
  get SEEN_RESULTS(): ViewerComposition['seenResultsOwner']['SEEN_RESULTS'] { return this.seenResultsOwner.SEEN_RESULTS; }
  get SEEN_LIMIT(): ViewerComposition['seenResultsOwner']['SEEN_LIMIT'] { return this.seenResultsOwner.SEEN_LIMIT; }
  get SEEN_KEY(): ViewerComposition['seenResultsOwner']['SEEN_KEY'] { return this.seenResultsOwner.SEEN_KEY; }
  get RANK(): ViewerComposition['domain']['RANK'] { return this.domain.RANK; }
  get TOYOU(): ViewerComposition['domain']['TOYOU'] { return this.domain.TOYOU; }
  get TOTAL_TOKEN_KINDS(): ViewerComposition['domain']['TOTAL_TOKEN_KINDS'] { return this.domain.TOTAL_TOKEN_KINDS; }
  get TOKEN_KINDS(): ViewerComposition['domain']['TOKEN_KINDS'] { return this.domain.TOKEN_KINDS; }
  get TREE_RANK(): ViewerComposition['domain']['TREE_RANK'] { return this.domain.TREE_RANK; }
  get sentenceHost(): ViewerComposition['sentencesOwner']['sentenceHost'] { return this.sentencesOwner.sentenceHost; }
  get pagerController(): ViewerComposition['pagingOwner']['pagerController'] { return this.pagingOwner.pagerController; }
  get routeModel(): ViewerComposition['bootstrapOwner']['routeModel'] { return this.bootstrapOwner.routeModel; }
  get app(): ViewerComposition['layoutOwner']['app'] { return this.layoutOwner.app; }
  get shellChrome(): ViewerComposition['accountControlsOwner']['shellChrome'] { return this.accountControlsOwner.shellChrome; }
  get accountChrome(): ViewerComposition['accountControlsOwner']['accountChrome'] { return this.accountControlsOwner.accountChrome; }
  get ORD(): ViewerComposition['orderingControlsOwner']['ORD'] { return this.orderingControlsOwner.ORD; }
  get ORD_DRAWER_MS(): ViewerComposition['orderingControlsOwner']['ORD_DRAWER_MS'] { return this.orderingControlsOwner.ORD_DRAWER_MS; }
  get recentRenderer(): ViewerComposition['recentNavigation']['recentRenderer'] { return this.recentNavigation.recentRenderer; }
  get COST_TIP(): ViewerComposition['recentNavigation']['COST_TIP'] { return this.recentNavigation.COST_TIP; }
  get errorNavigation(): ViewerComposition['_SessionChrome']['errorNavigation'] { return this._SessionChrome.errorNavigation; }
  get viewerBar(): ViewerComposition['_SessionChrome']['viewerBar'] { return this._SessionChrome.viewerBar; }
  get SLOTS(): ViewerComposition['documentRendererOwner']['SLOTS'] { return this.documentRendererOwner.SLOTS; }
  get liveController(): ViewerComposition['liveModelOwner']['liveController'] { return this.liveModelOwner.liveController; }
  get LIVE(): ViewerComposition['liveModelOwner']['LIVE'] { return this.liveModelOwner.LIVE; }
  get scrollController(): ViewerComposition['viewport']['scrollController'] { return this.viewport.scrollController; }
  get skipPop(): boolean { return this.toolViewsOwner.skipPop; }
  set skipPop(value: boolean) {
    this.toolViewsOwner.skipPop = value;
}
  get viewerEl(): HTMLDialogElement | null { return this.toolViewsOwner.viewerEl; }
  set viewerEl(value: HTMLDialogElement | null) {
    this.toolViewsOwner.viewerEl = value;
}
  get STATE(): Record<string, string> { return STATE; }
  get treePrefs(): ReturnType<typeof createLayout>["treePrefs"] { return this.layoutOwner.treePrefs; }
  set treePrefs(value: ReturnType<typeof createLayout>["treePrefs"]) {
    this.layoutOwner.treePrefs = value;
}
  get HARNESSES(): Record<string, { name: string; short: string; icon: { light: string; dark: string; }; }> { return HARNESSES; }
  get I(): Record<string, string> { return I; }
  get HARNESS(): { [k: string]: string; } { return HARNESS; }
  get railMode(): boolean { return this.layoutOwner.railMode; }
  set railMode(value: boolean) {
    this.layoutOwner.railMode = value;
}
  get ordIdle(): number | undefined { return this.orderingControlsOwner.ordIdle; }
  set ordIdle(value: number | undefined) {
    this.orderingControlsOwner.ordIdle = value;
}
  get wideMode(): boolean { return this.layoutOwner.wideMode; }
  set wideMode(value: boolean) {
    this.layoutOwner.wideMode = value;
}
  get TOK(): Record<string, string> { return this.transportOwner.TOK; }
  set TOK(value: Record<string, string>) {
    this.transportOwner.TOK = value;
}
  get HARNESS_SHORT(): { [k: string]: string; } { return HARNESS_SHORT; }
  get scrollRevision(): number { return this.pagingOwner.scrollRevision; }
  set scrollRevision(value: number) {
    this.pagingOwner.scrollRevision = value;
}
  get programmaticScrollPending(): boolean { return this.pagingOwner.programmaticScrollPending; }
  set programmaticScrollPending(value: boolean) {
    this.pagingOwner.programmaticScrollPending = value;
}
  constructor(host: ViewerHost | null, onDestroyed: (owner: ViewerApplication) => void) {
    const context = this;
    this.scope = new EffectScope();
    this.dialogs = new Map<HTMLDialogElement, {
    destroy(): void;
}>();
    this.disposed = false;
    this.application = { destroy() {
        if (context.disposed)
            return;
        context.disposed = true;
        context.scope.destroy();
        context.liveController.destroy();
        context.navigation.destroy();
        context.transcripts.destroy();
        context.stopOpeningEndPin();
        context.pagerController.disconnect();
        context.errorNavigation.destroy();
        context.scrollController.destroy();
        for (const dialog of context.dialogs.values())
            dialog.destroy();
        context.dialogs.clear();
        for (const slot of context.SLOTS.values())
            slot.ctx.destroy?.();
        context.SLOTS.clear();
        if (!context.SIDEBAR_ONLY)
            releaseScreen(context.$('#page'));
        context.recentRenderer.destroy();
        context.viewerBar.destroy();
        context.shellChrome?.destroy();
        context.accountChrome.destroy();
        releaseGeometry(context.app);
        releaseGeometry(document.documentElement, ['barHeight']);
        document.querySelectorAll('.livenote, .livenote-side').forEach(node => node.remove());
        document.documentElement.classList.remove('viewer-open', 'panel-open');
        onDestroyed(context.application);
    } };
    this.NOW = Date.now();
    this.modelStore = new ViewerModelStore();
    this.MACHINE = context.modelStore.machines;
    this.MACHINE_UP = context.modelStore.machineUp;
    this.MACHINE_LAST = context.modelStore.machineLast;
    this.ADMIN = null;
    this.ACCOUNT = null;
    this.viewerHost = host;
    this.NATIVE_PAGE = context.viewerHost?.nativePage;
    this.NAV_MACHINES = context.viewerHost?.machinesPath ?? null;
    this.SIDEBAR_ONLY = document.querySelector<HTMLElement>(".app")?.dataset.viewer === "sidebar";
    this.SESS = context.modelStore.sessions;
    this.H = context.modelStore.handoffs;
    this.transcripts = new TranscriptStore({ request: (path, signal) => context.api(path, signal), turns: (sid) => context.modelStore.turns[sid] ?? [], turn: (id) => context.modelStore.turn.get(id), entry: (e) => context.txEntry(e), cleared(sid) { if (context.navigation.route.v === "session" && ("id" in context.navigation.route ? context.navigation.route.id : "") === sid)
        context.resetPagerInput(); } });
    this.TX = context.transcripts.entries;
    this.seenResultsOwner = createSeenResults({});
    this.HID = context.modelStore.handoff;
    this.TURNS = context.modelStore.turns;
    this.TURN = context.modelStore.turn;
    this.STARTS = context.modelStore.starts;
    this.HOLDS = context.modelStore.holds;
    this.TXM = context.transcripts.meta;
    this.domain = createDomain({ sessions: context.SESS, machines: context.MACHINE, handoffs: context.H, turns: context.TURNS, turn: context.TURN, starts: context.STARTS, holds: context.HOLDS, handoff: context.HID, transcriptMeta: context.TXM }, () => context.NOW, context.SEEN_RESULTS);
    this.clock = (t: number) => formatClock(t, context.NOW);
    this.ago = (t: number) => formatAgo(t, context.NOW);
    this.dur = (a: number, b: number | null | undefined) => formatDuration(a, b ?? undefined, context.NOW);
    this.$ = <T extends HTMLElement = HTMLElement>(s: string, r: ParentNode = document) => r.querySelector<T>(s)!;
    this.spaced = (t: unknown) => String(t).replace(/ · /g, "\u2009 · \u2009").replace(/^· /, "·\u2009 ");
    this.darkTheme = () => { const t = document.documentElement.getAttribute("data-theme"); return t === "dark" || (t !== "light" && !!window.matchMedia?.("(prefers-color-scheme: dark)").matches); };
    this.facetLine = (s: Session) => [s.kind ?? HARNESS[s.harness], context.MACHINE[s.machine], context.where(s)].join(" · ");
    this.seenPersistenceOwner = createSeenPersistence(context);
    this.sentencesOwner = createSentences(context);
    this.isGap = (e: Entry) => e.k === "end" && /entries (not included|omitted)|^No activity/.test(e.text ?? "");
    this.transportOwner = createTransport(context);
    this.STALE_BRIEFS = context.transcripts.staleBriefs;
    this.TXCACHE = context.transcripts.cache;
    this.cacheTx = (sid: string, entries: Entry[], meta: TranscriptMeta) => context.transcripts.keep(sid, entries, meta, !!context.originHandoff(sid));
    this.adoptCached = (r: Extract<ApplicationRoute, {
    v: "session";
}>) => context.transcripts.adoptCached(r.id, r.turn);
    this.transcriptRevalidationOwner = createTranscriptRevalidation(context);
    this.pagingOwner = createPaging(context);
    this.toolLoaderOwner = createToolLoader(context);
    this.bootstrapOwner = createBootstrap(context);
    this.phone = window.matchMedia("(max-width: 760px)");
    this.navigation = new NavigationController({ model: context.routeModel, loadMachines: host ? (signal) => host.loadMachines(signal) : undefined }, context.viewerHost?.initialMachines ?? null);
    this.layoutOwner = createLayout(context);
    this.groupBy = "recent";
    this.query = "";
    this.focusSessionsSearchOnRender = null;
    this.analyticsRange = 7;
    this.analyticsMeasure = "hours";
    this.showApprovalReviews = false;
    this.sessionFilters = { repo: "", machine: "", harness: "", model: "" };
    this.pendingSessionOpen = null;
    this.accountSheet = false;
    this.afterPop = null;
    if (!context.SIDEBAR_ONLY)
    try {
        history.scrollRestoration = "manual";
    }
    catch { }
    this.SHOW_ALL = { messages: true, tools: true, thinking: true };
    this.show = { ...context.SHOW_ALL };
    this.find = "";
    this.findOpen = false;
    this.historyScrollOwner = createHistoryScroll(context);
    this.destination = createDestination(context);
    this.accountControlsOwner = createAccountControls(context);
    this.orderingControlsOwner = createOrderingControls(context);
    this.navigationViewOwner = createNavigationView(context);
    this.recentNavigation = createRecentNavigation(context);
    this._SessionChrome = createSessionChrome(context);
    this._ScreenViews = createScreenViews(context);
    this._TranscriptView = createTranscriptView(context);
    this.toolViewsOwner = createToolViews(context);
    this.documentRendererOwner = createDocumentRenderer(context);
    this._Analytics = createAnalytics(context);
    this.sessionListOwner = createSessionList(context);
    this.documentEventsOwner = createDocumentEvents(context);
    this.liveModelOwner = createLiveModel(context);
    this._LiveUpdates = createLiveUpdates(context);
    this.applicationRefreshOwner = createApplicationRefresh(context);
    this.viewport = createViewport(context);
    this.tickerOwner = createTicker(context);
    // An embedding page's sidebar: the row its data-viewer-nav names (home, sessions or machines) is current.
if (context.SIDEBAR_ONLY) {
    const nav = context.app.dataset.viewerNav;
    context.navigation.route = context.navigation.historyRoute({ v: nav }, { v: "home" });
}
    if (context.viewerHost) {
    this.ACCOUNT = context.accountOf(context.viewerHost.account);
    if (context.NATIVE_PAGE)
        context.navigation.route = context.navigation.historyRoute({ v: context.NATIVE_PAGE.nav }, { v: 'home' });
    else if (context.navigation.content)
        context.navigation.route = { v: "machines" };
    if (context.NATIVE_PAGE || context.navigation.content)
        context.render();
}
    context.boot();
  }
}

