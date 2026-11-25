import { render } from 'preact';
import type { ViewerHost } from '../viewer-host';

type Health = 'green' | 'amber' | 'red';
type Activity = 'running' | 'idle' | 'finished' | 'unknown';
type Agent = { id: string; activity: Activity; last_seen_at: string | null };
type Lane = {
  status: { Lane: string; State: 'active' | 'parked' | 'done'; Next: string };
  health: Health;
  activity: Activity;
  last_update_at: string;
  last_seen_at: string;
  agents: Agent[];
  alerts: string[];
};
type Decision = { id: string; lane: string };
type Budget = {
  id: string;
  label: string;
  spent: number;
  cap: number;
  unit: 'usd' | 'kiro-credits';
  reset_at: string | null;
  run_out_at: string | null;
  breakdown?: { lane?: string; agent?: string; model?: string; spent: number }[];
};
export type ManageData = {
  now: number;
  snapshot_unavailable: boolean;
  snapshot: {
    generated_at: string;
    chief_pm_checked_at: string;
    decisions_waiting: (Decision & {
      question: string;
      recommendation: string;
      waiting_since: string;
    })[];
    decisions_today: (Decision & { summary: string; decided_at: string; link: string })[];
    budgets: Budget[];
    lanes: Lane[];
    hosts: { host: string; temperature_c: number; gate: 'ok' | 'gated'; observed_at: string }[];
  } | null;
  stalls:
    | {
        id: string;
        machine: string | null;
        last_activity: number;
        stall_reason: 'waiting-question' | 'no-output' | 'subagent-unconfirmed' | 'process-gone';
      }[]
    | null;
};

const minute = 60_000;
const displayClock = new Intl.DateTimeFormat('en-GB', {
  timeZone: 'Asia/Shanghai',
  dateStyle: 'medium',
  timeStyle: 'short',
});
const day = new Intl.DateTimeFormat('en-GB', { timeZone: 'Asia/Shanghai', dateStyle: 'short' });
const number = new Intl.NumberFormat('en', { maximumFractionDigits: 8 });
const stallReasons = {
  'waiting-question': 'Waiting on a question',
  'no-output': 'No output',
  'subagent-unconfirmed': 'Subagent completion unconfirmed',
  'process-gone': 'Process gone',
};

function when(time: string | null) {
  if (time === null) return 'Unknown';
  const parsed = Date.parse(time);
  return Number.isFinite(parsed) ? (
    <time dateTime={time}>{displayClock.format(parsed)}</time>
  ) : (
    time
  );
}

function Status({ color, label }: { color: Health; label: string }) {
  return <strong class={`manage-status manage-${color}`}>{label}</strong>;
}

function SourceLink({ link }: { link: string }) {
  const url = new URL(link);
  return url.protocol === 'https:' && !url.username && !url.password ? (
    <a href={url.href} target="_blank" rel="noopener noreferrer">
      Decision log
    </a>
  ) : null;
}

function LaneRow({ lane, now }: { lane: Lane; now: number }) {
  const { Next: next, State: state } = lane.status;
  const dueMarker = ' · due ';
  const split = next.lastIndexOf(dueMarker);
  const milestone = split === -1 ? next : next.slice(0, split);
  const due = split === -1 ? null : next.slice(split + dueMarker.length);
  const stale = state === 'active' && now - Date.parse(lane.last_seen_at) >= 390 * minute;
  const overdue = state === 'active' && due !== null && now > Date.parse(due);
  const schedule = (
    <>
      {milestone} · {due === null ? 'no scheduled due' : <>Due {when(due)}</>}
    </>
  );
  return (
    <li>
      <details class="manage-disclosure">
        <summary class="manage-lane">
          <span class="manage-lane-name">{lane.status.Lane}</span>
          <Status color={lane.health} label={lane.health} />
          <span class="manage-lane-next manage-muted">
            {state} · {schedule}
          </span>
        </summary>
        <div class="manage-detail">
          <h3>Lane details</h3>
          <p>{schedule}</p>
          {(overdue || stale) && (
            <div class="manage-meta">
              {overdue && <Status color="red" label="Overdue" />}
              {stale && <Status color="amber" label="Stale heartbeat" />}
            </div>
          )}
          <dl class="manage-facts">
            <div>
              <dt>Last update</dt>
              <dd>{when(lane.last_update_at)}</dd>
            </div>
            <div>
              <dt>Run activity</dt>
              <dd>{lane.activity}</dd>
            </div>
            <div>
              <dt>Last seen</dt>
              <dd>{when(lane.last_seen_at)}</dd>
            </div>
          </dl>
          {lane.agents.length > 0 && (
            <>
              <h3>Agents</h3>
              <ul class="manage-rows">
                {lane.agents.map((agent) => (
                  <li key={agent.id}>
                    <strong>{agent.id}</strong>
                    <p class="manage-muted">
                      Run activity {agent.activity} · Last seen {when(agent.last_seen_at)}
                    </p>
                  </li>
                ))}
              </ul>
            </>
          )}
          {lane.alerts.length > 0 && (
            <>
              <h3>Lane alerts</h3>
              <ul class="manage-rows">
                {lane.alerts.map((alert, i) => (
                  <li key={i}>{alert}</li>
                ))}
              </ul>
            </>
          )}
        </div>
      </details>
    </li>
  );
}

