/** Readonly durable phase and observation health; this carries no native control authority. */
export interface RuntimeObservation {
  state: 'active' | 'disconnected' | 'failed' | 'ended' | 'unavailable';
  phase: string;
  freshness: 'current' | 'updating' | 'stale' | 'unavailable';
  presence?: 'active' | 'absent' | 'paused' | 'transitioning' | 'failed' | 'unknown';
  observedAt?: string | null;
  observationError?: string | null;
  updating?: boolean;
}
export interface RuntimeObservationView {
  observation: RuntimeObservation | null;
  reason: string | null;
  delivery: 'updating' | 'current' | 'stale' | 'unavailable';
}
export function parseRuntimeObservation(value: unknown): RuntimeObservation {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw new Error('Invalid readonly runtime observation');
  const row = value as Record<string, unknown>;
  if (
    typeof row.state !== 'string' ||
    !['active', 'disconnected', 'failed', 'ended', 'unavailable'].includes(row.state) ||
    typeof row.phase !== 'string' ||
    row.phase.length > 64 ||
    typeof row.freshness !== 'string' ||
    !['current', 'updating', 'stale', 'unavailable'].includes(row.freshness) ||
    (row.presence !== undefined &&
      (typeof row.presence !== 'string' ||
        !['active', 'absent', 'paused', 'transitioning', 'failed', 'unknown'].includes(
          row.presence,
        ))) ||
    (row.observedAt !== undefined &&
      row.observedAt !== null &&
      (typeof row.observedAt !== 'string' ||
        row.observedAt.length > 64 ||
        !Number.isFinite(Date.parse(row.observedAt)))) ||
    (row.observationError !== undefined &&
      row.observationError !== null &&
      (typeof row.observationError !== 'string' || row.observationError.length > 512)) ||
    (row.updating !== undefined && typeof row.updating !== 'boolean')
  )
    throw new Error('Invalid readonly runtime observation');
  return Object.freeze({
    state: row.state as RuntimeObservation['state'],
    phase: row.phase,
    freshness: row.freshness as RuntimeObservation['freshness'],
    ...(row.presence === undefined
      ? {}
      : { presence: row.presence as RuntimeObservation['presence'] }),
    ...(row.observedAt === undefined ? {} : { observedAt: row.observedAt as string | null }),
    ...(row.observationError === undefined
      ? {}
      : { observationError: row.observationError as string | null }),
    ...(row.updating === undefined ? {} : { updating: row.updating as boolean }),
  });
}
