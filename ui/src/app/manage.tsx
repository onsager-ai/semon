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
  return (
    <li>
      <h3>{lane.status.Lane}</h3>
      <p>
        <Status color={lane.health} label={lane.health} /> · {state}
      </p>
      <p>
        {milestone} · {due === null ? 'no scheduled due' : <>Due {when(due)}</>}
      </p>
      {overdue && (
        <p>
          <Status color="red" label="Overdue" />
        </p>
      )}
      {stale && (
        <p>
          <Status color="amber" label="Stale heartbeat" />
        </p>
      )}
      <p>Last update {when(lane.last_update_at)}</p>
      <p>
        Run activity {lane.activity} · Last seen {when(lane.last_seen_at)}
      </p>
      {lane.agents.length > 0 && (
        <ul>
          {lane.agents.map((agent) => (
            <li key={agent.id}>
              <p>
                <strong>{agent.id}</strong>
              </p>
              <p>
                Run activity {agent.activity} · Last seen {when(agent.last_seen_at)}
              </p>
            </li>
          ))}
        </ul>
      )}
      {lane.alerts.map((alert, i) => (
        <p key={i} role="note">
          Alert · {alert}
        </p>
      ))}
    </li>
  );
}

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
  return (
    <div class="manage-dashboard">
      <header>
        {snapshot ? (
          <>
            <p>
              <Status color={color} label={state} /> ·{' '}
              {age === null ? 'Age unknown' : `${age} min old`}
            </p>
            <p>Generated {when(snapshot.generated_at)}</p>
            <p>Chief PM checked {when(snapshot.chief_pm_checked_at)}</p>
            <p class="manage-muted">Times shown in Asia/Shanghai</p>
          </>
        ) : (
          <p>
            {data.snapshot_unavailable ? 'Snapshot storage unavailable.' : 'No snapshot received.'}
          </p>
        )}
      </header>
      {snapshot && (
        <>
          <section aria-labelledby="manage-waiting">
            <h2 id="manage-waiting">Waiting on the owner</h2>
            {snapshot.decisions_waiting.length ? (
              <ul class="manage-rows">
                {snapshot.decisions_waiting.map((item) => (
                  <li key={item.id}>
                    <h3>{item.question}</h3>
                    <p>
                      {item.id} · {item.lane}
                    </p>
                    <p>Recommendation · {item.recommendation}</p>
                    <p>Waiting since {when(item.waiting_since)}</p>
                  </li>
                ))}
              </ul>
            ) : (
              <p>No decisions waiting.</p>
            )}
          </section>
          <section aria-labelledby="manage-spend">
            <h2 id="manage-spend">Spend</h2>
            {snapshot.budgets.length ? (
              <ul class="manage-rows">
                {snapshot.budgets.map((budget) => (
                  <li key={budget.id}>
                    <h3>{budget.label}</h3>
                    <p>
                      {number.format(budget.spent)} spent of {number.format(budget.cap)}{' '}
                      {budget.unit}
                    </p>
                    <p>
                      {budget.reset_at ? <>Resets {when(budget.reset_at)}</> : 'Reset unavailable'}
                    </p>
                    <p>
                      {budget.run_out_at ? (
                        <>Forecast to run out {when(budget.run_out_at)}</>
                      ) : (
                        'Forecast unavailable'
                      )}
                    </p>
                    {budget.breakdown?.length ? (
                      <ul>
                        {budget.breakdown.map((row, i) => (
                          <li key={i}>
                            {[row.lane, row.agent, row.model].filter(Boolean).join(' · ')} ·{' '}
                            {number.format(row.spent)} {budget.unit}
                          </li>
                        ))}
                      </ul>
                    ) : null}
                  </li>
                ))}
              </ul>
            ) : (
              <p>No budgets reported.</p>
            )}
          </section>
          <section aria-labelledby="manage-lanes">
            <h2 id="manage-lanes">Lane health</h2>
            {snapshot.lanes.length ? (
              <ul class="manage-rows">
                {snapshot.lanes.map((lane) => (
                  <LaneRow key={lane.status.Lane} lane={lane} now={now} />
                ))}
              </ul>
            ) : (
              <p>No lanes reported.</p>
            )}
          </section>
          <section aria-labelledby="manage-decisions">
            <h2 id="manage-decisions">Decisions made without the owner today</h2>
            {decisions?.length ? (
              <ul class="manage-rows">
                {decisions.map((decision) => (
                  <li key={decision.id}>
                    <h3>{decision.summary}</h3>
                    <p>
                      {decision.id} · {decision.lane}
                    </p>
                    <p>{when(decision.decided_at)}</p>
                    <SourceLink link={decision.link} />
                  </li>
                ))}
              </ul>
            ) : (
              <p>No decisions recorded today.</p>
            )}
          </section>
        </>
      )}
      <section aria-labelledby="manage-hosts">
        <h2 id="manage-hosts">Hosts</h2>
        {snapshot &&
          (snapshot.hosts.length ? (
            <ul class="manage-rows">
              {snapshot.hosts.map((host) => (
                <li key={host.host}>
                  <h3>{host.host}</h3>
                  <p>
                    {number.format(host.temperature_c)} °C ·{' '}
                    <Status color={host.gate === 'ok' ? 'green' : 'red'} label={host.gate} />
                  </p>
                  <p>Observed {when(host.observed_at)}</p>
                </li>
              ))}
            </ul>
          ) : (
            <p>No hosts reported.</p>
          ))}
        <h3>Stalled sessions</h3>
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
                <p>
                  {stallReasons[stall.stall_reason]} · Last activity{' '}
                  {when(new Date(stall.last_activity).toISOString())}
                </p>
              </li>
            ))}
          </ul>
        ) : (
          <p>No stalled sessions.</p>
        )}
      </section>
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
