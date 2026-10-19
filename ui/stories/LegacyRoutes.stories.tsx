import { render } from 'preact';
import { useEffect, useRef } from 'preact/hooks';
import type { Meta, StoryObj } from '@storybook/preact-vite';
import { createShellChrome } from '../src/lib/shell';
import { createRecentRenderer } from '../src/lib/recent';
import { createViewerBar } from '../src/lib/viewer-bar';
import {
  renderHomeScreen,
  renderMachinesScreen,
  renderMachineScreen,
  releaseScreen,
} from '../src/lib/screens';
import { renderSessionScreen, type TranscriptHost } from '../src/lib/transcript';
import { renderCausalTrace } from '../src/app/tracePresentation';
import { renderCatalogList, catalogFilterDescription } from '../src/lib/catalog';
import { createCatalogBrowser } from '../src/app/catalogBrowser';
import { createPanelChrome, type PanelChrome } from '../src/lib/panel';
import { renderSessionMenu } from '../src/lib/session-menu';
import { selectedSessionMenu, selectedSessionLabels } from '../src/app/sessionPresentation';
import { renderAnalyticsPresentation } from '../src/app/analyticsPresentation';
import { I, STATE } from '../src/app/registry';
import navigation from '../src/lib/navigation.json';
import {
  causalTrace,
  catalogItems,
  fixtureNow,
  home,
  machine,
  machines,
  mark,
  metadata,
  recentItems,
  sessions,
  transcript,
  type FixtureScope,
} from './LegacyRoutes.data';
import { analytics } from './LegacyRoutes.analytics';

interface RouteArgs {
  frame: string;
  scope: FixtureScope;
}
const idle = () => {};

