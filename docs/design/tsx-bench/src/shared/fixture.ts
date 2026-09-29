// A small synthetic model and a live step that changes it the way polling does: states flip, recency moves, runs appear.
import type { Model, Session, State } from "./model";

const STATES: State[] = ["work", "wait", "idle", "done", "err"];

export function makeModel(lanes = 12, runsPerLane = 4, now = Date.now()): Model {
  const sessions: Session[] = [];
  for (let i = 0; i < lanes; i++) {
    const id = "s" + i;
    sessions.push({ id, name: "Session " + i, state: STATES[i % STATES.length]!, last: now - i * 97_000, host: "laptop", repo: "semon", brief: "Port the sidebar to components. ".repeat(1 + (i % 5)) });
    for (let j = 0; j < runsPerLane; j++) sessions.push({ id: id + "." + j, name: "Run " + j + " of " + i, state: STATES[(i + j) % STATES.length]!, last: now - i * 97_000 - j * 13_000, parent: id, host: "laptop" });
  }
  return { version: "1", now, sessions };
}

export function step(m: Model, tick: number): Model {
  const now = m.now + 2000;
  const sessions = m.sessions.map((s, i) => (i % 7 === tick % 7 ? { ...s, state: STATES[(STATES.indexOf(s.state) + 1) % STATES.length]!, last: now } : s));
  if (tick % 5 === 0) sessions.push({ id: "n" + tick, name: "New session " + tick, state: "work", last: now, host: "desk" });
  return { version: String(Number(m.version) + 1), now, sessions };
}
