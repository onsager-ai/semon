// The pushed copy answers the same native session queries as the machine itself (the mirror protocol's promise, and what
// a second machine's sessions in the viewer rest on). Run by the "Pushed data" steps of .github/workflows/ui.yml, after
// `semon push --watch` has been sending the sample fixture to `semon receive`:
//
//   node pushed-parity.mjs --home DIR --received DIR --scratch DIR
//
//   --home DIR      the fixture machine, with claude/, codex/, proc/ and .claude.json, read directly.
//   --received DIR  the receiver's --dir. Read with `--machines DIR --no-local`, from an empty home of the reader's own.
//   --scratch DIR   the reader's own state: its empty home, its caches. Written to.
//
// Polls, with a deadline, until all five native Query tools over the received copy equal the local ones, byte for byte,
// once the fields in NORMALISED are set aside. While NORMALISED is empty the two stdout texts are compared as they are, so
// number formatting, escapes and key order count too; with fields to set aside, the normalised JSON is compared as text. A push arrives file by file, so the copy is complete a little after the
// first pass; the check is the last state it saw, not a race.
//
// SEMON_BIN is the binary (default target/debug/semon), SEMON_TEST_NOW pins both reads to the fixture's clock, and
// SEMON_UI_OUT (default ./out) gets both query corpora when they differ.
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

// The fields that legitimately differ between a machine read directly and the same machine received. Each is set to null on
// both sides before comparing, and each needs its reason. More than machine identity differing is a bug to report, never
// something to add here.
//
// `at` is a list of keys from the corpus root; "*" stands for every key at that level.
const NORMALISED = [];

const arg = (name) => {
  const i = process.argv.indexOf('--' + name);
  if (i < 0 || !process.argv[i + 1]) {
    console.error('pushed-parity: --' + name + ' is required');
    process.exit(2);
  }
  return path.resolve(process.argv[i + 1]);
};
const fixtureRoot = arg('home'),
  received = arg('received'),
  scratch = arg('scratch');
const bin = path.resolve(process.env.SEMON_BIN ?? '../../target/debug/semon');
const out = path.resolve(process.env.SEMON_UI_OUT ?? 'out');
const DEADLINE_MS = 90_000;
// A read that hangs is killed here, so the deadline above ends the check and not the job's own timeout.
const READ_TIMEOUT_MS = 30_000;

function corpus(args, env) {
  const call = (tool, values = []) => {
    const run = spawnSync(bin, ['query', tool, '--all', '--json', ...args, ...values], {
      env: { ...process.env, ...env },
      encoding: 'utf8',
      maxBuffer: 1 << 28,
      timeout: READ_TIMEOUT_MS,
    });
    if (run.error) throw new Error('semon query ' + tool + ': ' + run.error.message);
    if (run.status !== 0)
      throw new Error('semon query ' + tool + ' exited ' + run.status + ': ' + run.stderr);
    const value = JSON.parse(run.stdout);
    if (value.error) throw new Error('semon query ' + tool + ': ' + JSON.stringify(value.error));
    return value;
  };
  const listing = call('list_sessions', ['--limit', '1000']);
  if (listing.truncated) throw new Error('pushed-parity fixture exceeds the native listing bound');
  const sessions = {};
  for (const row of listing.sessions) {
    const transcript = [];
    let after = 0;
    do {
      const page = call('read_transcript', [row.id, '--after', String(after)]);
      transcript.push(page);
      if (page.to >= page.total) break;
      if (!Number.isSafeInteger(page.to) || page.to <= after || transcript.length >= 10000)
        throw new Error('pushed-parity transcript did not advance for ' + row.id);
      after = page.to;
    } while (true);
    sessions[row.id] = { details: call('get_session', [row.id]), transcript };
  }
  const json = {
    listing,
    sessions,
    search: call('find', ['session']),
    stalls: call('stalls', ['--idle-minutes', '1']),
  };
  return { text: JSON.stringify(json), json };
}

// The machine as it is: its own homes, its own /proc, read directly.
const local = () =>
  corpus(
    [
      '--claude-home',
      fixtureRoot + '/claude',
      '--claude-json',
      fixtureRoot + '/.claude.json',
      '--codex-home',
      fixtureRoot + '/codex',
      '--copilot-home',
      fixtureRoot + '/copilot',
      '--proc-root',
      fixtureRoot + '/proc',
      '--cache',
      scratch + '/local-index.json',
    ],
    { XDG_STATE_HOME: scratch + '/local-state' },
  );

