import type { Session } from './types';

/** A local CLI command uses the native identity, never the display/namespace key. */
export function resumeCommand(
  session: Pick<Session, 'harness' | 'id' | 'sessionId'>,
): string | null {
  const id = session.sessionId ?? session.id;
  if (!id.trim() || id.startsWith('-') || /[\0\r\n]/.test(id)) return null;
  // Saved identities are source data. Quote shell metacharacters before offering a command to paste.
  const argument = /^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(id)
    ? id
    : "'" + id.replaceAll("'", "'\"'\"'") + "'";
  switch (session.harness) {
    case 'claude':
      return 'claude --resume ' + argument;
    case 'codex':
      return 'codex resume ' + argument;
    case 'copilot':
      return 'copilot --resume=' + argument;
    default:
      return null;
  }
}
