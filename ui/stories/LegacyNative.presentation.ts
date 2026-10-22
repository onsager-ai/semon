import recorded from './LegacyNative.records.json';
import { array, dictionary, object, text } from '../src/domain/validate';
import { parseCatalogPage } from '../src/state/catalog-wire';
import { parseCatalogCapabilities } from '../src/state/catalog-capabilities';
import { parseCatalogTranscriptPage } from '../src/state/catalog-transcript-wire';
import { parseNativeRelationships } from '../src/domain/relationships';
import { catalogTranscriptBlocks, recordedRelationshipViews } from '../src/app/catalogTranscriptView';
import { selectedSessionFooter } from '../src/app/sessionPresentation';
import { parseCausalTrace } from '../src/app/tracePresentation';
import { clock } from '../src/domain/format';
import type { SessionSnapshot } from '../src/lib/transcript';
import type { ViewerRoute } from '../src/lib/routes';

// Raw, sanitized publications recorded from the current-head synthetic server in
// CI. Production parsers and presentation owners interpret them. The empty seed
// deliberately fails rendering until that exact-head producer has run.
export function nativePresentation(scope: string, frame: string) {
  const manifest = object(recorded);
  if (manifest.version !== 1 || typeof manifest.sourceRevision !== 'string')
    throw Error('Run the CI native presentation producer before building legacy stories');
  const record = object(object(object(manifest.scopes)[scope])[frame]);
  const responses = array(record.responses, (value) => {
    const row = object(value);
    return { url: new URL(text(row.path), 'http://fixture.invalid'), value: row.value };
  });
  const route = parseRoute(record.route);
  const source = record.source == null ? undefined : text(record.source);
  const sourceLabels = dictionary(record.sourceLabels, text),
    sourceKeys = dictionary(record.sourceKeys, text);
  const label = (key: string) => sourceLabels[Object.keys(sourceKeys).find((id) => sourceKeys[id] === key) ?? key] ?? key;
  const values = (pathname: string) => responses.filter((row) => row.url.pathname === pathname);
  const catalogPages = values('/api/sessions').map((row) => ({ url: row.url, page: parseCatalogPage(row.value) }));
  const catalogPage = catalogPages.filter((row) => !row.url.searchParams.has('sid')).at(-1)?.page;
  const capabilitiesValue = values('/api/session-capabilities').at(-1);
  const capabilities = capabilitiesValue ? parseCatalogCapabilities(capabilitiesValue.value) : undefined;
  const catalog = catalogPage?.items ?? catalogPages.flatMap((row) => row.page.items);
  const catalogRows = [...new Map(catalog.map((row) => [JSON.stringify([row.machine?.key, row.key]), row])).values()];
  function api(path: string): Promise<unknown> {
    const requested = new URL(path, 'http://fixture.invalid');
    const signature = (url: URL) => JSON.stringify([...url.searchParams].sort(([a], [b]) => a.localeCompare(b)));
    const found = responses.filter((row) => row.url.pathname === requested.pathname && signature(row.url) === signature(requested)).at(-1);
    if (!found) throw Error('Unrecorded presentation request: ' + path);
    return Promise.resolve(structuredClone(found.value));
  }
  const previews = values('/api/session-home').map((row) => object(row.value));
  function preview(owner: string | undefined, generation?: string) {
    return owner ? previews.filter((row) => {
      const identity = object(row.identity);
      return identity.catalog_key === owner && identity.source_key === source &&
        (generation === undefined || row.generation === generation);
    }) : [];
  }
  function event(owner: string | undefined, generation: string | undefined, id: string) {
    return preview(owner, generation).flatMap((row) => parseNativeRelationships(row, Object.keys(object(row.events))))
      .filter((row) => row.id === id).at(-1);
  }
  function selected() {
    if (route.v !== 'session') throw Error('Not a selected session fixture');
    const pages = values('/api/session-transcript').map((row) => parseCatalogTranscriptPage(row.value))
      .filter((row) => row.identity.source_key === source && row.identity.catalog_key === route.id);
    const page = pages.at(-1);
    if (!page) throw Error('Missing native transcript publication');
    const entries = [...new Map(pages.filter((row) => row.projection.generation === page.projection.generation)
      .flatMap((row) => row.entries).map((row) => [row.slot, row])).values()].sort((a, b) => a.slot - b.slot);
    return { page, entries, metadata: page.session };
  }
  function transcript(): SessionSnapshot {
    const { page, entries, metadata } = selected(), context = page.relationship_context;
    const origin = context.handoffs.find((row) => row.edge.kind === 'spawn' && row.edge.to === metadata.key),
      parent = origin && context.sessions[origin.edge.from];
    const blocks = catalogTranscriptBlocks(entries, (entry) => {
      if (!entry.clipped || !entry.field) return null;
      return { note: 'Recorded text preview. ', action: { label: 'Load recorded text', busy: false,
        run() { throw Error('Native text expansion is outside this recorded legacy state'); } } };
    }, (item) => {
      if (item.entry.k !== 'h') return null;
      const id = item.entry.id, relationship = context.handoffs.find((row) => row.edge.id === id);
      if (!relationship) return null;
      const brief = event(relationship.briefOwner, context.previews[relationship.briefOwner ?? ''], id),
        result = event(relationship.resultOwner, context.previews[relationship.resultOwner ?? ''], id);
      const detail = preview(relationship.edge.to, context.previews[relationship.edge.to]).at(-1);
      const activity = detail ? object(detail.session).activity : null;
      const parts = activity == null ? null : array(activity, (value) => value);
      return recordedRelationshipViews(item, relationship, metadata.key, context.sessions, {
        brief: brief?.brief, result: result?.result, answer: brief?.answer,
        activity: parts ? [text(parts[0]), text(parts[1])] : undefined,
        trace: context.turns.find((turn) => turn.sent.includes(id))?.id,
      });
    }, metadata.details?.live_observation === true, context, undefined, metadata.harness, metadata.key,
      metadata.details?.state === 'work' ? metadata.details.activity : undefined);
    const first = entries[0]?.slot ?? 0;
    return {
      id: metadata.key, name: metadata.name, blocks,
      order: [...new Set(entries.map((row) => row.entry.turn ?? row.entry_id))],
      footer: selectedSessionFooter(metadata, origin && parent ? {
        id: origin.edge.from, name: parent.name, status: origin.edge.status, done: origin.edge.done, turn: origin.sourceTurn,
      } : undefined),
      observation: { cached: 'Cached history', stale: 'Stale history', incomplete: 'History is incomplete', unavailable: 'Source history is unavailable' }[page.freshness.state] +
        (context.state === 'incomplete' ? ' · related session context is incomplete' : ''),
      observationSummary: 'History details',
      started: first === 0 && metadata.start != null ? {
        lead: 'Started ' + clock(metadata.start, Date.now()) + ' on\u00a0', machine: label(source!),
      } : undefined,
      before: first > 0 ? { sid: metadata.key, where: 'before', text: 'Load earlier records', disabled: false, busy: false } : undefined,
      empty: entries.length ? undefined : 'No native records in this range.',
    };
  }
  function trace() {
    if (route.v !== 'trace' || !source) throw Error('Not a native trace fixture');
    const value = values('/api/session-trace').filter((row) => row.url.searchParams.get('sid') === route.sid && row.url.searchParams.get('turn') === route.turn).at(-1);
    if (!value) throw Error('Missing native causal publication');
    const trace = parseCausalTrace(value.value, source, route.sid, route.turn);
    trace.metadata = catalogPages.filter((row) => row.url.searchParams.get('sid') === route.sid)
      .flatMap((row) => row.page.items).filter((row) => row.key === route.sid).at(-1);
    for (const edge of trace.handoffs.values()) {
      const owner = trace.previewOwners.get(edge.id),
        brief = event(owner?.brief, trace.previews[owner?.brief ?? ''], edge.id),
        result = event(owner?.result, trace.previews[owner?.result ?? ''], edge.id);
      if (brief?.brief !== undefined) edge.brief = brief.brief;
      if (edge.kind === 'toyou' && brief?.answer !== undefined) edge.answer = brief.answer;
      if (result?.result !== undefined) edge.result = result.result;
    }
    for (const [sid, turns] of Object.entries(trace.turns)) {
      const texts: Record<string, string | null> = {};
      for (const row of preview(sid, trace.previews[sid]))
        Object.assign(texts, dictionary(row.turns, (value) => value == null ? null : text(value)));
      for (const turn of turns) if (turn.u && texts[turn.id] != null) turn.u.text = texts[turn.id]!;
    }
    return trace;
  }
  return { api, route, source, label, transcript, trace, selected, catalogRows, catalogPage, capabilities };
}

function parseRoute(value: unknown): ViewerRoute {
  const row = object(value), v = text(row.v);
  if (v === 'home' || v === 'analytics' || v === 'sessions' || v === 'machines') return { v };
  if (v === 'session' || v === 'machine') return { v, id: text(row.id) };
  if (v === 'trace') return { v, sid: text(row.sid), turn: text(row.turn) };
  throw Error('Unsupported recorded presentation route');
}
