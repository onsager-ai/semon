import type { JsonObject } from './model';
export interface ControlRequest {
  id: string;
  kind: 'permission' | 'question';
  payload: JsonObject;
  hash: string | null;
  state: { state: string; reason?: string };
  reason: string | null;
  remainingMs: number;
}
/** Durable runtime state and native-observation freshness remain distinct. */
export interface ControlRuntimeStatus {
  state: 'active' | 'disconnected' | 'failed' | 'ended' | 'unavailable';
  phase: string;
  freshness: 'current' | 'updating' | 'stale' | 'unavailable';
  reconnectable: boolean;
  presence?: 'active' | 'absent' | 'paused' | 'transitioning' | 'failed' | 'unknown';
  observedAt?: string | null;
  observationError?: string | null;
  updating?: boolean;
}
export interface ControlSnapshot {
  runtime?: ControlRuntimeStatus;
  thread: string;
  generation: string;
  activeTurn: string | null;
  connected: boolean;
  capabilities: Record<
    'input' | 'steer' | 'interrupt' | 'commandApproval' | 'fileApproval' | 'questions',
    boolean
  >;
  reason: string | null;
  requests: ControlRequest[];
  actions: JsonObject;
}
function record(value: unknown): value is JsonObject {
  return !!value && typeof value === 'object' && !Array.isArray(value);
}
export function parseControl(value: unknown): ControlSnapshot | null {
  if (value == null) return null;
  if (
    !record(value) ||
    typeof value.thread !== 'string' ||
    typeof value.generation !== 'string' ||
    !(value.activeTurn === null || typeof value.activeTurn === 'string') ||
    typeof value.connected !== 'boolean' ||
    !(value.reason === null || typeof value.reason === 'string') ||
    !record(value.capabilities) ||
    !['input', 'steer', 'interrupt', 'commandApproval', 'fileApproval', 'questions'].every(
      (key) => record(value.capabilities) && typeof value.capabilities[key] === 'boolean',
    ) ||
    !Array.isArray(value.requests) ||
    value.requests.length > 1280 ||
    !record(value.actions)
  )
    throw new Error('Invalid control snapshot');
  if (
    value.runtime !== undefined &&
    (!record(value.runtime) ||
      !['active', 'disconnected', 'failed', 'ended', 'unavailable'].includes(
        String(value.runtime.state),
      ) ||
      typeof value.runtime.phase !== 'string' ||
      value.runtime.phase.length > 64 ||
      !['current', 'updating', 'stale', 'unavailable'].includes(String(value.runtime.freshness)) ||
      typeof value.runtime.reconnectable !== 'boolean')
  )
    throw new Error('Invalid runtime status');
  if (
    record(value.runtime) &&
    ((value.runtime.presence !== undefined &&
      !['active', 'absent', 'paused', 'transitioning', 'failed', 'unknown'].includes(
        String(value.runtime.presence),
      )) ||
      (value.runtime.observedAt !== undefined &&
        value.runtime.observedAt !== null &&
        (typeof value.runtime.observedAt !== 'string' ||
          value.runtime.observedAt.length > 64 ||
          !Number.isFinite(Date.parse(value.runtime.observedAt)))) ||
      (value.runtime.observationError !== undefined &&
        value.runtime.observationError !== null &&
        (typeof value.runtime.observationError !== 'string' ||
          value.runtime.observationError.length > 512)) ||
      (value.runtime.updating !== undefined && typeof value.runtime.updating !== 'boolean') ||
      (value.runtime.state !== 'active' &&
        (value.runtime.reconnectable ||
          value.connected ||
          ['input', 'steer', 'interrupt', 'commandApproval', 'fileApproval', 'questions'].some(
            (key) => record(value.capabilities) && value.capabilities[key] === true,
          ))))
  )
    throw new Error('Invalid runtime authority');
  for (const request of value.requests) {
    if (
      !record(request) ||
      typeof request.id !== 'string' ||
      !/^[0-9a-f]{32}$/.test(request.id) ||
      !['permission', 'question'].includes(String(request.kind)) ||
      !record(request.payload) ||
      !(
        request.hash === null ||
        (typeof request.hash === 'string' && /^[0-9a-f]{64}$/.test(request.hash))
      ) ||
      !record(request.state) ||
      typeof request.state.state !== 'string' ||
      !(request.reason === null || typeof request.reason === 'string') ||
      typeof request.remainingMs !== 'number' ||
      !Number.isFinite(request.remainingMs) ||
      request.remainingMs < 0
    )
      throw new Error('Invalid control request');
  }
  return value as unknown as ControlSnapshot;
}
export interface ControlView {
  canReconnect?: boolean;
  snapshot: ControlSnapshot;
  busy: boolean;
  uncertain: boolean;
  note: string;
  send(text: string): Promise<boolean>;
  interrupt(): void;
  answer(request: ControlRequest, answer: JsonObject): void;
  reconnect(): void;
}
