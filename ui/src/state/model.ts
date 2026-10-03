import { ModelStore, parseModel } from '../lib/model';
import { normalizeModel } from '../domain/normalize';
import type { DomainState, Session, Handoff, Turn } from '../domain/types';
function replaceRecord<T>(target: Record<string, T>, next: Record<string, T>) { for (const id of Object.keys(target)) delete target[id]; Object.assign(target, next); }
function replaceMap<T>(target: Map<string, T>, next: Map<string, T>) { target.clear(); for (const [id, row] of next) target.set(id, row); }
/** The raw delta baseline and the normalized graph have one owner and one commit. */
export class ViewerModelStore {
  private readonly raw = new ModelStore();
  readonly sessions: Record<string, Session> = Object.create(null);
  readonly machines: Record<string, string> = Object.create(null);
  readonly machineUp: Record<string, boolean> = Object.create(null);
  readonly machineLast: Record<string, number> = Object.create(null);
  readonly handoffs: Handoff[] = [];
  readonly turns: Record<string, Turn[]> = Object.create(null);
  readonly turn = new Map<string, Turn>(); readonly starts = new Map<string, Turn>();
  readonly holds = new Map<string, Turn>(); readonly handoff = new Map<string, Handoff>();
  apply(value: unknown) { return this.raw.apply(value); }
  adopt(value: unknown) {
    const model = parseModel(value), normalized = normalizeModel(model);
    const machines = model.machines == null ? [parseModelMachine(model.machine)] : array(model.machines, parseModelMachine);
    // Validation of every domain field precedes raw baseline or normalized-state writes.
    this.raw.adopt(model);
    replaceRecord(this.sessions, normalized.sessions); replaceRecord(this.turns, normalized.turns);
    this.handoffs.splice(0, this.handoffs.length, ...normalized.handoffs);
    replaceMap(this.turn, normalized.turn); replaceMap(this.starts, normalized.starts);
    replaceMap(this.holds, normalized.holds); replaceMap(this.handoff, normalized.handoff);
    for (const table of [this.machines, this.machineUp, this.machineLast]) for (const id of Object.keys(table)) delete table[id];
    for (const row of machines) {
      // ModelWire's validated collection still uses transport JSON; narrow its boundary here.
      this.machines[row.id] = row.name; this.machineUp[row.id] = row.up;
      if (row.last !== undefined) this.machineLast[row.id] = row.last;
    }
    return model;
  }
  domain(transcriptMeta: DomainState['transcriptMeta']): DomainState { return { sessions: this.sessions, machines: this.machines, handoffs: this.handoffs, turns: this.turns, turn: this.turn, starts: this.starts, holds: this.holds, handoff: this.handoff, transcriptMeta }; }
}
import { object, text, boolean, number, optional, array } from '../domain/validate';
function parseModelMachine(value: unknown) { const v = object(value); return { id: text(v.id), name: text(v.name), up: boolean(v.up), last: optional(v.last, number) }; }