// Only presentation owners are mounted. No application router, API port,
// history, stream, production hook or replacement screen markup is installed.
function RouteOwner({ frame, scope }: RouteArgs) {
  const app = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!app.current) return;
    const chrome = createShellChrome({
      account: {
        place: idle,
        opened: idle,
        closed: idle,
        navigate: () => true,
        submit: () => true,
      },
      navigate: () => true,
      drawerOpened: idle,
      drawerClosed: idle,
      railChanged: idle,
    });
    chrome.mount(app.current);
    const page = chrome.slots.content;
    const sessionId = frame.startsWith('session-') ? frame.slice(8) : undefined;
    const traceRoute = traces[frame];
    const selected = sessionId ?? traceRoute?.[0];
    const top =
      frame === 'drawer'
        ? 'home'
        : frame.startsWith('machine-')
          ? 'machines'
          : selected
            ? 'sessions'
            : frame;
    chrome.update(
      navigation.map((item) => ({ ...item, href: item.path, current: item.key === top })),
      false,
    );
    const recent = createRecentRenderer(chrome.slots.recent, {
      open: idle,
      all: idle,
      fewer: idle,
      toggle(id, open) {
        const update = (items: ReturnType<typeof recentItems>): ReturnType<typeof recentItems> =>
          items.map((item) => ({
            ...item,
            open: item.id === id ? open : item.open,
            children: item.children ? update([...item.children]) : undefined,
          }));
        recentRows = update(recentRows);
        recent.update({ items: recentRows, empty: false });
      },
    });
    let recentRows = recentItems(selected);
    recent.update({ items: recentRows, empty: false });
    const bar = createViewerBar();
    let panel: PanelChrome | undefined;
    const facets = createCatalogBrowser({
      canOpen: () => true,
      opened: idle,
      closed: idle,
      changed: idle,
      clearFilters: idle,
    });
    const filterDescription = catalogFilterDescription('Workspace', 'Cached history', true, true);
    facets.update(
      { harness: '', repo: '', model: '', machine: '', q: '' },
      Object.keys(sessions).map(metadata),
      'laptop',
      filterDescription,
      machines(scope).rows.map((row) => ({ source_key: row.id, label: row.name })),
      true,
    );
    const title = selected
      ? metadata(selected).name
      : frame.startsWith('machine-')
        ? frame.slice(8)
        : (navigation.find((item) => item.key === top)?.label ?? frame);
    const phone = matchMedia('(max-width: 760px)').matches;
    const machineDetail =
      frame === 'machine-laptop' || frame === 'machine-desktop'
        ? machine(frame.slice(8), scope)
        : undefined;
    const barSlots = bar.update(
      {
        mode: 'normal',
        name: traceRoute ? 'Trace' : title,
        session: selected,
        state: sessionId ? sessions[sessionId].state : undefined,
        stateLabel: sessionId ? STATE[sessions[sessionId].state] : undefined,
        showState: !!sessionId && !(phone && metadata(sessionId).parent),
        crumb: traceRoute ? title : machineDetail ? 'Machines' : undefined,
        ancestors:
          sessionId && !phone && metadata(sessionId).parent
            ? [
                {
                  id: metadata(sessionId).parent!,
                  name: sessions[metadata(sessionId).parent!].name,
                },
              ]
            : [],
        labels: sessionId
          ? selectedSessionLabels(metadata(sessionId), 'laptop')
          : machineDetail
            ? [
                {
                  key: 'state',
                  text: 'Up',
                  className: 'state done',
                  drop: 0,
                  state: 'work',
                  stateLabel: 'Up',
                },
                {
                  key: 'sessions',
                  text:
                    machineDetail.sessions.filter((row) => row.state === 'work').length +
                    ' working · ' +
                    machineDetail.totalSessions +
                    ' sessions',
                  drop: 1,
                },
              ]
            : [],
        trace: !!traceRoute,
        analytics: frame === 'analytics',
        days: 7,
        query: '',
        count: '',
        filter: '',
        failed: 0,
        signals: 0,
        errorMode: '',
        errorText: '',
        errorDisabled: false,
        icons: { search: I.search, more: I.more, back: I.back, x: I.x, up: I.up, dn: I.dn },
      },
      {
        ancestor: idle,
        crumb: idle,
        find: idle,
        closeFind: idle,
        query: idle,
        filter: idle,
        errors: idle,
        closeErrors: idle,
        step: idle,
        range: idle,
        menu(trigger) {
          if (!selected) return;
          panel?.destroy();
          const snapshot = selectedSessionMenu(metadata(selected), 'laptop', false, true);
          panel = createPanelChrome(
            {
              title,
              sub: snapshot.status,
              label: 'Session menu for ' + title,
              className: 'anchored session-menu',
            },
            {
              opened() {
                trigger.setAttribute('aria-expanded', 'true');
              },
              closed() {
                if (panel) render(null, panel.body);
                trigger.setAttribute('aria-expanded', 'false');
                if (trigger.isConnected) trigger.focus({ preventScroll: true });
              },
            },
          );
          renderSessionMenu(panel.body, snapshot, {
            action: idle,
            session: idle,
            wide: () => false,
          });
          panel.show();
        },
      },
    );
    const account = {
      account: {
        name: 'Ada Example',
        login: 'ada@example.test',
        initials: 'AE',
        avatar_href: null,
        workspaces: [
          { name: 'Workspace', role: 'Owner', current: true, switch_href: '/workspace' },
        ],
        links: [],
      },
      compact: false,
      wide: false,
      onWideChange: idle,
    };
    chrome.topbar({
      titleSlot: barSlots.titleSlot,
      actions: [barSlots.actions],
      session: !!sessionId,
      lead: traceRoute
        ? { label: 'Back to ' + title, icon: I.chev, back: idle }
        : { label: 'Open menu', icon: I.menu },
      account,
    });
    chrome.drawerAccount({ ...account, compact: true });
    const activityHost = { session: idle, inbox: idle, trace: idle, committed: idle };
    const transcriptHost: TranscriptHost = {
      session: idle,
      machine: idle,
      sender: idle,
      committed: idle,
      trace: idle,
      toolAll: idle,
      script: idle,
      image: idle,
      background: idle,
      pager: idle,
      jump: idle,
    };
    if (frame === 'home' || frame === 'drawer')
      renderHomeScreen(page, home(), { ...activityHost, answered: idle });
    else if (frame === 'machines')
      renderMachinesScreen(page, machines(scope), { machine: idle, admin: idle, committed: idle });
    else if (frame === 'machine-laptop' || frame === 'machine-desktop')
      renderMachineScreen(page, machine(frame.slice(8), scope), activityHost);
    else if (sessionId) renderSessionScreen(page, transcript(sessionId), transcriptHost);
    else if (traceRoute) {
      const expanded = new Map<string, Set<string>>();
      const paint = () =>
        renderCausalTrace(page, causalTrace(traceRoute[0], traceRoute[1]), fixtureNow, expanded, {
          session: idle,
          fold: paint,
        });
      paint();
    } else if (frame === 'sessions') {
      const items = catalogItems();
      renderCatalogList(
        page,
        {
          items,
          sourceLabel: 'Workspace',
          updating: false,
          note: '',
          harness: '',
          repo: '',
          more: false,
          metadataSearch: true,
          globalUnion: true,
          filterDescription: filterDescription.description,
          searchQualification: filterDescription.searchQualification,
          completeCount: true,
          browser: {
            facets: facets.element,
            groupBy: 'recent',
            working: Object.values(sessions).filter((row) => row.state === 'work').length,
            waiting: Object.values(sessions).filter((row) => row.state === 'wait').length,
            groups: [{ title: '', items }],
          },
        },
        { session: idle, filters: idle, clearFilters: idle, more: idle, retry: idle },
      );
    } else if (frame === 'analytics')
      renderAnalyticsPresentation(
        page,
        analytics(),
        null,
        {
          range: 7,
          measure: 'hours',
          query: '',
          machines: { laptop: 'laptop' },
          costTip: 'API-equivalent cost',
          clock: (value) => new Date(value).toISOString().slice(11, 16),
          money: (value) => '$' + value.toFixed(2),
          mark,
          width: () => Math.max(250, page.clientWidth),
          sessionHref: () => undefined,
        },
        {
          committed: idle,
          facets: () => facets.element,
          session: idle,
          measure: idle,
          breakdown: idle,
          slice: idle,
          model: idle,
        },
      );
    else throw new Error('Unknown explicit route story ' + frame);
    app.current.dataset.fixture = scope;
    app.current.dataset.frame = frame;
    return () => {
      panel?.destroy();
      facets.destroy();
      releaseScreen(page);
      render(null, page);
      recent.destroy();
      bar.destroy();
      chrome.destroy();
    };
  }, [frame, scope]);
  return <div class="app" ref={app} data-storybook-route="legacy" />;
}

