import { useEffect, useRef, useState } from 'preact/hooks';
import { render } from 'preact';
import type { Meta, StoryObj } from '@storybook/preact-vite';
import markUrl from '../../crates/semon-sessions/src/mark.svg?url';
import { mountDesignGallery } from '../tests/fixtures/design-gallery';
import {
  HomeView,
  MachinesView,
  SessionRowView,
  type HomeHost,
  type HomeSnapshot,
  type MachinesHost,
  type MachinesSnapshot,
  type SessionRow as SessionRowData,
} from '../src/lib/screens';
import { renderAnalyticsScreen, type AnalyticsHost } from '../src/lib/analytics';
import { renderCatalogList, type CatalogListHost } from '../src/lib/catalog';
import { createFacetChrome } from '../src/lib/facets';
import { createSelect } from '../src/lib/select';
import { installTooltip } from '../src/lib/tooltip';
import { renderSessionMenu, type SessionMenuHost } from '../src/lib/session-menu';
import { renderTraceScreen, type TraceHost } from '../src/lib/trace';
import { releaseScreen } from '../src/lib/screens';
import {
  renderSessionScreen,
  type SessionSnapshot,
  type TranscriptHost,
} from '../src/lib/transcript';

const meta = {
  title: 'Semon/Production owners',
  parameters: { layout: 'fullscreen' },
} satisfies Meta;
export default meta;
type Story = StoryObj<typeof meta>;

const homeHost: HomeHost = {
  session() {},
  inbox() {},
  trace() {},
  committed() {},
  answered() {},
};
const homeSnapshot: HomeSnapshot = {
  waiting: 1,
  working: 2,
  up: 2,
  machines: 2,
  inbox: [
    {
      id: 'review-1',
      quiet: false,
      icon: 'M5 12h14M12 5l7 7-7 7',
      parts: [{ className: 'who', text: 'Semon' }],
      age: '8m',
      preview: 'The UI review is ready for your decision.',
      context: 'Session: viewer migration',
      trace: 'trace-review',
    },
  ],
  live: [
    {
      id: 'session-1',
      name: 'Migrate the visual acceptance surfaces',
      state: 'work',
      stateLabel: 'Working',
      status: 'Working',
      detail: 'Inspect Storybook owners',
      harness: {
        id: 'codex',
        name: 'Codex',
        light: markUrl,
        dark: markUrl,
        darkTheme: false,
      },
      activity: ['Working', 'Inspecting sources', 0],
    },
  ],
  answered: [],
  totalAnswered: 0,
  allAnswered: true,
};

export const Home: Story = {
  render: () => (
    <main class="page storybook-component-frame">
      <HomeView snapshot={homeSnapshot} host={homeHost} />
    </main>
  ),
};

const machinesSnapshot: MachinesSnapshot = {
  up: 1,
  working: 2,
  rows: [
    {
      id: 'laptop',
      name: 'Marvin’s laptop',
      state: 'work',
      stateLabel: 'Online',
      status: '2 sessions working',
      detail: 'Updated just now',
    },
    {
      id: 'desktop',
      name: 'Build desktop',
      state: 'off',
      stateLabel: 'Offline',
      status: 'Unavailable',
      detail: 'Last seen yesterday',
    },
  ],
  admin: { label: 'Manage machines', href: '/machines' },
};
const machinesHost: MachinesHost = { machine() {}, admin() {}, committed() {} };

export const Machines: Story = {
  render: () => (
    <main class="page storybook-component-frame">
      <MachinesView snapshot={machinesSnapshot} host={machinesHost} />
    </main>
  ),
};

const sessionRow: SessionRowData = {
  id: 'session-1',
  name: 'Review the new visual contract',
  state: 'work',
  stateLabel: 'Working',
  age: '12m',
  harness: {
    id: 'codex',
    name: 'Codex',
    light: markUrl,
    dark: markUrl,
    darkTheme: false,
  },
  model: 'gpt-6',
  modelTip: 'gpt-6',
  fields: [
    { className: 'row-repo', text: 'onsager-ai/semon', priority: 2 },
    { className: 'row-branch', text: 'feat/storybook', priority: 3 },
  ],
};

