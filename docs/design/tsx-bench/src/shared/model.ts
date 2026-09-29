// The slice of the viewer's model the samples draw: sessions with a state, a recency and a parent. Shared by every runtime's
// sample, so the bundles differ only in the runtime and the components.

export type State = "work" | "wait" | "idle" | "done" | "err";

export interface Session {
  id: string;
  name: string;
  state: State;
  /** Last activity, epoch ms. */
  last: number;
  parent?: string;
  host: string;
  repo?: string;
  brief?: string;
}

export interface Model {
  version: string;
  now: number;
  sessions: Session[];
}

export const STATE_LABEL: Record<State, string> = { work: "Working", wait: "Needs you", idle: "Idle", done: "Done", err: "Failed" };

export const ago = (now: number, t: number): string => {
  const d = Math.floor((now - t) / 60000);
  return d < 1 ? "now" : d < 60 ? d + "m" : d < 2880 ? Math.floor(d / 60) + "h" : Math.floor(d / 1440) + "d";
};

export const byRecency = (a: Session, b: Session): number => b.last - a.last;

export function childrenOf(sessions: readonly Session[]): Map<string, Session[]> {
  const out = new Map<string, Session[]>();
  for (const s of sessions) if (s.parent) (out.get(s.parent) ?? out.set(s.parent, []).get(s.parent)!).push(s);
  return out;
}

export function descendants(id: string, children: Map<string, Session[]>, out: Session[] = [], seen = new Set([id])): Session[] {
  for (const c of children.get(id) ?? []) {
    if (seen.has(c.id)) continue;
    seen.add(c.id);
    out.push(c);
    descendants(c.id, children, out, seen);
  }
  return out;
}

/** The count pill's tip: how many runs below are in each state, most urgent first. */
export function childParts(all: readonly Session[]): string[] {
  const n = (state: State) => all.filter((x) => x.state === state).length;
  return (["wait", "work", "err", "done"] as const).filter((st) => n(st)).map((st) => n(st) + " " + STATE_LABEL[st].toLowerCase());
}
