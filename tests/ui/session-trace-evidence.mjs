import { createHash } from 'node:crypto';

export const SESSION_TRACE_EVIDENCE_LIMITS = Object.freeze({
  responseBytes: 8 * 1024 * 1024,
  summaryBytes: 1024 * 1024,
  turns: 100,
  handoffs: 1000,
  sessions: 100,
  participants: 128,
  edgesPerTurn: 1000,
  identityIdChars: 4096,
  identityTextChars: 512,
});

const fail = (message) => {
  throw new Error('session-trace evidence: ' + message);
};
const record = (value, label) => {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    fail(label + ' must be an object');
  return value;
};
function string(value, label, limit = SESSION_TRACE_EVIDENCE_LIMITS.identityIdChars) {
  if (typeof value !== 'string' || value.length === 0 || value.length > limit)
    fail(label + ' must be a nonempty string of at most ' + limit + ' characters');
  return value;
}
function optionalString(value, label) {
  return value == null ? null : string(value, label);
}
function list(value, label, limit) {
  if (!Array.isArray(value)) fail(label + ' must be an array');
  if (value.length > limit) fail(label + ' count ' + value.length + ' exceeds limit ' + limit);
  return value;
}
function identityRows(value, label, limit, project) {
  const source = record(value, label),
    entries = Object.entries(source);
  if (entries.length > limit) fail(label + ' count ' + entries.length + ' exceeds limit ' + limit);
  return entries
    .map(([id, item]) => project(string(id, label + ' id'), item))
    .sort((left, right) => left.id.localeCompare(right.id));
}

/** Retain bounded trace identity and graph structure while hashing the exact wire bytes. */
export function summarizeSessionTraceResponse(body) {
  if (!Buffer.isBuffer(body)) fail('response body must be a Buffer');
  if (body.length > SESSION_TRACE_EVIDENCE_LIMITS.responseBytes)
    fail(
      'response bytes ' +
        body.length +
        ' exceeds limit ' +
        SESSION_TRACE_EVIDENCE_LIMITS.responseBytes,
    );
  let data;
  try {
    data = record(JSON.parse(body.toString('utf8')), 'response');
  } catch (error) {
    if (String(error.message).startsWith('session-trace evidence:')) throw error;
    fail('response is not valid JSON');
  }
  if (data.api !== 1) fail('api must equal 1');
  if (typeof data.incomplete !== 'boolean') fail('incomplete must be boolean');
  if (typeof data.previews_incomplete !== 'boolean') fail('previews_incomplete must be boolean');
  const sessions = identityRows(
      data.sessions,
      'sessions',
      SESSION_TRACE_EVIDENCE_LIMITS.sessions,
      (id, value) => {
        const session = record(value, 'session ' + id),
          name = string(
            session.name,
            'session ' + id + ' name',
            SESSION_TRACE_EVIDENCE_LIMITS.identityTextChars,
          ),
          harness = string(
            session.harness,
            'session ' + id + ' harness',
            SESSION_TRACE_EVIDENCE_LIMITS.identityTextChars,
          ),
          kind =
            session.kind == null
              ? null
              : string(
                  session.kind,
                  'session ' + id + ' kind',
                  SESSION_TRACE_EVIDENCE_LIMITS.identityTextChars,
                );
        return { id, name, harness, kind };
      },
    ),
    participants = identityRows(
      data.participants,
      'participants',
      SESSION_TRACE_EVIDENCE_LIMITS.participants,
      (id, value) => ({
        id,
        name: string(
          value,
          'participant ' + id + ' name',
          SESSION_TRACE_EVIDENCE_LIMITS.identityTextChars,
        ),
      }),
    ),
    turns = list(data.turns, 'turns', SESSION_TRACE_EVIDENCE_LIMITS.turns).map((value, index) => {
      const turn = record(value, 'turn ' + index),
        sent = list(
          turn.sent,
          'turn ' + index + ' sent',
          SESSION_TRACE_EVIDENCE_LIMITS.edgesPerTurn,
        ).map((id) => string(id, 'turn ' + index + ' sent id'));
      return {
        id: string(turn.id, 'turn ' + index + ' id'),
        sid: string(turn.sid, 'turn ' + index + ' sid'),
        start: optionalString(turn.start, 'turn ' + index + ' start'),
        sent,
      };
    }),
    handoffs = list(data.handoffs, 'handoffs', SESSION_TRACE_EVIDENCE_LIMITS.handoffs).map(
      (value, index) => {
        const edge = record(value, 'handoff ' + index);
        const kind = string(edge.kind, 'handoff ' + index + ' kind', 32);
        if (!['ask', 'spawn', 'relay', 'move', 'toyou'].includes(kind))
          fail('handoff ' + index + ' kind is not recognized');
        return {
          id: string(edge.id, 'handoff ' + index + ' id'),
          kind,
          from: string(edge.from, 'handoff ' + index + ' from'),
          to: string(edge.to, 'handoff ' + index + ' to'),
        };
      },
    );
  const root = string(data.root, 'root'),
    catalogKey = string(data.catalog_key, 'catalog_key'),
    roots = turns.filter((turn) => turn.id === root);
  if (roots.length !== 1 || roots[0].sid !== catalogKey)
    fail('root does not identify exactly one turn in catalog_key');
  const summary = {
    responseBytes: body.length,
    responseDigest: createHash('sha256').update(body).digest('hex'),
    api: data.api,
    sourceKey: string(data.source_key, 'source_key'),
    catalogKey,
    root,
    generation: string(data.generation, 'generation', 256),
    incomplete: data.incomplete,
    previewsIncomplete: data.previews_incomplete,
    turns,
    handoffs,
    sessions,
    participants,
  };
  const summaryBytes = Buffer.byteLength(JSON.stringify(summary));
  if (summaryBytes > SESSION_TRACE_EVIDENCE_LIMITS.summaryBytes)
    fail(
      'summary bytes ' +
        summaryBytes +
        ' exceeds limit ' +
        SESSION_TRACE_EVIDENCE_LIMITS.summaryBytes,
    );
  return summary;
}
