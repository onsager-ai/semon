import retained from '../../tests/ui/fixture-data/overhaul.json';
import { parseAnalytics } from '../src/state/analytics-wire';
import { edges, fixtureNow, recordedCost, recordedCounts, sessions } from './LegacyRoutes.data';

// Reuse recorded seven-day activity, wait and token facts. Unknown model pricing
// stays unavailable; it is never substituted with zero cost.
export function analytics() {
  const day = 86400000, base = Date.UTC(2026, 8, 28);
  const intervals = Object.entries(sessions).flatMap(([sid, session]) => [
    ...(session.busy ?? []).map(([a, b]) => ({ sid, a, b })),
    ...((retained.ANALYTICS_HISTORY as Record<string, { busy: number[][] }>)[sid]?.busy ?? []).map(([daysAgo, a, b]) => ({ sid, a: base - daysAgo * day + a * 60000, b: base - daysAgo * day + b * 60000 })),
  ]);
  const start = base - 6 * day;
  const columns = Array.from({ length: 7 }, (_, index) => {
    const from = start + index * day, to = Math.min(from + day, fixtureNow);
    const busy = Object.keys(sessions).map((sid) => ({ sid, ms: intervals.filter((row) => row.sid === sid).reduce((total, row) => total + Math.max(0, Math.min(to, row.b) - Math.max(from, row.a)), 0) })).filter((row) => row.ms > 0);
    return { from, to, sessions: busy, more: 0, claude_ms: busy.filter((row) => sessions[row.sid].harness === 'claude').reduce((sum, row) => sum + row.ms, 0), codex_ms: busy.filter((row) => sessions[row.sid].harness === 'codex').reduce((sum, row) => sum + row.ms, 0) };
  });
  const counts = Object.keys(sessions).map(recordedCounts), costs = Object.keys(sessions).map(recordedCost);
  const waits = [
    ...edges.filter((edge) => edge.kind === 'toyou').map((edge) => ({ sid: edge.from, ms: (edge.done ?? fixtureNow) - edge.at })),
    ...retained.ANALYTICS_WAIT_SAMPLES.map((row) => ({ sid: row.sid, ms: (row.start[0] - row.end[0]) * day + (row.end[1] - row.start[1]) * 60000 })),
  ].sort((a, b) => a.ms - b.ms);
  const phases = intervals.flatMap((row) => [[row.a, 1], [row.b, -1]]).sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  let active = 0, peak = 0;
  for (const [, delta] of phases) { active += delta; peak = Math.max(peak, active); }
  const current = { agent_ms: columns.reduce((sum, row) => sum + row.claude_ms + row.codex_ms, 0), started: Object.keys(sessions).length,
    turns: counts.reduce((sum, row) => sum + row.turns, 0), tools: counts.reduce((sum, row) => sum + row.tools, 0), errors: counts.reduce((sum, row) => sum + row.errors, 0),
    peak, wait_ms: waits.reduce((sum, row) => sum + row.ms, 0), median_wait_ms: waits[Math.floor(waits.length / 2)]?.ms ?? 0, longest_wait_ms: waits.at(-1)?.ms ?? 0,
    cost: { usd: costs.some((row) => row.usd === null) ? null : costs.reduce((sum, row) => sum + (row.usd ?? 0), 0), unpriced_models: [...new Set(costs.flatMap((row) => row.unpriced_models ?? []))] } };
  const top = Object.keys(sessions).map((sid) => ({ sid, ms: intervals.filter((row) => row.sid === sid).reduce((sum, row) => sum + row.b - row.a, 0) })).sort((a, b) => b.ms - a.ms);
  function breakdown(field: 'repo' | 'machine' | 'model') {
    const groups = new Map<string, { ms: number; usd: number; sessions: number; unpriced_models: string[] }>();
    for (const [sid, session] of Object.entries(sessions)) {
      const key = session[field] ?? '', cost = recordedCost(sid), row = groups.get(key) ?? { ms: 0, usd: 0, sessions: 0, unpriced_models: [] };
      row.ms += top.find((item) => item.sid === sid)?.ms ?? 0; row.usd += cost.usd ?? 0; row.sessions++; row.unpriced_models.push(...(cost.unpriced_models ?? [])); groups.set(key, row);
    }
    return [...groups].map(([key, row]) => ({ [field]: field === 'repo' && !key ? null : key, ...row }));
  }
  const priced = Object.keys(sessions).map((sid) => ({ sid, usd: recordedCost(sid).usd, unpriced_models: recordedCost(sid).unpriced_models ?? [] }));
  const costDays = columns.map(({ from, to }) => {
    const rows = priced.filter((row) => sessions[row.sid].last >= from && (sessions[row.sid].last < to || to === fixtureNow && sessions[row.sid].last === to));
    return { from, to, sessions: rows, more: 0,
      claude_usd: rows.filter((row) => sessions[row.sid].harness === 'claude').reduce((sum, row) => sum + (row.usd ?? 0), 0),
      codex_usd: rows.filter((row) => sessions[row.sid].harness === 'codex').reduce((sum, row) => sum + (row.usd ?? 0), 0) };
  });
  return parseAnalytics({ days: 7, version: 'retained-overhaul-activity', current, previous: { ...current, agent_ms: 0, started: 0, turns: 0, tools: 0, errors: 0, peak: 0, wait_ms: 0, median_wait_ms: 0, longest_wait_ms: 0, cost: { usd: null, unpriced_models: [] } }, longest_current_wait: waits.at(-1) ?? null, calls_unknown: 0,
    agents: { unit: 'per day', columns }, cost: { days: costDays, unpriced_models: current.cost.unpriced_models, incomplete_usage: current.cost.usd === null }, breakdown: { repo: breakdown('repo'), machine: breakdown('machine'), model: breakdown('model') }, top: { busy: top, waited: [...waits].reverse(), cost: priced },
    sessions: Object.fromEntries(Object.entries(sessions).map(([sid, row]) => [sid, { name: row.name, harness: row.harness }])),
    models: { groups: [], unknown_reasons: {} }, allowance: null, facets: { repo: [...new Set(Object.values(sessions).flatMap((row) => row.repo ? [row.repo] : []))], machine: ['laptop'], harness: ['claude', 'codex'], model: [...new Set(Object.values(sessions).map((row) => row.model ?? ''))] },
  });
}
