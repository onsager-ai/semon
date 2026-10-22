import { BASE, sample } from './fixture.mjs';

// Same historical-to-native identity binding as pixels.mjs screens/referenceHandoffs.
// Display names, mockup IDs and the final session state never manufacture turns.
export function nativeRoutes(D) {
  const source = sample(),
    handoffs = new Map();
  for (const edge of source.H) {
    const matches = D.H.filter(
      (row) =>
        row.kind === edge.kind &&
        row.from === edge.from &&
        row.to === edge.to &&
        Math.round(((row.at - BASE) / 60000) * 1000) / 1000 === edge.at,
    );
    if (matches.length > 1) throw Error('Ambiguous native fixture handoff ' + edge.id);
    if (matches.length) handoffs.set(matches[0].id, edge.id);
  }
  const routes = {
    home: { v: 'home' },
    drawer: { v: 'home' },
    sessions: { v: 'sessions' },
    machines: { v: 'machines' },
    analytics: { v: 'analytics' },
  };
  for (const id of Object.keys(D.MACHINE)) routes['machine-' + id] = { v: 'machine', id };
  for (const id of Object.keys(D.SESS)) routes['session-' + id] = { v: 'session', id };
  for (const turn of D.turns) {
    if (!turn.sent?.length) continue;
    const slot = (D.TX[turn.sid] ?? []).findIndex((entry) => entry.turn === turn.id);
    const historical =
      slot < 0
        ? turn.id
        : turn.start
          ? handoffs.get(turn.start)
          : source.TX[turn.sid]?.[slot]?.k === 'u'
            ? turn.sid + ':' + slot
            : null;
    if (!historical)
      throw Error('Missing native fixture trace binding ' + turn.sid + '/' + turn.id);
    const frame = 'trace-' + turn.sid + '-' + String(historical).replace(/[^\w-]/g, '_');
    if (routes[frame]) throw Error('Duplicate native trace frame ' + frame);
    routes[frame] = { v: 'trace', sid: turn.sid, turn: turn.id };
  }
  return routes;
}
