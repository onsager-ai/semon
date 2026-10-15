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
// Polls, with a deadline, until all four active native Query tools over the received copy equal the local ones, byte for byte,
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
// The existing native-page fixture census also checks every transcript, recorded
// turn/handoff body and supported field/output expansion on the exact served pair.
if (process.argv.includes('--native-pages')) {
  const { data } = await import('./lib.mjs');
  // Entry identity intentionally includes the admitted source key (local versus
  // the received machine). Map only that proven carrier identity after validating
  // each side; native provenance, every content field and unknown fields compare
  // unchanged. No byte-proof/source-reference difference is normalized away.
  const addressedEntry = (sid, entry) => {
    if (
      typeof entry.entry_id !== 'string' ||
      !/^[0-9a-f]{64}$/.test(entry.entry_id) ||
      !Number.isSafeInteger(entry.slot) ||
      entry.slot < 0
    )
      throw Error('pushed-parity native entry has an invalid identity/address');
    return { ...entry, entry_id: JSON.stringify([sid, entry.slot]) };
  };
  const nativeCorpus = async (base, token) => {
    const fixture = await data({ base, token });
    const landing = await fetch(base + '/?t=' + token);
    const cookie = landing.headers
      .getSetCookie()
      .map((value) => value.split(';')[0])
      .join('; ');
    const get = async (query, endpoint = '/api/session-entry') => {
      const response = await fetch(base + endpoint + '?' + query, { headers: { cookie } });
      if (!response.ok) throw Error('pushed-parity native expansion: ' + response.status);
      return response.json();
    };
    const expansions = {},
      transcripts = {};
    for (const [sid, entries] of Object.entries(fixture.TX)) {
      const identities = new Set();
      transcripts[sid] = entries.map((entry, ordinal) => {
        const addressed = addressedEntry(sid, entry);
        if (entry.slot !== ordinal || identities.has(entry.entry_id))
          throw Error('pushed-parity native entry identity/slot repeated or out of order');
        identities.add(entry.entry_id);
        return addressed;
      });
      for (const entry of entries) {
        if (!entry.field && !entry.cut) continue;
        const machine = fixture.SOURCES[fixture.SESS[sid].machine];
        const query = new URLSearchParams({
          machine,
          sid: fixture.KEYS[sid],
          scope: 'retained_history',
          after: String(entry.slot),
          limit: '1',
        });
        const selected = await get(query, '/api/session-transcript');
        const saved = selected.entries[0];
        if (
          selected.identity.source_key !== machine ||
          selected.identity.catalog_key !== fixture.KEYS[sid] ||
          selected.range.first !== entry.slot ||
          selected.entries.length !== 1 ||
          saved.entry_id !== entry.entry_id ||
          JSON.stringify(saved.provenance) !== JSON.stringify(entry.provenance)
        )
          throw Error('pushed-parity native expansion changed its selected source');
        query.set('generation', selected.projection.generation);
        const key = JSON.stringify([sid, entry.slot]);
        if (entry.field) {
          if (!Number.isSafeInteger(entry.field.chunks) || entry.field.chunks < 0)
            throw Error(
              'pushed-parity native field has an invalid chunk count (type ' +
                typeof entry.field.chunks +
                ')',
            );
          // Empty native strings have one checkpoint and therefore zero chunks.
          // Their complete body already participates in exact transcript parity;
          // there is no field chunk to request from the expansion endpoint.
          if (entry.field.chunks === 0) {
            // The producer deliberately omits empty Thinking text. Require that
            // exact shape on both observations; other fields must contain ''.
            const omittedThinking = entry.k === 'think' && entry.field.name === 'text';
            const emptyBody = (value) =>
              omittedThinking
                ? value.k === 'think' && !Object.hasOwn(value, 'text')
                : value[entry.field.name] === '';
            const sameDescriptor = JSON.stringify(saved.field) === JSON.stringify(entry.field);
            if (
              !['text', 'out'].includes(entry.field.name) ||
              entry.field.complete !== true ||
              entry.clipped !== false ||
              !emptyBody(entry) ||
              !sameDescriptor ||
              saved.clipped !== false ||
              !emptyBody(saved)
            )
              throw Error(
                'pushed-parity zero-chunk native field is not complete empty content: ' +
                  JSON.stringify({
                    slot: entry.slot,
                    omittedThinking,
                    sameDescriptor,
                    entryComplete: entry.field.complete === true,
                    entryUnclipped: entry.clipped === false,
                    savedUnclipped: saved.clipped === false,
                    entryEmpty: emptyBody(entry),
                    savedEmpty: emptyBody(saved),
                  }),
              );
            expansions[key] = { field: entry.field.name, text: '' };
            continue;
          }
          const anchor = entry.field.provenance ?? entry.provenance;
          let text = '';
          for (let chunk = 0; chunk < entry.field.chunks; chunk++) {
            query.set('field_chunk', String(chunk));
            const field = await get(query);
            const complete = chunk + 1 === entry.field.chunks;
            if (
              field.identity.source_key !== machine ||
              field.identity.catalog_key !== fixture.KEYS[sid] ||
              field.projection.generation !== selected.projection.generation ||
              field.slot !== entry.slot ||
              field.field.chunk !== chunk ||
              field.field.name !== entry.field.name ||
              field.field.complete !== complete ||
              field.field.next !== (complete ? null : chunk + 1) ||
              typeof field.text !== 'string' ||
              JSON.stringify(field.provenance) !== JSON.stringify(anchor)
            )
              throw Error('pushed-parity native field generation/range changed');
            text += field.text;
          }
          expansions[key] = { field: entry.field.name, text };
        } else {
          query.set('part', 'output');
          const page = await get(query);
          if (
            page.identity.source_key !== machine ||
            page.identity.catalog_key !== fixture.KEYS[sid] ||
            page.projection.generation !== selected.projection.generation ||
            page.range.first !== entry.slot ||
            page.entries.length !== 1 ||
            page.entries[0].entry_id !== entry.entry_id ||
            JSON.stringify(page.entries[0].provenance) !== JSON.stringify(entry.provenance)
          )
            throw Error('pushed-parity native output generation/range changed');
          expansions[key] = addressedEntry(sid, page.entries[0]);
        }
      }
    }
    return {
      transcripts,
      transcript_metadata: fixture.TXM,
      handoffs: fixture.H,
      turns: fixture.turns,
      expansions,
    };
  };
  const direct = await nativeCorpus(process.env.SEMON_REF_BASE, process.env.SEMON_REF_TOKEN);
  const copy = await nativeCorpus(process.env.SEMON_BASE, process.env.SEMON_TOKEN);
  const changed = differences(direct, copy);
  if (changed.length) {
    const artifactRoot = path.resolve(process.env.SEMON_UI_OUT ?? 'out');
    fs.mkdirSync(artifactRoot, { recursive: true });
    fs.writeFileSync(artifactRoot + '/pushed-native-local.json', JSON.stringify(direct));
    fs.writeFileSync(artifactRoot + '/pushed-native-copy.json', JSON.stringify(copy));
    throw Error('pushed-parity native-page bodies differ:\n' + changed.join('\n'));
  }
  console.log(
    'pushed-parity: complete native transcript, tool, turn and handoff corpus equals the local server',
  );
  process.exit(0);
}

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
  const disabled = spawnSync(bin, ['query', 'find', 'session', '--all', '--json', ...args], {
    env: { ...process.env, ...env },
    encoding: 'utf8',
    timeout: READ_TIMEOUT_MS,
  });
  if (
    disabled.error ||
    disabled.status !== 1 ||
    disabled.stdout.trim() ||
    !disabled.stderr.includes('capability_unavailable')
  )
    throw Error(
      'pushed-parity stale find did not explicitly refuse before search: ' + disabled.stderr,
    );
  const json = {
    listing,
    sessions,
    paused_find: { error: 'capability_unavailable' },
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
