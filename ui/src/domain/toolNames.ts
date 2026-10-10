const RUN = ['run', 'Ran', 'ran', 'command', 'commands'],
  FIND = ['find', 'Searched for', 'searched', 'time', 'times'];
const TOOLS: Record<string, string[]> = {
  Bash: RUN,
  shell: RUN,
  exec_command: RUN,
  local_shell: RUN,
  write_stdin: ['run', 'Sent input to', 'sent input to', 'time', 'times'],
  Grep: FIND,
  Glob: FIND,
  Read: ['read', 'Read', 'read', 'file', 'files'],
  Edit: ['edit', 'Edited', 'edited', 'file', 'files'],
  MultiEdit: ['edit', 'Edited', 'edited', 'file', 'files'],
  Write: ['edit', 'Wrote', 'wrote', 'file', 'files'],
  apply_patch: ['edit', 'Patched', 'patched', 'file', 'files'],
  NotebookEdit: ['edit', 'Edited', 'edited', 'notebook', 'notebooks'],
  AskUserQuestion: ['q', 'Asked you', 'asked you', 'question', 'questions'],
  ToolSearch: ['find', 'Loaded', 'loaded', 'tool', 'tools'],
  SendMessage: ['out', 'Sent', 'sent', 'message', 'messages'],
  SendUserFile: ['out', 'Sent you', 'sent you', 'file', 'files'],
  Agent: ['out', 'Started', 'started', 'agent', 'agents'],
  Task: ['out', 'Started', 'started', 'agent', 'agents'],
  Monitor: ['now', 'Watched', 'watched', 'process', 'processes'],
  ScheduleWakeup: ['now', 'Scheduled', 'scheduled', 'wake-up', 'wake-ups'],
  TaskStop: ['x', 'Stopped', 'stopped', 'task', 'tasks'],
  Artifact: ['ext', 'Published', 'published', 'page', 'pages'],
  WebFetch: ['ext', 'Fetched', 'fetched', 'page', 'pages'],
  WebSearch: ['search', 'Searched the web for', 'searched the web', 'time', 'times'],
  Skill: ['stack', 'Used skill', 'used', 'skill', 'skills'],
};

const providers: Record<string, string> = {
  github: 'GitHub',
  figma: 'Figma',
  google_drive: 'Google Drive',
  notion: 'Notion',
  slack: 'Slack',
  gmail: 'Gmail',
  railway: 'Railway',
  linear: 'Linear',
  calendar: 'Calendar',
  search_service: 'Search',
};
/** Presentation only. Call identity and native input/output remain untouched. */
export function toolInfo(name: string): string[] {
  if (TOOLS[name]) return TOOLS[name];
  let provider: string | undefined, action: string | undefined;
  const dotted = /^codex_apps\.([^.]+)\.(.+)$/.exec(name);
  const double = /^mcp__(.+?)__(.+)$/.exec(name);
  if (dotted) [, provider, action] = dotted;
  else if (double?.[1] === 'codex_apps') {
    const key = Object.keys(providers)
      .sort((a, b) => b.length - a.length)
      .find((key) => double[2].startsWith(key + '_'));
    if (key) {
      provider = key;
      action = double[2].slice(key.length + 1);
    }
  } else if (double) {
    [, provider, action] = double;
  }
  const known = provider && providers[provider.toLowerCase().replace(/^claude_ai_/, '')];
  if (known && action) {
    const label = known + ' · ' + action.replace(/[_.]+/g, ' ').trim();
    return ['ext', label, label, 'time', 'times'];
  }
  // Unrecognized identities stay exact, including unfamiliar app prefixes.
  return dotted || double
    ? ['ext', name, name, 'time', 'times']
    : ['run', name, name, 'step', 'steps'];
}