type WaitingDecision = NonNullable<ManageData['snapshot']>['decisions_waiting'][number];

function WaitingRows({ decisions }: { decisions: WaitingDecision[] }) {
  return (
    <ul class="manage-rows">
      {decisions.map((item) => (
        <li key={item.id}>
          <h3>{item.question}</h3>
          <p>
            <strong>Recommendation</strong> · {item.recommendation}
          </p>
          <p class="manage-muted">
            {item.id} · {item.lane} · Waiting since {when(item.waiting_since)}
          </p>
        </li>
      ))}
    </ul>
  );
}

function BudgetRow({ budget }: { budget: Budget }) {
  return (
    <li>
      <details class="manage-disclosure">
        <summary class="manage-budget">
          <span class="manage-budget-line">
            <span class="manage-budget-name">{budget.label}</span>
            <span class="manage-budget-amount">
              <strong>{number.format(budget.spent)}</strong>{' '}
              <span class="manage-muted">
                of {number.format(budget.cap)} {budget.unit}
              </span>
            </span>
          </span>
          {budget.cap > 0 && (
            <meter
              aria-hidden="true"
              min={0}
              max={budget.cap}
              value={Math.min(budget.spent, budget.cap)}
            />
          )}
        </summary>
        <div class="manage-detail">
          <p>{budget.reset_at ? <>Resets {when(budget.reset_at)}</> : 'Reset unavailable'}</p>
          <p>
            {budget.run_out_at ? (
              <>Forecast to run out {when(budget.run_out_at)}</>
            ) : (
              'Forecast unavailable'
            )}
          </p>
          {budget.breakdown?.length ? (
            <ul class="manage-rows">
              {budget.breakdown.map((row, i) => (
                <li key={i}>
                  {[row.lane, row.agent, row.model].filter(Boolean).join(' · ')} ·{' '}
                  {number.format(row.spent)} {budget.unit}
                </li>
              ))}
            </ul>
          ) : null}
        </div>
      </details>
    </li>
  );
}

const overviewLimit = 3;