export const SessionRow: Story = {
  render: () => (
    <main class="page storybook-component-frame">
      <div class="ph">
        <h1>Sessions</h1>
        <div class="sub">Recent activity</div>
      </div>
      <div class="list">
        <SessionRowView row={sessionRow} host={{ session() {} }} />
        <SessionRowView
          row={{ ...sessionRow, id: 'child-1', name: 'Check visual diffs' }}
          child
          host={{ session() {} }}
        />
      </div>
    </main>
  ),
};

export const KeyboardFocus: Story = {
  render: () => {
    function FocusedRow() {
      const frame = useRef<HTMLElement>(null);
      useEffect(() => {
        frame.current?.querySelector<HTMLButtonElement>('[data-session-row]')?.focus();
      }, []);
      return (
        <main class="page storybook-component-frame" ref={frame}>
          <div class="ph">
            <h1>Keyboard focus</h1>
            <div class="sub">Session navigation</div>
          </div>
          <div class="list">
            <SessionRowView row={sessionRow} host={{ session() {} }} />
          </div>
        </main>
      );
    }
    return <FocusedRow />;
  },
};

function Mounted({ mount }: { mount(root: HTMLElement): () => void }) {
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!root.current) return;
    return mount(root.current);
  }, [mount]);
  return <main class="page storybook-component-frame" ref={root} />;
}

export const ComponentGallery: Story = {
  render: () => (
    <div class="app storybook-component-frame">
      <Mounted mount={mountDesignGallery} />
    </div>
  ),
};

export const GalleryLifecycle: Story = {
  render: () => {
    function LifecycleOwner() {
      const [mounted, setMounted] = useState(true);
      return (
        <main class="page storybook-component-frame">
          <button
            type="button"
            aria-label={mounted ? 'Unmount gallery' : 'Mount gallery'}
            onClick={() => setMounted((value) => !value)}
          >
            {mounted ? 'Unmount gallery' : 'Mount gallery'}
          </button>
          {mounted && <Mounted mount={mountDesignGallery} />}
        </main>
      );
    }
    return <LifecycleOwner />;
  },
};

export const Tooltip: Story = {
  render: () => {
    function TooltipOwner() {
      useEffect(() => {
        installTooltip();
      }, []);
      return (
        <main class="page storybook-component-frame">
          <div class="ph">
            <h1>Tooltip</h1>
            <div class="sub">Keyboard and pointer description</div>
          </div>
          <button type="button" data-tip="A recorded source may be incomplete while history loads.">
            Source status
          </button>
        </main>
      );
    }
    return <TooltipOwner />;
  },
};

export const AnalyticsStates: Story = {
  render: () => (
    <Mounted
      mount={(root) => {
        const host: AnalyticsHost = {
          facets: () => document.createElement('div'),
          committed() {},
          measure() {},
          breakdown() {},
          session() {},
          slice() {},
          model() {},
        };
        renderAnalyticsScreen(
          root,
          {
            heading: 'No activity is available in this time range.',
            ready: false,
            query: '',
            metrics: [],
            charts: [],
            measure: 'cost',
            breakdowns: [],
            lists: [],
            modelHeaders: [],
          },
          host,
        );
        return () => releaseScreen(root);
      }}
    />
  ),
};