const traces: Record<string, readonly [string, string]> = {
  'trace-a-review-a-h-review': ['a-review', 'a-h-review'],
  'trace-atlas-h12': ['atlas', 'h12'],
  'trace-atlas-ingest-a-ask': ['atlas-ingest', 'a-ask'],
  'trace-h-review-h3': ['h-review', 'h3'],
  'trace-harbor-h1': ['harbor', 'h1'],
  'trace-ledger-h16': ['ledger', 'h16'],
  'trace-principal-h8': ['principal', 'h8'],
  'trace-quill-h4': ['quill', 'h4'],
  'trace-sentinel-sentinel_0': ['sentinel', 'sentinel_0'],
};
const meta = {
  title: 'Semon/Legacy routes',
  component: RouteOwner,
  parameters: { layout: 'fullscreen', routeComposition: true },
  args: { scope: 'main' as FixtureScope },
  argTypes: { scope: { control: 'select', options: ['main', 'two-machines', 'pushed'] } },
} satisfies Meta<typeof RouteOwner>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Home: Story = { args: { frame: 'home' } };
export const Drawer: Story = { args: { frame: 'drawer' } };
export const Sessions: Story = { args: { frame: 'sessions' } };
export const Machines: Story = { args: { frame: 'machines' } };
export const MachineLaptop: Story = { args: { frame: 'machine-laptop' } };
export const MachineDesktop: Story = { args: { frame: 'machine-desktop', scope: 'two-machines' } };
export const Analytics: Story = { args: { frame: 'analytics' } };
export const SessionADocs: Story = { args: { frame: 'session-a-docs' } };
export const SessionAIndex: Story = { args: { frame: 'session-a-index' } };
export const SessionAParse: Story = { args: { frame: 'session-a-parse' } };
export const SessionAReview: Story = { args: { frame: 'session-a-review' } };
export const SessionAReviewGrep: Story = { args: { frame: 'session-a-review-grep' } };
export const SessionAScan: Story = { args: { frame: 'session-a-scan' } };
export const SessionAStore: Story = { args: { frame: 'session-a-store' } };
export const SessionAdvisor: Story = { args: { frame: 'session-advisor' } };
export const SessionAtlas: Story = { args: { frame: 'session-atlas' } };
export const SessionAtlasIngest: Story = { args: { frame: 'session-atlas-ingest' } };
export const SessionDeps: Story = { args: { frame: 'session-deps' } };
export const SessionHCodex: Story = { args: { frame: 'session-h-codex' } };
export const SessionHFailed: Story = { args: { frame: 'session-h-failed' } };
export const SessionHReview: Story = { args: { frame: 'session-h-review' } };
export const SessionHReviewCodex: Story = { args: { frame: 'session-h-review-codex' } };
export const SessionHarbor: Story = { args: { frame: 'session-harbor' } };
export const SessionLSub: Story = { args: { frame: 'session-l-sub' } };
export const SessionLedger: Story = { args: { frame: 'session-ledger' } };
export const SessionPrincipal: Story = { args: { frame: 'session-principal' } };
export const SessionQCodex: Story = { args: { frame: 'session-q-codex' } };
export const SessionQPlan: Story = { args: { frame: 'session-q-plan' } };
export const SessionQuill: Story = { args: { frame: 'session-quill' } };
export const SessionSentinel: Story = { args: { frame: 'session-sentinel' } };
export const Trace_a_review_a_h_review: Story = { args: { frame: 'trace-a-review-a-h-review' } };
export const TraceAtlasH12: Story = { args: { frame: 'trace-atlas-h12' } };
export const Trace_atlas_ingest_a_ask: Story = { args: { frame: 'trace-atlas-ingest-a-ask' } };
export const TraceHReviewH3: Story = { args: { frame: 'trace-h-review-h3' } };
export const TraceHarborH1: Story = { args: { frame: 'trace-harbor-h1' } };
export const TraceLedgerH16: Story = { args: { frame: 'trace-ledger-h16' } };
export const TracePrincipalH8: Story = { args: { frame: 'trace-principal-h8' } };
export const TraceQuillH4: Story = { args: { frame: 'trace-quill-h4' } };
export const TraceSentinelSentinel0: Story = { args: { frame: 'trace-sentinel-sentinel_0' } };