export function ManageDashboard({ data }: { data: ManageData }) {
  const { now, snapshot, stalls } = data;
  const generated = snapshot ? Date.parse(snapshot.generated_at) : NaN;
  const age = Number.isFinite(generated) ? Math.floor(Math.max(0, now - generated) / minute) : null;
  const color = age === null || age >= 180 ? 'red' : age >= 90 ? 'amber' : 'green';
  const state = { green: 'Current', amber: 'Aging', red: 'Stale' }[color];
  const decisions = snapshot?.decisions_today.filter((decision) => {
    const parsed = Date.parse(decision.decided_at);
    return !Number.isFinite(parsed) || day.format(parsed) === day.format(now);
  });
  const alerts = snapshot?.lanes
    .flatMap((lane) => lane.alerts.map((message) => ({ lane: lane.status.Lane, message })))
    .slice(0, overviewLimit);
  return (
    <div class="manage-dashboard">
      <header class="manage-header">
        <div>
          <h1>Operations overview</h1>
          {snapshot ? (
            <p class="manage-meta">
              <Status color={color} label={state} />
              <span class="manage-muted">
                {age === null ? 'Snapshot age unknown' : `Snapshot ${age} min old`}
              </span>
            </p>
          ) : (
            <p>
              {data.snapshot_unavailable
                ? 'Snapshot storage unavailable.'
                : 'No snapshot received.'}
            </p>
          )}
        </div>
        {snapshot && (
          <div class="manage-snapshot-times manage-muted">
            <p>Generated {when(snapshot.generated_at)}</p>
            <p>Chief PM checked {when(snapshot.chief_pm_checked_at)}</p>
            <p>Times shown in Asia/Shanghai</p>
          </div>
        )}
      </header>
      {snapshot && (
        <>
          <div class="manage-grid manage-exceptions">
            <section class="manage-card" aria-labelledby="manage-alerts">
              <h2 id="manage-alerts">Lane alerts</h2>
              {alerts?.length ? (
                <ul class="manage-rows manage-alerts">
                  {alerts.map(({ lane, message }, i) => (
                    <li key={i}>
                      <h3>{lane}</h3>
                      <p>{message}</p>
                    </li>
                  ))}
                </ul>
              ) : (
                <p class="manage-muted">No lane alerts.</p>
              )}
            </section>
            <section class="manage-card" aria-labelledby="manage-waiting">
              <h2 id="manage-waiting">Waiting on the owner</h2>
              {snapshot.decisions_waiting.length ? (
                <WaitingRows decisions={snapshot.decisions_waiting.slice(0, overviewLimit)} />
              ) : (
                <p class="manage-muted">No decisions waiting.</p>
              )}
              {snapshot.decisions_waiting.length > overviewLimit && (
                <details class="manage-disclosure manage-more">
                  <summary>
                    {snapshot.decisions_waiting.length - overviewLimit} more decisions
                  </summary>
                  <div class="manage-detail">
                    <WaitingRows decisions={snapshot.decisions_waiting.slice(overviewLimit)} />
                  </div>
                </details>
              )}
            </section>
          </div>
          <div class="manage-grid manage-overview">
            <section class="manage-card" aria-labelledby="manage-lanes">
              <h2 id="manage-lanes">Lane health</h2>
              {snapshot.lanes.length ? (
                <ul class="manage-rows manage-disclosure-list">
                  {snapshot.lanes.map((lane) => (
                    <LaneRow key={lane.status.Lane} lane={lane} now={now} />
                  ))}
                </ul>
              ) : (
                <p class="manage-muted">No lanes reported.</p>
              )}
            </section>
            <section class="manage-card" aria-labelledby="manage-spend">
              <h2 id="manage-spend">Spend</h2>
              {snapshot.budgets.length ? (
                <ul class="manage-rows manage-disclosure-list">
                  {snapshot.budgets.map((budget) => (
                    <BudgetRow key={budget.id} budget={budget} />
                  ))}
                </ul>
              ) : (
                <p class="manage-muted">No budgets reported.</p>
              )}
            </section>
          </div>
        </>
      )}
      <details class="manage-card manage-disclosure manage-metrics" open={!snapshot || undefined}>
        <summary>More metrics and decisions</summary>
        <div class="manage-grid manage-detail">
          {snapshot && (
            <section aria-labelledby="manage-decisions">
              <h2 id="manage-decisions">Decisions made without the owner today</h2>
              {decisions?.length ? (
                <ul class="manage-rows">
                  {decisions.map((decision) => (
                    <li key={decision.id}>
                      <h3>{decision.summary}</h3>
                      <p class="manage-muted">
                        {decision.id} · {decision.lane} · {when(decision.decided_at)}
                      </p>
                      <SourceLink link={decision.link} />
                    </li>
                  ))}
                </ul>
              ) : (
                <p class="manage-muted">No decisions recorded today.</p>
              )}
            </section>
          )}
          <section aria-labelledby="manage-hosts">
            <h2 id="manage-hosts">Hosts</h2>
            {snapshot &&
              (snapshot.hosts.length ? (
                <ul class="manage-rows">
                  {snapshot.hosts.map((host) => (
                    <li key={host.host}>
                      <h3>{host.host}</h3>
                      <p class="manage-meta">
                        {number.format(host.temperature_c)} °C{' '}
                        <Status color={host.gate === 'ok' ? 'green' : 'red'} label={host.gate} />
                      </p>
                      <p class="manage-muted">Observed {when(host.observed_at)}</p>
                    </li>
                  ))}
                </ul>
              ) : (
                <p class="manage-muted">No hosts reported.</p>
              ))}
            <h3 class="manage-stalls-heading">Stalled sessions</h3>
            {stalls === null ? (
              <p>Catalog unavailable.</p>
            ) : stalls.length ? (
              <ul class="manage-rows">
                {stalls.map((stall) => (
                  <li key={stall.id}>
                    <p>
                      <strong>{stall.id}</strong>
                      {stall.machine ? ` · ${stall.machine}` : ''}
                    </p>
                    <p class="manage-muted">
                      {stallReasons[stall.stall_reason]} · Last activity{' '}
                      {when(new Date(stall.last_activity).toISOString())}
                    </p>
                  </li>
                ))}
              </ul>
            ) : (
              <p class="manage-muted">No stalled sessions.</p>
            )}
          </section>
        </div>
      </details>
    </div>
  );
}

export const receiverHost: ViewerHost = {
  machinesPath: '/machines',
  receiver: true,
  navigation: {
    trailing: [
      {
        key: 'manage',
        label: 'Manage',
        href: '/manage',
        icon: 'M4 5h16v14H4zM8 9h8M8 14h8',
        current: false,
      },
    ],
  },
  pages: {
    match: (url) => (url.pathname === '/manage' ? { title: 'Manage', nav: 'manage' } : null),
    async load(_url, signal) {
      const response = await fetch('/api/viewer/manage', { signal, cache: 'no-store' });
      if (!response.ok) throw new Error('Management dashboard unavailable');
      // The same-origin receiver validates snapshots before committing them and
      // constructs the response and Catalog projection behind owner authorization.
      const data: ManageData = await response.json();
      const element = document.createElement('div');
      element.className = 'manage-page';
      return {
        element,
        title: 'Manage',
        nav: 'manage',
        mount() {
          render(<ManageDashboard data={data} />, element);
        },
        destroy() {
          render(null, element);
        },
      };
    },
  },
};
