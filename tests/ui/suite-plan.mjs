// Assign every functional check exactly once. Adding a check without updating this plan fails CI.
export const GROUPS = {
  navigation: ["home", "bar", "turns", "analytics", "runview", "signals", "models", "stepnames", "sidebarfocus"],
  transcript: ["viewer", "md", "full", "check-real", "extras", "attach", "viewer-backlog", "deeplink-live", "codex-thinking"],
  shell: ["shell", "embedsidebar", "names", "hicons", "sidebar", "agentfont"],
  interaction: ["tokens", "scrollbars", "switch", "embed", "tooltip", "details", "select", "filters", "breakdown", "tracebrief"],
  accessibility: ["taps"],
};

export function selectChecks(checks, args = []) {
  const registered = checks.map(([name]) => name);
  const assigned = Object.values(GROUPS).flat();
  if (new Set(registered).size !== registered.length || new Set(assigned).size !== assigned.length
      || assigned.length !== registered.length || assigned.some(name => !registered.includes(name))) {
    throw new Error("CI groups must assign every registered check exactly once; update suite-plan.mjs");
  }
  if (!args.length) return checks;
  if (args.length !== 2 || args[0] !== "--group" || !Object.hasOwn(GROUPS, args[1])) {
    throw new Error("Usage: node checks/run.mjs [--group " + Object.keys(GROUPS).join("|") + "]");
  }
  return checks.filter(([name]) => GROUPS[args[1]].includes(name));
}

export function selectSchemes(schemes, requested) {
  if (!requested) return schemes;
  const names = requested.split(",");
  if (new Set(names).size !== names.length || names.some(name => !schemes.some(([s]) => s === name))) {
    throw new Error("Unknown or duplicate scheme: " + requested);
  }
  return schemes.filter(([name]) => names.includes(name));
}

// Shared components need distinct content shapes, not every session that uses them.
// Keep separate examples for child/error/relay transcripts and attachment/code/output controls.
export const TAP_SESSIONS = {
  sample: ["harbor", "deps", "h-failed", "atlas-ingest", "q-codex", "quill"],
  extras: ["bgcmd", "result-card", "web-search", "attach", "code-mode", "yielded-ui", "codex-cut", "backlog", "fan-out", "swarm", "<img src=x onerror=window.__xss=4>"],
};
export function tapSessions(sessions, fixture, exhaustive = false) {
  if (!Object.hasOwn(TAP_SESSIONS, fixture)) throw new Error("Unknown tap fixture: " + fixture);
  const missing = TAP_SESSIONS[fixture].filter(id => !Object.hasOwn(sessions, id));
  if (missing.length) throw new Error("Missing representative tap sessions: " + missing.join(", "));
  return exhaustive ? Object.keys(sessions) : TAP_SESSIONS[fixture];
}