// The machine as a receiver holds it: nothing of the reader's own (an empty home), only DIR/machines/.
fs.mkdirSync(scratch + '/reader/proc', { recursive: true });
const pushed = () => {
  const machines = fs
    .readdirSync(received + '/machines', { withFileTypes: true })
    .filter((item) => item.isDirectory())
    .map((item) => item.name)
    .sort();
  if (machines.length !== 1)
    throw new Error('pushed-parity requires the one pushed fixture machine');
  return corpus(
    [
      '--machine',
      received + '/machines/' + machines[0],
      '--claude-home',
      scratch + '/reader/claude',
      '--claude-json',
      scratch + '/reader/.claude.json',
      '--codex-home',
      scratch + '/reader/codex',
      '--copilot-home',
      scratch + '/reader/copilot',
      '--proc-root',
      scratch + '/reader/proc',
      '--cache',
      scratch + '/reader/index.json',
    ],
    { XDG_STATE_HOME: scratch + '/reader/state' },
  );
};

function normalise(value, at = []) {
  for (const { at: target } of NORMALISED)
    if (target.length === at.length && target.every((key, i) => key === '*' || key === at[i]))
      return null;
  if (Array.isArray(value)) return value.map((item, i) => normalise(item, [...at, i]));
  if (value && typeof value === 'object')
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [key, normalise(item, [...at, key])]),
    );
  return value;
}

// Where two values differ, as paths, so a failing run says what and not just that.
function differences(a, b, at = '', found = []) {
  if (found.length >= 60) return found;
  const short = (v) => {
    const s = JSON.stringify(v);
    return s === undefined ? 'undefined' : s.length > 120 ? s.slice(0, 120) + '…' : s;
  };
  if (
    a &&
    b &&
    typeof a === 'object' &&
    typeof b === 'object' &&
    Array.isArray(a) === Array.isArray(b)
  ) {
    for (const key of new Set([...Object.keys(a), ...Object.keys(b)]))
      differences(a[key], b[key], at + '/' + key, found);
  } else if (JSON.stringify(a) !== JSON.stringify(b))
    found.push(at + '\n    local:  ' + short(a) + '\n    pushed: ' + short(b));
  return found;
}

// What is compared, and what is shown when it differs: the raw text while nothing is set aside, else the normalised JSON.
const view = (read) => {
  const json = normalise(read.json);
  return { json, text: NORMALISED.length ? JSON.stringify(json) : read.text };
};

const want = view(local());
const wantText = want.text;
const sessions = Object.keys(want.json.sessions ?? {}).length;
if (sessions < 5) {
  console.error(
    'pushed-parity: the local query corpus has only ' +
      sessions +
      ' sessions; the fixture is not what this check expects',
  );
  process.exit(1);
}

const started = Date.now();
let last,
  lastText,
  polls = 0;
while (true) {
  polls++;
  try {
    last = view(pushed());
    lastText = last.text;
  } catch (error) {
    last = null;
    lastText = String(error.message ?? error);
  }
  if (last && lastText === wantText) break;
  if (Date.now() - started > DEADLINE_MS) {
    fs.mkdirSync(out, { recursive: true });
    fs.writeFileSync(out + '/pushed-parity-local.json', wantText);
    fs.writeFileSync(out + '/pushed-parity-pushed.json', lastText);
    console.error(
      'pushed-parity: after ' +
        polls +
        ' reads over ' +
        Math.round((Date.now() - started) / 1000) +
        " s the pushed copy still differs from the machine's native query corpus",
    );
    if (!last) console.error(lastText);
    else
      console.error(
        differences(want.json, last.json).join('\n') ||
          '(same values, different bytes: number formatting, escapes or key order)',
      );
    process.exit(1);
  }
  await new Promise((resolve) => setTimeout(resolve, 1000));
}
console.log(
  "pushed-parity: the pushed copy's native query corpus equals the machine's own (" +
    sessions +
    ' sessions, ' +
    wantText.length +
    ' bytes) after ' +
    polls +
    ' read(s), ' +
    NORMALISED.length +
    ' field(s) set aside',
);
