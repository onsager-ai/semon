export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
export interface JsonObject { [key: string]: Json }
export interface MachineWire extends JsonObject { id: string; name: string; up: boolean }
export interface SessionWire extends JsonObject { name: string; harness: string; state: string; machine: string; start: number; last: number }
export interface HandoffWire extends JsonObject { id: string; kind: string; from: string; at: number; status: string }
export interface TurnWire extends JsonObject { id: string; sid: string; sent: string[] }
export interface ModelWire extends JsonObject { now: number; version: string; machine: MachineWire; sessions: { [id: string]: SessionWire }; handoffs: HandoffWire[]; turns: TurnWire[] }
function object(value: unknown): value is JsonObject { return !!value && typeof value === 'object' && !Array.isArray(value); }
const strings = (value: unknown): value is string[] => Array.isArray(value) && value.every(item => typeof item === 'string');
function machine(value: unknown): value is MachineWire { return object(value) && typeof value.id === 'string' && typeof value.name === 'string' && typeof value.up === 'boolean'; }
export function parseModel(value: unknown): ModelWire {
  if (!object(value) || !Number.isFinite(value.now) || typeof value.version !== 'string' || !machine(value.machine) || !object(value.sessions) || !Array.isArray(value.handoffs) || !Array.isArray(value.turns)) throw new Error('Invalid model response');
  if (value.machines !== undefined && (!Array.isArray(value.machines) || !value.machines.every(machine))) throw new Error('Invalid machine response');
  for (const session of Object.values(value.sessions)) if (!object(session) || !['name','harness','state','machine'].every(field => typeof session[field] === 'string') || !Number.isFinite(session.start) || !Number.isFinite(session.last)) throw new Error('Invalid session response');
  for (const handoff of value.handoffs) if (!object(handoff) || !['id','kind','from','status'].every(field => typeof handoff[field] === 'string') || !Number.isFinite(handoff.at)) throw new Error('Invalid handoff response');
  for (const turn of value.turns) if (!object(turn) || typeof turn.id !== 'string' || typeof turn.sid !== 'string' || !strings(turn.sent)) throw new Error('Invalid turn response');
  return value as ModelWire;
}
export class ModelStore {
  private current: ModelWire | null = null;
  adopt(value: unknown) { const model = parseModel(value); this.current = structuredClone(model); return model; }
  apply(value: unknown): ModelWire {
    if (!object(value)) throw new Error('Invalid model response');
    if (value.delta !== 1) return parseModel(value);
    if (!this.current || value.from !== this.current.version) throw new Error('Model delta base expired');
    if (value.set !== undefined && !object(value.set)) throw new Error('Invalid model delta');
    const next: JsonObject = { ...this.current, ...(object(value.set) ? value.set : {}) };
    if (value.remove !== undefined && !strings(value.remove)) throw new Error('Invalid model delta removals');
    for (const field of (value.remove ?? []) as string[]) delete next[field];
    if (value.collections !== undefined && !object(value.collections)) throw new Error('Invalid model delta collections');
    for (const [field, patch] of Object.entries(object(value.collections) ? value.collections : {})) {
      if (!object(patch) || patch.set !== undefined && !object(patch.set) || patch.remove !== undefined && !strings(patch.remove) || patch.order !== undefined && !strings(patch.order)) throw new Error('Invalid collection patch');
      const array = field === 'handoffs' || field === 'turns', previous = this.current[field];
      const rows = new Map<string, Json>();
      if (array) { if (!Array.isArray(previous)) throw new Error('Invalid delta base'); for (const row of previous) { if (!object(row) || typeof row.id !== 'string') throw new Error('Invalid delta row'); rows.set(row.id,row); } }
      else { if (previous !== undefined && !object(previous)) throw new Error('Invalid delta base'); for (const [id,row] of Object.entries(object(previous) ? previous : {})) rows.set(id,row); }
      for (const id of (patch.remove ?? []) as string[]) rows.delete(id);
      for (const [id,row] of Object.entries(object(patch.set) ? patch.set : {})) rows.set(id,row);
      if (array) { const order = (patch.order ?? [...rows.keys()]) as string[]; if (order.some(id => !rows.has(id))) throw new Error('Incomplete model delta'); next[field] = order.map(id => rows.get(id)!); }
      else next[field] = Object.fromEntries(rows);
    }
    next.version = value.version ?? null; return parseModel(next);
  }
}
export class ApiError extends Error { constructor(message: string, readonly status: number) { super(message); } }
export async function requestJson(path: string, signal?: AbortSignal, unchanged = false): Promise<unknown> {
  let response: Response;
  try { response = await fetch(path,{ credentials: 'same-origin', signal }); }
  catch (error) { if (error instanceof Error && error.name === 'AbortError') throw error; throw new ApiError(error instanceof Error ? error.message : 'No response',0); }
  if (unchanged && response.status === 304) return null;
  if (!response.ok) throw new ApiError(response.status + ' ' + response.statusText,response.status);
  return response.json() as Promise<unknown>;
}
