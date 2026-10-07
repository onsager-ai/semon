import { boolean, number, object, text } from '../domain/validate';
import { parseCatalogIdentity, parseCatalogSource } from './catalog-wire';
import type { CatalogTranscriptEntry } from './catalog-transcript-wire';
export interface CatalogFieldRequest {
  source_key: string;
  catalog_key: string;
  generation: string;
  entry: CatalogTranscriptEntry;
  chunk: number;
}
/** Qualified native string chunks never use a workspace tool or transcript endpoint. */
export function parseCatalogField(value: unknown, request: CatalogFieldRequest) {
  const row = object(value),
    identity = parseCatalogIdentity(row.identity),
    projection = object(row.projection),
    field = object(row.field),
    provenance = object(row.provenance),
    source = parseCatalogSource(provenance.source),
    chunk = number(field.chunk),
    next = field.next === null ? null : number(field.next),
    complete = boolean(field.complete),
    body = text(row.text),
    sourceBytes = number(object(row.observation).source_bytes),
    layers = field.layers ?? 1;
  const sourceLimit = layers === 2 ? 65607 : 65547;
  if (
    row.api !== 1 ||
    identity.read_scope !== 'retained_history' ||
    identity.source_key !== request.source_key ||
    identity.catalog_key !== request.catalog_key ||
    projection.version !== 5 ||
    projection.generation !== request.generation ||
    row.slot !== request.entry.slot ||
    field.name !== request.entry.field?.name ||
    (layers !== 1 && layers !== 2) ||
    layers !== (request.entry.field?.layers ?? 1) ||
    chunk !== request.chunk ||
    !Number.isSafeInteger(chunk) ||
    chunk < 0 ||
    next !== (complete ? null : chunk + 1) ||
    !request.entry.field ||
    chunk >= request.entry.field.chunks ||
    complete !== (chunk + 1 === request.entry.field.chunks) ||
    object(row.freshness).state !== 'cached' ||
    !Number.isSafeInteger(sourceBytes) ||
    sourceBytes < 0 ||
    sourceBytes > sourceLimit ||
    new TextEncoder().encode(body).byteLength > 131072 ||
    JSON.stringify(source) !== JSON.stringify(request.entry.provenance.source) ||
    provenance.offset !== request.entry.provenance.offset ||
    provenance.block !== request.entry.provenance.block ||
    provenance.native_event_id !== request.entry.provenance.native_event_id
  )
    throw new Error('Native field chunk does not match the selected source projection');
  return { text: body, next, complete };
}
