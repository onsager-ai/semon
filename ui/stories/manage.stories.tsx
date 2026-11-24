import { render } from 'preact';
import { ManageDashboard, type ManageData } from '../src/app/manage';
import '../../crates/semon-sessions/src/viewer.css';

// CSF for the Storybook HTML renderer. A story root uses the same Preact
// component and tokens as the receiver page.
const fixture: NonNullable<ManageData['snapshot']> = {
  generated_at: '2026-01-01T16:30:00Z',
  chief_pm_checked_at: '2026-01-01T16:25:00Z',
  decisions_waiting: [
    {
      id: 'eval-D1',
      lane: 'Eval · Fixtures',
      question: 'Extend the fixture set?',
      recommendation: 'Finish the current set first.',
      waiting_since: '2026-01-01T15:00:00Z',
    },
  ],
  decisions_today: [
    {
      id: 'eval-D2',
      lane: 'Eval · Fixtures',
      summary: 'Deferred the optional fixture batch.',
      decided_at: '2026-01-01T16:10:00Z',
      link: 'https://example.com/decisions/eval-D2',
    },
  ],
  budgets: [
    {
      id: 'eval',
      label: 'Eval',
      spent: 120,
      cap: 473.5,
      unit: 'usd',
      reset_at: null,
      run_out_at: null,
      breakdown: [
        { lane: 'Eval · Fixtures', agent: 'fixture-review', model: 'example-model', spent: 45 },
      ],
    },
  ],
  lanes: [
    {
      status: {
        Lane: 'Eval · Fixtures',
        State: 'active',
        Next: 'Finish fixture review · due 2026-01-01T17:00Z',
      },
      alerts: ['Review is waiting for a fixture.'],
      health: 'green',
      activity: 'finished',
      last_update_at: '2026-01-01T11:29:00Z',
      last_seen_at: '2026-01-01T11:30:00Z',
      agents: [{ id: 'fixture-review', activity: 'finished', last_seen_at: null }],
    },
  ],
  hosts: [{ host: 'build-1', temperature_c: 64, gate: 'ok', observed_at: '2026-01-01T16:20:00Z' }],
};
const example: ManageData = {
  now: Date.parse('2026-01-01T18:00:00Z'),
  snapshot_unavailable: false,
  snapshot: fixture,
  stalls: [
    {
      id: 'session-fixture',
      machine: 'build-1',
      last_activity: Date.parse('2026-01-01T16:00:00Z'),
      stall_reason: 'no-output',
    },
  ],
};
function renderDashboard(data: ManageData) {
  const element = document.createElement('div');
  render(<ManageDashboard data={data} />, element);
  return element;
}
export default { title: 'Viewer/Management dashboard', render: renderDashboard };
export const Aging = { args: example };
export const CatalogUnavailable = { args: { ...example, stalls: null } };
export const Empty = { args: { ...example, snapshot: null, stalls: [] } };