export const AnalyticsLoaded: Story = {
  render: () => (
    <Mounted
      mount={(root) => {
        const host: AnalyticsHost = {
          facets: () => document.createElement('div'),
          committed() {},
          measure() {},
          breakdown() {},
          session() {},
          slice() {},
          model() {},
        };
        renderAnalyticsScreen(
          root,
          {
            heading: 'Last 7 days · 18 sessions',
            ready: true,
            query: '',
            metrics: [
              {
                id: 'total-cost',
                label: 'Recorded API cost',
                value: '$0.42',
                explanation: 'Known cost from the selected recorded sessions.',
                note: { text: 'across 18 sessions' },
              },
              {
                id: 'active-time',
                label: 'Active time',
                value: '3h 24m',
                note: { text: 'from recorded activity' },
              },
            ],
            charts: [
              {
                key: 'daily-cost',
                heading: 'Recorded cost over time',
                sub: 'Daily totals · last 7 days',
                width: 640,
                label: 'Recorded daily cost by harness',
                role: 'img',
                left: 42,
                right: 628,
                grid: [
                  { y: 150, label: '$0' },
                  { y: 95, label: '$0.10' },
                  { y: 40, label: '$0.20' },
                ],
                bins: [
                  {
                    key: 'day-1',
                    x: 78,
                    width: 36,
                    claude: 38,
                    codex: 66,
                    barX: 78,
                    barWidth: 36,
                    active: false,
                    label: 'Monday',
                    when: 'Monday',
                    value: '$0.19',
                  },
                  {
                    key: 'day-2',
                    x: 176,
                    width: 36,
                    claude: 20,
                    codex: 44,
                    barX: 176,
                    barWidth: 36,
                    active: true,
                    label: 'Tuesday',
                    when: 'Tuesday',
                    value: '$0.12',
                  },
                ],
                ago: 'Updated just now',
                legend: [
                  { id: 'codex', label: 'Codex' },
                  { id: 'claude', label: 'Claude' },
                ],
              },
            ],
            measure: 'cost',
            breakdowns: [
              {
                heading: 'By harness',
                key: 'harness',
                rows: [
                  {
                    key: 'codex',
                    name: 'Codex',
                    count: '11',
                    width: 72,
                    hours: '2h 12m',
                    cost: '$0.28',
                  },
                  {
                    key: 'claude',
                    name: 'Claude',
                    count: '7',
                    width: 46,
                    hours: '1h 12m',
                    cost: '$0.14',
                  },
                ],
              },
            ],
            lists: [
              {
                heading: 'Most active sessions',
                rows: [
                  {
                    id: 'session-1',
                    name: 'Review the Storybook coverage',
                    harness: 'codex',
                    harnessName: 'Codex',
                    value: '$0.16',
                    rank: 1,
                    href: '/session/session-1',
                  },
                ],
              },
            ],
            models: [],
            modelHeaders: ['Model', 'Sessions', 'Recorded cost'],
          },
          host,
        );
        return () => releaseScreen(root);
      }}
    />
  ),
};

export const SourceCandidates: Story = {
  render: () => (
    <Mounted
      mount={(root) => {
        const host: CatalogListHost = {
          session() {},
          filters() {},
          clearFilters() {},
          more() {},
          retry() {},
          candidate() {},
          candidateMore() {},
          candidateRetry() {},
        };
        renderCatalogList(
          root,
          {
            items: [],
            sourceLabel: 'Local machine',
            updating: false,
            observation: 'Current source metadata',
            observationState: 'incomplete',
            note: 'History is still being indexed.',
            harness: '',
            repo: '',
            more: false,
            candidates: [
              {
                candidate_key: 'candidate-1',
                source: { root: '/workspace/project', path: '/workspace/project/session.jsonl' },
                native_name_hint: 'Imported session',
                generation: 'generation-1',
                archive_observed_at: 1_800_000_000,
              },
            ],
            candidateNote: 'An archive candidate is available for review.',
          },
          host,
        );
        return () => render(null, root);
      }}
    />
  ),
};

function catalogSnapshot(overrides: Partial<Parameters<typeof renderCatalogList>[1]> = {}) {
  return {
    items: [
      {
        key: 'session-1',
        name: 'Review the visual acceptance contract',
        harness: 'codex',
        repo: 'onsager-ai/semon',
        branch: 'feat/storybook',
        model: 'gpt-6',
        freshness: { state: 'current' },
        presentation: sessionRow,
        machine: { key: 'laptop', label: 'Marvin’s laptop' },
      },
    ],
    sourceLabel: 'Workspace',
    updating: false,
    note: '',
    harness: '',
    repo: '',
    more: false,
    ...overrides,
  };
}

function mountCatalog(root: HTMLElement, snapshot: Parameters<typeof renderCatalogList>[1]) {
  const host: CatalogListHost = {
    session() {},
    filters() {},
    clearFilters() {},
    more() {},
    retry() {},
    candidate() {},
    candidateMore() {},
    candidateRetry() {},
  };
  renderCatalogList(root, snapshot, host);
  return () => render(null, root);
}

export const SessionsCatalog: Story = {
  render: () => <Mounted mount={(root) => mountCatalog(root, catalogSnapshot())} />,
};

export const FilteredCatalog: Story = {
  render: () => (
    <Mounted
      mount={(root) =>
        mountCatalog(
          root,
          catalogSnapshot({
            query: 'visual',
            metadataSearch: true,
            searchQualification: 'Searches recorded names, IDs and repositories.',
            searchPartial: true,
            note: 'Only the loaded part of the index has been searched.',
          }),
        )
      }
    />
  ),
};

