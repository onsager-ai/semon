import { countText, hLabel, niceStep, shortModel, timeText } from '../domain/format';
import { HARNESS, HARNESS_SHORT } from './registry';
import {
  renderAnalyticsScreen,
  type AnalyticsSnapshot,
  type Breakdown,
  type Chart,
  type Metric,
  type ModelBand,
  type HarnessMark,
} from '../lib';
import type {
  AnalyticsData,
  AnalyticsModelGroup,
  AnalyticsSliceItem,
} from '../state/analytics-wire';
const HOUR = 3600000;
const hoursText = (ms: number) => (ms / HOUR).toFixed(1) + ' h';
const nameOfSid = (data: AnalyticsData, sid: string) => data.sessions[sid]?.name ?? sid;
const harnessOfSid = (data: AnalyticsData, sid: string) => data.sessions[sid]?.harness ?? '';
export interface AnalyticsPresentation {
  range: number;
  measure: string;
  query: string;
  machines: Readonly<Record<string, string>>;
  costTip: string;
  clock(time: number): string;
  money(value: number): string;
  mark(harness: string): HarnessMark | undefined;
  width(): number;
  sessionHref(id: string): string | undefined;
}
export interface AnalyticsPresentationActions {
  committed(): void;
  facets(): HTMLElement;
  session(id: string): void;
  measure(value: string): void;
  breakdown(group: string, key: string): void;
  slice(
    data: AnalyticsData,
    bin: { a: number; b: number; sessions: AnalyticsSliceItem[]; more: number },
    costMode: boolean,
  ): void;
  model(group: AnalyticsModelGroup): void;
}
/** Shared projection; transport, filtering, route and dialog lifetimes stay with the reader. */
export function renderAnalyticsPresentation(
  page: HTMLElement,
  A: AnalyticsData | null,
  failure: string | null,
  host: AnalyticsPresentation,
  actions: AnalyticsPresentationActions,
) {
  const rangeName = host.range === 1 ? '24 h' : host.range + ' d';
  const metrics: Metric[] = [],
    charts: Chart[] = [],
    breakdowns: Breakdown[] = [],
    lists: NonNullable<AnalyticsSnapshot['lists']>[number][] = [],
    modelGroups = new Map<string, AnalyticsModelGroup>(),
    slices = new Map<
      string,
      {
        bin: { a: number; b: number; sessions: AnalyticsSliceItem[]; more: number };
        costMode: boolean;
      }
    >();
  const heading =
    'Measured activity · Last ' + (host.range === 1 ? '24 hours' : host.range + ' days');
  let error = failure
    ? A
      ? "Couldn't update Analytics: " + failure + '. Showing the last answer.'
      : "Couldn't load Analytics: " + failure
    : A
      ? undefined
      : 'Loading…';
  const note = (value: number, previous: number, format: (n: number) => string): Metric['note'] => {
    const delta = value - previous;
    return Math.abs(delta) < 1e-9
      ? { text: 'No change vs previous ' + rangeName }
      : {
          lead: (delta > 0 ? '+' : '−') + format(Math.abs(delta)),
          tone: delta > 0 ? 'up' : 'down',
          text: ' vs previous ' + rangeName,
        };
  };
  const metric = (
    label: string,
    value: string,
    delta: Metric['note'],
    explanation?: string,
    info?: string,
  ) =>
    metrics.push({
      id: 'metric-more-analytics-' + metrics.length,
      label,
      value,
      note: delta,
      explanation,
      info,
    });
  const widthOf = (measure: number, max: number) =>
    Math.max(measure ? 2 : 0, (measure / max) * 100);
  const makeChart = (costMode: boolean): Chart => {
    if (!A) throw new Error('Analytics model unavailable');
    const key = costMode ? 'cost' : 'agents',
      W = host.width(),
      left = costMode ? 46 : 40,
      right = W - 4;
    if (costMode && !A.cost)
      return {
        key,
        heading: 'Cost over time',
        info: host.costTip,
        empty:
          'Cost is recorded per UTC day, so there is no hourly series. Pick 7 d or 30 d for a daily chart.',
        width: W,
        left,
        right,
        grid: [],
        bins: [],
        legend: [],
        role: 'img',
        label: '',
        ago: A.days === 1 ? '24 h ago' : A.days + ' d ago',
      };
    const raw = costMode
      ? A.cost!.days.map((d) => ({
          a: d.from,
          b: d.to,
          claude: d.claude_usd,
          codex: d.codex_usd,
          sessions: d.sessions,
          more: d.more,
        }))
      : A.agents.columns.map((c) => ({
          a: c.from,
          b: c.to,
          claude: c.claude_ms / HOUR,
          codex: c.codex_ms / HOUR,
          sessions: c.sessions,
          more: c.more,
        }));
    const most = Math.max(0, ...raw.map((b) => b.claude + b.codex)),
      stepY = niceStep(most || 1),
      max = costMode ? Math.max(0.01, most) : Math.max(stepY, Math.ceil(most / stepY) * stepY);
    const grid = [];
    if (costMode)
      for (let i = 0; i <= 2; i++)
        grid.push({ y: 151 - (139 * i) / 2, label: '$' + ((max * i) / 2).toFixed(2) });
    else
      for (let n = 0; n <= max + 1e-9; n += stepY)
        grid.push({ y: 151 - (139 * n) / max, label: hLabel(n) });
    const step = (right - left) / Math.max(1, raw.length),
      barWidth = Math.max(2, step * 0.64);
    const bins = raw.map((bin, i) => {
      const id = key + ':' + i,
        total = bin.claude + bin.codex,
        when = host.clock(bin.a) + '–' + host.clock(bin.b),
        tip = when + ': ' + hLabel(total);
      slices.set(id, { bin, costMode });
      return {
        key: id,
        x: left + i * step,
        width: step,
        barX: left + i * step + (step - barWidth) / 2,
        barWidth,
        claude: (139 * bin.claude) / max,
        codex: (139 * bin.codex) / max,
        active: costMode ? !!bin.sessions.length : total > 0,
        tip: costMode ? undefined : tip,
        label: costMode
          ? host.clock(bin.a) + ' to ' + host.clock(bin.b) + ': ' + host.money(total)
          : tip + '. Open the sessions busy then',
        when,
        value: costMode ? host.money(total) : hLabel(total) + ' agent-hours',
      };
    });
    return {
      key,
      heading: costMode ? 'Cost over time' : 'Agents at work',
      info: costMode ? host.costTip : undefined,
      sub: costMode
        ? 'API-equivalent cost per UTC day · today so far · stacked by harness'
        : 'Agent-hours ' + A.agents.unit + ' · stacked by harness',
      width: W,
      left,
      right,
      grid,
      bins,
      label: costMode
        ? 'API-equivalent cost per day, stacked by harness'
        : 'Agent-hours ' + A.agents.unit + ' over the selected range, stacked by harness',
      role: costMode ? 'img' : 'group',
      ago: A.days === 1 ? '24 h ago' : A.days + ' d ago',
      legend: [
        ['claude', 'Claude'],
        ['codex', 'Codex'],
      ].map(([id, label]) => ({ id, label, mark: host.mark(id) })),
      missing:
        costMode && A.cost!.incomplete_usage
          ? 'Copied usage is omitted from bars. Its original owner and the full cost are unknown.'
          : costMode && A.cost!.unpriced_models.length
            ? 'no price for ' +
              A.cost!.unpriced_models.join(', ') +
              '; unpriced usage is omitted from bars.'
            : undefined,
    };
  };
  let models: ModelBand[] | undefined, allowance: AnalyticsSnapshot['allowance'];
  const measures: [
    string,
    (
      | 'first_pass_acceptance'
      | 'median_review_rounds'
      | 'median_red_ci_heads'
      | 'median_model_ms'
      | 'median_cost_usd'
      | 'allowance_per_million_input'
    ),
    'acceptance_n' | 'review_rounds_n' | 'red_ci_n' | 'model_time_n' | 'cost_n' | 'allowance_n',
    string,
    (n: number) => string,
  ][] = [
    [
      'Acceptance',
      'first_pass_acceptance',
      'acceptance_n',
      'acceptance',
      (n) => (n * 100).toFixed(0) + '%',
    ],
    ['Review rounds', 'median_review_rounds', 'review_rounds_n', 'review_rounds', String],
    ['Red CI heads', 'median_red_ci_heads', 'red_ci_n', 'ci', String],
    ['Model time', 'median_model_ms', 'model_time_n', 'model_time', timeText],
    [
      'API cost',
      'median_cost_usd',
      'cost_n',
      'cost',
      (...args: Parameters<typeof host.money>) => host.money(...args),
    ],
    [
      'Allowance / M input',
      'allowance_per_million_input',
      'allowance_n',
      'allowance',
      (n) => n.toFixed(2) + '%',
    ],
  ];
  if (A) {
    const now = A.current,
      previous = A.previous,
      pct = (errors: number, tools: number) =>
        tools ? Math.round((errors / tools) * 100) + '%' : '0%';
    metric(
      'Agent-hours',
      hoursText(now.agent_ms),
      note(now.agent_ms, previous.agent_ms, hoursText),
      'Busy time summed across sessions; two sessions busy for an hour count two hours.',
    );
    metric(
      A.days === 1 ? 'Cost today (UTC)' : 'Cost, last ' + A.days + ' UTC days',
      now.cost.usd == null ? '—' : host.money(now.cost.usd),
      now.cost.usd == null || previous.cost.usd == null
        ? {
            text:
              'no price for ' +
              [...new Set([...now.cost.unpriced_models, ...previous.cost.unpriced_models])].join(
                ', ',
              ),
          }
        : note(now.cost.usd, previous.cost.usd, (...args: Parameters<typeof host.money>) =>
            host.money(...args),
          ),
      A.days === 1
        ? 'API-equivalent cost. Cost is recorded per UTC day: this is the whole current UTC day so far, compared with the whole day before.'
        : 'API-equivalent cost. Cost is recorded per UTC day: the last ' +
            A.days +
            ' UTC days count, today so far, compared with the ' +
            A.days +
            ' whole UTC days before.',
      host.costTip,
    );
    metric(
      'Sessions started',
      countText(now.started),
      note(now.started, previous.started, countText),
    );
    metric('Turns', countText(now.turns), note(now.turns, previous.turns, countText));
    const toolNote = note(now.tools, previous.tools, countText);
    if (A.calls_unknown)
      toolNote.tail =
        ' · — for ' + A.calls_unknown + (A.calls_unknown === 1 ? ' session' : ' sessions');
    metric(
      'Tool calls',
      countText(now.tools),
      toolNote,
      countText(now.errors) +
        ' failed (' +
        pct(now.errors, now.tools) +
        ') · previous ' +
        rangeName +
        ': ' +
        countText(previous.errors) +
        ' failed (' +
        pct(previous.errors, previous.tools) +
        ')',
    );
    metric(
      'Peak concurrency',
      countText(now.peak),
      note(now.peak, previous.peak, countText),
      'The most sessions busy at the same moment.',
    );
    metric(
      'Waited on you',
      timeText(now.wait_ms),
      note(now.wait_ms, previous.wait_ms, timeText),
      'Median wait ' +
        timeText(now.median_wait_ms) +
        ' · previous ' +
        rangeName +
        ': ' +
        timeText(previous.median_wait_ms),
    );
    const wait = A.longest_current_wait;
    metric(
      'Longest current wait',
      wait ? timeText(wait.ms) : '—',
      note(wait ? wait.ms : 0, previous.longest_wait_ms, timeText),
      wait
        ? nameOfSid(A, wait.sid) + ' has waited on you for ' + timeText(wait.ms)
        : 'No session is waiting on you',
    );
    charts.push(makeChart(false), makeChart(true));
    for (const [key, title, groups] of [
      ['repo', 'By repo', A.breakdown.repo],
      ['machine', 'By machine', A.breakdown.machine],
      ['harness', 'By harness and model', A.breakdown.model],
    ] satisfies [string, string, AnalyticsData['breakdown']['repo']][]) {
      const keyFor = (g: AnalyticsData['breakdown']['repo'][number]) =>
        key === 'repo'
          ? (g.repo ?? '__none__')
          : key === 'machine'
            ? (g.machine ?? '')
            : g.harness + '\u0000' + g.model;
      const labelFor = (id: string) =>
        key === 'repo'
          ? id === '__none__'
            ? 'No repo (roles)'
            : id
          : key === 'machine'
            ? (host.machines[id] ?? id)
            : (HARNESS[id.split('\u0000')[0]] ?? id.split('\u0000')[0]) +
              ' · ' +
              shortModel(id.split('\u0000')[1]);
      const selected = (g: AnalyticsData['breakdown']['repo'][number]) =>
          host.measure === 'cost' ? g.usd : g.ms,
        rows = [...groups].sort(
          (a, b) =>
            selected(b) - selected(a) || labelFor(keyFor(a)).localeCompare(labelFor(keyFor(b))),
        ),
        max = Math.max(1, ...rows.map(selected));
      breakdowns.push({
        key,
        heading: title,
        rows: rows.map((g) => ({
          key: keyFor(g),
          name: labelFor(keyFor(g)),
          count: g.sessions + (g.sessions === 1 ? ' session' : ' sessions'),
          width: widthOf(selected(g), max),
          color:
            key === 'harness' ? (keyFor(g).startsWith('claude') ? 'claude' : 'codex') : undefined,
          hours: hoursText(g.ms),
          cost: g.incomplete_usage || g.unpriced_models.length ? '—' : host.money(g.usd),
          missing: g.incomplete_usage
            ? 'copied usage; original owner unknown'
            : g.unpriced_models.length
              ? 'no price for ' + g.unpriced_models.join(', ')
              : undefined,
        })),
      });
    }
    const topLists: [
      string,
      AnalyticsSliceItem[],
      (x: AnalyticsSliceItem) => string,
      (x: AnalyticsSliceItem) => number,
    ][] = [
      [
        'Top sessions · busy time',
        A.top.busy,
        (x) => timeText('ms' in x ? x.ms : 0),
        (x) => ('ms' in x ? x.ms : 0),
      ],
      [
        'Top sessions · waited on',
        A.top.waited,
        (x) => timeText('ms' in x ? x.ms : 0),
        (x) => ('ms' in x ? x.ms : 0),
      ],
      [
        'Most expensive sessions · API-equivalent cost',
        A.top.cost,
        (x) => ('usd' in x && x.usd != null ? host.money(x.usd) : '—'),
        (x) => ('usd' in x ? (x.usd ?? 0) : 0),
      ],
    ];
    for (const [title, items, value, measure] of topLists) {
      const max = Math.max(1, ...items.map(measure));
      lists.push({
        heading: title,
        rows: items.map((item) => {
          const harness = harnessOfSid(A, item.sid),
            missing = 'unpriced_models' in item ? item.unpriced_models : [];
          return {
            id: item.sid,
            name: nameOfSid(A, item.sid),
            harness,
            harnessName: HARNESS_SHORT[harness] ?? harness,
            harnessTip:
              HARNESS[harness] && HARNESS[harness] !== HARNESS_SHORT[harness]
                ? HARNESS[harness]
                : undefined,
            mark: host.mark(harness),
            value: value(item),
            href: host.sessionHref(item.sid),
            rank: widthOf(measure(item), max),
            missing: missing.length ? 'no price for ' + missing.join(', ') : undefined,
          };
        }),
      });
    }
    if (A.models) {
      const reasons = A.models.unknown_reasons ?? {};
      models = [];
      for (const band of ['easy', 'medium', 'hard', 'unknown']) {
        const rows = (A.models.groups ?? []).filter((g) => g.band === band);
        if (!rows.length) continue;
        const points = rows.filter(
            (g) => g.first_pass_acceptance != null && g.median_cost_usd != null,
          ),
          W = Math.max(280, host.width()),
          maxCost = Math.max(0.01, ...points.map((g) => g.median_cost_usd ?? 0));
        models.push({
          key: band,
          heading:
            band === 'unknown' ? 'Unknown difficulty' : band[0].toUpperCase() + band.slice(1),
          width: W,
          rows: rows.map((g) => {
            const key = band + '\u0000' + g.model;
            modelGroups.set(key, g);
            const tokens = g.tokens ?? {};
            return {
              key,
              name: shortModel(g.model),
              tip: g.model,
              count: g.n + (g.small_sample ? ' · small sample' : ''),
              cells: [
                ...measures.map(([, valueKey, nKey, reason, format]) => ({
                  text: g[valueKey] == null ? 'Unknown' : format(g[valueKey]!),
                  n: g[nKey] ?? 0,
                  tip: g[valueKey] == null ? reasons[reason] : undefined,
                })),
                {
                  text: g.tokens_n
                    ? [
                        tokens.input ?? 0,
                        tokens.output ?? 0,
                        (tokens.cache_read ?? 0) + (tokens.cache_write ?? 0),
                      ]
                        .map(countText)
                        .join(' / ')
                    : 'Unknown',
                  n: g.tokens_n ?? 0,
                },
              ],
            };
          }),
          points: points.map((g) => ({
            x: 42 + (g.median_cost_usd! / maxCost) * (W - 58),
            y: 150 - g.first_pass_acceptance! * 130,
            label: shortModel(g.model),
            tip:
              g.model +
              ': ' +
              host.money(g.median_cost_usd ?? 0) +
              ', ' +
              (g.first_pass_acceptance! * 100).toFixed(0) +
              '% accepted; acceptance n=' +
              g.acceptance_n +
              ', cost n=' +
              g.cost_n,
          })),
        });
      }
    }
    const limits = A.allowance;
    if (limits?.recorded_at != null && limits.windows?.length)
      allowance = {
        when: new Date(limits.recorded_at).toLocaleString([], {
          hour: 'numeric',
          minute: '2-digit',
        }),
        windows: limits.windows.map((limit) => ({
          label:
            limit.minutes === 300
              ? '5-hour window'
              : limit.minutes === 10080
                ? 'Weekly window'
                : limit.minutes + '-minute window',
          used: limit.used_percent + '% used',
          reset:
            'Resets ' +
            new Date(limit.resets_at).toLocaleString([], {
              weekday: 'short',
              hour: 'numeric',
              minute: '2-digit',
            }),
        })),
      };
  }
  renderAnalyticsScreen(
    page,
    {
      heading,
      error,
      ready: !!A,
      query: host.query,
      metrics,
      charts,
      breakdowns,
      lists,
      models,
      measure: host.measure,
      allowance,
      modelHeaders: [
        'Model',
        'Work items',
        ...measures.map((m) => m[0]),
        'Tokens · input / output / cache',
      ],
    },
    {
      committed: actions.committed,
      facets: actions.facets,
      session: actions.session,
      measure: actions.measure,
      breakdown: actions.breakdown,
      slice(key) {
        const slice = slices.get(key);
        if (slice && A) actions.slice(A, slice.bin, slice.costMode);
      },
      model(key) {
        const group = modelGroups.get(key);
        if (group) actions.model(group);
      },
    },
  );
}