export const MachineFilter: Story = {
  render: () => (
    <Mounted
      mount={(root) => {
        const facets = createFacetChrome({
          select(label, value, changed) {
            const controller = createSelect({
              label,
              value,
              options: [
                { value: 'all', label: 'All machines' },
                { value: 'laptop', label: 'Marvin’s laptop' },
              ],
              onChange: changed,
            });
            return {
              el: controller.el,
              setOptions: controller.setOptions,
              setValue: controller.setValue,
              focus: controller.focus,
              close: () => controller.close(),
              destroy: controller.destroy,
            };
          },
          change() {},
          cleared() {},
          canOpen: () => true,
          opened() {},
          closed() {},
          clear() {},
        });
        facets.update(
          [
            {
              key: 'machine',
              label: 'Machine',
              value: 'all',
              display: 'All machines',
              options: [
                { value: 'all', label: 'All machines' },
                { value: 'laptop', label: 'Marvin’s laptop' },
              ],
            },
          ],
          { description: 'Filter workspace sessions by machine.' },
        );
        root.append(facets.element);
        return () => {
          facets.destroy();
          render(null, root);
        };
      }}
    />
  ),
};

export const CatalogFailure: Story = {
  render: () => (
    <Mounted
      mount={(root) =>
        mountCatalog(
          root,
          catalogSnapshot({
            items: [],
            observationState: 'unavailable',
            observation: 'The source is unavailable.',
            note: 'Session history could not be loaded.',
          }),
        )
      }
    />
  ),
};

export const CatalogEmpty: Story = {
  render: () => <Mounted mount={(root) => mountCatalog(root, catalogSnapshot({ items: [] }))} />,
};

export const CatalogLoading: Story = {
  render: () => (
    <Mounted
      mount={(root) =>
        mountCatalog(
          root,
          catalogSnapshot({ items: [], updating: true, observationState: 'updating' }),
        )
      }
    />
  ),
};

export const SessionMenu: Story = {
  render: () => (
    <Mounted
      mount={(root) => {
        const host: SessionMenuHost = { action() {}, session() {}, wide: () => false };
        renderSessionMenu(
          root,
          {
            actions: [
              { key: 'copy', text: 'Copy session ID' },
              { key: 'trace', text: 'Open trace' },
            ],
            path: [
              {
                id: 'session-1',
                name: 'Visual contract review',
                harness: 'codex',
                harnessName: 'Codex',
              },
            ],
            status: 'Working',
            state: 'work',
            stateLabel: 'Working',
            details: [{ label: 'Model', value: 'gpt-6', mono: true }],
            cost: {
              figure: '$0.04',
              caption: 'Recorded cost',
              note: 'Available usage',
              details: [],
              runs: [],
              models: [],
              includesRuns: false,
            },
            notice: '',
            icons: { chevron: 'M6 9l6 6 6-6', copy: 'M8 8h12v12H8z' },
            command: 'codex resume session-1',
          },
          host,
        );
        return () => render(null, root);
      }}
    />
  ),
};

export const TraceEmpty: Story = {
  render: () => (
    <Mounted
      mount={(root) => {
        const host: TraceHost = {
          session() {},
          machine() {},
          sender() {},
          fold() {},
          committed() {},
        };
        renderTraceScreen(
          root,
          { empty: true, hops: [], observation: 'No trace is available.' },
          host,
        );
        return () => releaseScreen(root);
      }}
    />
  ),
};

export const TraceFlow: Story = {
  render: () => (
    <Mounted
      mount={(root) => {
        const host: TraceHost = {
          session() {},
          machine() {},
          sender() {},
          fold() {},
          committed() {},
        };
        renderTraceScreen(
          root,
          {
            empty: false,
            summary: 'One request led to two recorded child sessions.',
            hops: [
              {
                key: 'hop-1',
                className: 'trace-root',
                nodeClass: 'trace-request',
                icon: 'M5 5h14v14H5z',
                turn: 'turn-1',
                parts: [
                  { className: 'who', text: 'You' },
                  { className: 'verb', text: 'asked Codex to review the viewer.' },
                ],
                time: '10:14',
                brief: 'Review the full visual contract and report missing states.',
                result: 'Identified active component and route coverage.',
                meta: {
                  state: 'done',
                  stateLabel: 'Complete',
                  text: 'Review complete',
                  chip: 'gpt-6',
                  harness: {
                    id: 'codex',
                    name: 'Codex',
                    light: markUrl,
                    dark: markUrl,
                    darkTheme: false,
                  },
                  session: 'session-1',
                  turn: 'turn-1',
                  name: 'Review the viewer contract',
                },
              },
              {
                key: 'hop-2',
                className: 'trace-child',
                nodeClass: 'trace-result',
                icon: 'M6 12h12M12 6l6 6-6 6',
                handoff: 'handoff-1',
                turn: 'turn-child',
                parts: [
                  { className: 'who', text: 'Codex' },
                  { className: 'verb', text: 'started a phone layout check.' },
                ],
                time: '10:16',
                brief: 'Verify compact related-session rows at 334px.',
                result: 'Touch controls remain available.',
                meta: {
                  state: 'done',
                  stateLabel: 'Complete',
                  text: 'Returned to parent',
                  chip: '2m',
                  session: 'child-1',
                  turn: 'turn-child',
                  name: 'Check phone layout',
                },
              },
            ],
            agents: {
              summary: ['2 sessions', '3m observed time'],
              start: '10:14',
              end: '10:17',
              ticks: [
                { left: 0, label: '10:14', nearEnd: false },
                { left: 50, label: '10:15', nearEnd: false },
                { left: 100, label: '10:17', nearEnd: true },
              ],
              legend: 'Observed time and recorded cost',
              incomplete: false,
              rows: [
                {
                  kind: 'agent',
                  id: 'session-1',
                  depth: 0,
                  label: 'Parent session',
                  name: 'Review the viewer contract',
                  harnessClass: 'h-codex',
                  model: 'gpt-6',
                  state: 'done',
                  stateLabel: 'Complete',
                  duration: '3m',
                  cost: '$0.04',
                  segments: [{ left: 2, width: 98, kind: 'parent' }],
                  spawns: [40],
                },
                {
                  kind: 'agent',
                  id: 'child-1',
                  depth: 1,
                  label: 'Child session',
                  name: 'Check phone layout',
                  harnessClass: 'h-codex',
                  model: 'gpt-6',
                  state: 'done',
                  stateLabel: 'Complete',
                  duration: '2m',
                  cost: '$0.02',
                  edge: { parent: 'session-1', started: 40, done: 95, returned: true },
                  segments: [{ left: 40, width: 55, kind: 'child' }],
                },
              ],
            },
          },
          host,
        );
        return () => releaseScreen(root);
      }}
    />
  ),
};

export const TranscriptEmpty: Story = {
  render: () => (
    <Mounted
      mount={(root) => {
        const snapshot: SessionSnapshot = {
          id: 'session-1',
          name: 'Visual contract review',
          empty: 'No messages have been recorded for this session.',
          blocks: [],
          order: [],
        };
        const host = {
          session() {},
          machine() {},
          sender() {},
          committed() {},
          trace() {},
          toolAll() {},
          script() {},
          image() {},
          background() {},
          pager() {},
          jump() {},
        } satisfies TranscriptHost;
        renderSessionScreen(root, snapshot, host);
        return () => releaseScreen(root);
      }}
    />
  ),
};

function mountTranscript(root: HTMLElement, snapshot: SessionSnapshot) {
  const host: TranscriptHost = {
    session() {},
    machine() {},
    sender() {},
    committed() {},
    trace() {},
    toolAll() {},
    script() {},
    image() {},
    background() {},
    pager() {},
    jump() {},
  };
  renderSessionScreen(root, snapshot, host);
  return () => releaseScreen(root);
}

const compactReaderSnapshot: SessionSnapshot = {
  id: 'session-1',
  name: 'Review the visual acceptance contract',
  blocks: [
    {
      kind: 'turn',
      turn: {
        id: 'turn-1',
        traceIcon: 'M6 4v10a4 4 0 0 0 4 4h8',
        stateLabel: 'Complete',
        entries: [
          {
            kind: 'message',
            flavor: 'user',
            text: 'Please review the compact related-session disclosure.',
          },
          {
            kind: 'child',
            handoff: 'handoff-1',
            id: 'child-1',
            turn: 'turn-child',
            name: 'Check the phone layout',
            state: 'done',
            stateLabel: 'Complete',
            mark: { id: 'codex', name: 'Codex', light: markUrl, dark: markUrl, darkTheme: false },
            meta: 'Codex · 3m',
            brief: 'Check the narrow reader and touch targets.',
            result: 'The name, status and disclosure remain visible.',
            failed: false,
            activity: ['Checked the 334px reader', 'Review completed'],
            trace: 'trace-child',
            traceLabel: 'Trace child work',
            chevron: 'M9 18l6-6-6-6',
          },
          {
            kind: 'message',
            flavor: 'assistant',
            text: 'The related work stays compact until expanded.',
          },
        ],
      },
    },
  ],
  order: ['turn-1'],
};

export const CompactReader: Story = {
  render: () => <Mounted mount={(root) => mountTranscript(root, compactReaderSnapshot)} />,
};

const longRecordedText = Array.from(
  { length: 24 },
  (_, index) =>
    `Review note ${index + 1}: the recorded session keeps its complete response, tool output, and related work in the same reading order. The viewer retains this text while the transcript is paged and while the reading viewport moves between earlier and later content.`,
).join('\n\n');

export const LongTranscript: Story = {
  render: () => (
    <Mounted
      mount={(root) =>
        mountTranscript(root, {
          id: 'session-long',
          name: 'Review the complete visual contract',
          blocks: [
            {
              kind: 'turn',
              turn: {
                id: 'turn-long',
                traceIcon: 'M6 4v10a4 4 0 0 0 4 4h8',
                stateLabel: 'Complete',
                entries: [
                  {
                    kind: 'message',
                    key: 'long-request',
                    flavor: 'user',
                    text: 'Read the full report and preserve its content.',
                  },
                  {
                    kind: 'thought',
                    key: 'long-thought',
                    mode: 'readable',
                    label: 'Planning',
                    text: 'Inspect the source, compare the visible states, and keep the complete finding available.',
                  },
                  {
                    kind: 'message',
                    key: 'long-answer',
                    flavor: 'assistant',
                    text: longRecordedText,
                  },
                ],
              },
            },
          ],
          order: ['turn-long'],
        })
      }
    />
  ),
};

export const FailureInspection: Story = {
  render: () => (
    <Mounted
      mount={(root) => {
        const snapshot: SessionSnapshot = {
          id: 'session-failed',
          name: 'Inspect a failed command',
          blocks: [
            {
              kind: 'turn',
              turn: {
                id: 'turn-failed',
                traceIcon: 'M6 4v10a4 4 0 0 0 4 4h8',
                stateLabel: 'Failed',
                error: 'The recorded command exited with status 1.',
                entries: [
                  {
                    kind: 'message',
                    key: 'request',
                    flavor: 'user',
                    text: 'Run the visual check and report failures.',
                  },
                  {
                    kind: 'tool',
                    key: 'failed-tool',
                    entryKey: 'failed-tool-entry',
                    step: {
                      className: 'tool-failed',
                      key: 'failed-tool',
                      entryKey: 'failed-tool-entry',
                      tid: 'tool-1',
                      sid: 'session-failed',
                      running: false,
                      background: false,
                      waiting: false,
                      verb: 'Run',
                      label: 'Bash',
                      named: true,
                      status: 'Failed',
                      icon: 'M4 6h16M4 12h16M4 18h16',
                      chevron: 'M9 18l6-6-6-6',
                      data: {
                        name: 'Bash',
                        arg: 'npm run ui:check',
                        cwd: '/workspace/semon',
                        out: 'Error: screenshot “trace-harbor-h1” differs from the approved reference.\nExpected the recorded child result and trace action to remain visible.',
                        ok: false,
                      },
                    },
                  },
                ],
              },
            },
          ],
          order: ['turn-failed'],
        };
        return mountTranscript(root, snapshot);
      }}
    />
  ),
};

export const LoadingEmptyError: Story = {
  render: () => (
    <Mounted
      mount={(root) =>
        mountCatalog(
          root,
          catalogSnapshot({
            items: [],
            updating: true,
            observationState: 'incomplete',
            note: 'Loading retained history. Retry if this message stays visible.',
          }),
        )
      }
    />
  ),
};
