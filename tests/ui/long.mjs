// Writes deterministic, made-up long Claude and Codex sessions for the UI baseline.
//
//   node long.mjs OUT_DIR   writes the fixture and prints the pinned `now`.
//
// Claude records use the same JSONL envelope as fixture.mjs's private Claude writer. Subagent
// files and their .meta.json companions follow that writer's projects/<slug>/<parent>/subagents
// layout. `write` creates the standard home/proc skeleton first; its sample logs are then
// replaced by this fixture.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { BASE, ms, write } from './fixture.mjs';

const MINUTE = 60_000;
const TURN_SPAN = 10 * MINUTE;
const TOOLS = ['Bash', 'Read', 'Edit', 'Grep'];
const OUTPUT_SIZES = [2048, 40960, 4096, 32768, 8192, 16384];
const SUBAGENT_TURNS = [5, 16, 27, 38, 49];
const REPOS = ['aster', 'beacon', 'cinder'];

const iso = (time) => new Date(time).toISOString();
const slug = (cwd) => cwd.replace(/[^A-Za-z0-9]/g, '-');

function put(root, rel, text) {
  const file = path.join(root, rel);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text);
  return file;
}

function jsonl(root, rel, records) {
  return put(root, rel, records.map((record) => JSON.stringify(record)).join('\n') + '\n');
}

function repeatedOutput(size, index) {
  const phrase = `Synthetic diagnostic row ${String(index).padStart(4, '0')}: made-up result text for a repeatable UI baseline.\n`;
  return phrase.repeat(Math.ceil(size / phrase.length)).slice(0, size);
}

function claudeWriter({ sid, cwd, branch = 'main', agent = null }) {
  const records = [];
  let seq = 0;
  const base = (time, type, extra = {}) => {
    const record = {
      parentUuid: null,
      isSidechain: !!agent,
      type,
      timestamp: iso(time),
      sessionId: agent ? agent.parent : sid,
      ...(agent ? { agentId: sid } : {}),
      cwd,
      gitBranch: branch,
      version: '2.1.0',
      uuid: `u-${sid}-${seq++}`,
      ...extra,
    };
    records.push(record);
    return record;
  };
  const said = (time, content, extra = {}) =>
    base(time, 'assistant', {
      message: {
        id: `msg-${sid}-${seq}`,
        model: 'claude-opus-5-5',
        role: 'assistant',
        type: 'message',
        content,
      },
      ...extra,
    });
  return {
    records,
    ask(time, text) {
      base(time, 'user', { origin: { kind: 'human' }, message: { role: 'user', content: text } });
    },
    prompt(time, text) {
      base(time, 'user', { message: { role: 'user', content: text } });
    },
    text(time, text) {
      said(time, [{ type: 'text', text }]);
    },
    think(time, text) {
      said(time, [{ type: 'thinking', thinking: text, signature: 'synthetic-signature' }]);
    },
    tool(time, id, name, input, extra = {}) {
      said(time, [{ type: 'tool_use', id, name, input }], extra);
    },
    result(time, id, content, extra = {}) {
      base(time, 'user', {
        message: { role: 'user', content: [{ type: 'tool_result', tool_use_id: id, content }] },
        ...extra,
      });
    },
  };
}

function markdown(turn, sid) {
  return [
    `## Synthetic ${sid} turn ${String(turn + 1).padStart(2, '0')}`,
    'A repeatable assistant note with **bold text**, `inline code`, and a short list:',
    `- inspect record ${turn + 1}`,
    '- keep the generated result deterministic',
    '',
    '```text',
    `made-up summary for ${sid}, turn ${turn + 1}`,
    '```',
  ].join('\n');
}

function makeLongClaude({ sid, cwd, start, turns, toolCounts, isMarathon }) {
  const writer = claudeWriter({ sid, cwd });
  let calls = 0;
  let outputIndex = 0;
  for (let turn = 0; turn < turns; turn++) {
    const turnStart = start + turn * TURN_SPAN;
    let line = 0;
    const time = () => turnStart + line++ * 1000;
    writer.ask(
      time(),
      `Please inspect synthetic batch ${turn + 1} in ${sid} and summarize what changed.`,
    );
    writer.think(time(), `Comparing made-up records for ${sid}, turn ${turn + 1}.`);
    writer.text(time(), markdown(turn, sid));

    for (let call = 0; call < toolCounts[turn]; call++) {
      const toolIndex = calls++;
      const childIndex = isMarathon ? SUBAGENT_TURNS.indexOf(turn) : -1;
      const isSpawn = childIndex >= 0 && call === 0;
      const id = `toolu-${sid}-${String(toolIndex).padStart(4, '0')}`;
      const name = isSpawn ? 'Agent' : TOOLS[toolIndex % TOOLS.length];
      const input = isSpawn
        ? {
            description: `Synthetic reviewer ${childIndex + 1}`,
            subagent_type: 'general-purpose',
            prompt: `Review made-up records for ${sid}, batch ${turn + 1}.`,
          }
        : name === 'Bash'
          ? { command: `printf synthetic-batch-${turn + 1}-${call + 1}` }
          : name === 'Read'
            ? { file_path: `src/synthetic-${turn + 1}.txt` }
            : name === 'Edit'
              ? {
                  file_path: `src/synthetic-${turn + 1}.txt`,
                  old_string: 'before',
                  new_string: 'after',
                }
              : { pattern: `synthetic-pattern-${turn + 1}-${call + 1}` };
      writer.tool(time(), id, name, input);
      const output = repeatedOutput(OUTPUT_SIZES[outputIndex++ % OUTPUT_SIZES.length], toolIndex);
      writer.result(
        time(),
        id,
        output,
        isSpawn
          ? {
              toolUseResult: {
                status: 'async_launched',
                agentId: `marathon-agent-${childIndex + 1}`,
              },
            }
          : {},
      );
    }

    const remaining = 50 - line;
    for (let extra = 0; extra < remaining; extra++) {
      const finalEntry = turn === turns - 1 && extra === remaining - 1;
      const marker = sid === 'marathon' ? 'PERF_MARATHON_LAST' : 'PERF_RELAY_LAST';
      writer.text(
        time(),
        finalEntry
          ? `Final synthetic assistant entry: ${marker}\n\n${markdown(turn, sid)}`
          : markdown(turn, sid),
      );
    }
    if (line !== 50) throw new Error(`${sid} turn ${turn + 1} wrote ${line} records instead of 50`);
  }
  return { records: writer.records, calls };
}

function writeSubagent(root, { id, parent, cwd, start, toolUseId }) {
  const writer = claudeWriter({ sid: id, cwd, agent: { parent } });
  writer.prompt(start, `Review a made-up section of the marathon transcript as ${id}.`);
  for (let i = 0; i < 33; i++) {
    const at = start + (i + 1) * 1000;
    const callId = `toolu-${id}-${String(i + 1).padStart(3, '0')}`;
    const name = TOOLS[i % TOOLS.length];
    const input =
      name === 'Bash'
        ? { command: `printf synthetic-review-${i + 1}` }
        : name === 'Read'
          ? { file_path: `src/review-${i + 1}.txt` }
          : name === 'Edit'
            ? { file_path: `src/review-${i + 1}.txt`, old_string: 'draft', new_string: 'reviewed' }
            : { pattern: `review-pattern-${i + 1}` };
    writer.tool(at, callId, name, input);
    writer.result(at + 100, callId, `Synthetic review output ${i + 1}: no real source was read.`);
    writer.text(
      at + 200,
      `Reviewed made-up section ${i + 1}; the deterministic sample is consistent.`,
    );
  }
  const dir = `claude/projects/${slug(cwd)}/${parent}/subagents/agent-${id}`;
  jsonl(root, `${dir}.jsonl`, writer.records);
  put(
    root,
    `${dir}.meta.json`,
    JSON.stringify({
      agentType: 'general-purpose',
      description: `Synthetic reviewer ${id.slice(-1)}`,
      toolUseId,
    }),
  );
  return writer.records.length;
}

function writeShortClaude(root, { sid, cwd, branch, start, index }) {
  const writer = claudeWriter({ sid, cwd, branch });
  const name = TOOLS[index % TOOLS.length];
  const input =
    name === 'Bash'
      ? { command: `printf short-session-${index + 1}` }
      : name === 'Read'
        ? { file_path: `src/short-${index + 1}.txt` }
        : name === 'Edit'
          ? { file_path: `src/short-${index + 1}.txt`, old_string: 'draft', new_string: 'reviewed' }
          : { pattern: `short-pattern-${index + 1}` };
  writer.ask(start, `Review the made-up change set ${index + 1}.`);
  writer.tool(start + 1000, `toolu-${sid}`, name, input);
  writer.result(start + 1500, `toolu-${sid}`, 'Synthetic result: the short sample completed.');
  writer.text(start + 2000, `## Short summary ${index + 1}\n\nThe made-up change is **complete**.`);
  const file = `claude/projects/${slug(cwd)}/${sid}.jsonl`;
  jsonl(root, file, [
    {
      type: 'custom-title',
      customTitle: `Short session ${String(index + 1).padStart(2, '0')}`,
      sessionId: sid,
    },
    ...writer.records,
  ]);
}

function writeShortCodex(root, { sid, cwd, branch, start, index }) {
  const records = [
    {
      timestamp: iso(start),
      type: 'session_meta',
      payload: {
        id: sid,
        timestamp: iso(start),
        cwd,
        originator: 'codex_exec',
        cli_version: '0.120.0',
        source: 'exec',
        git: { branch },
      },
    },
    {
      timestamp: iso(start),
      type: 'response_item',
      payload: {
        type: 'message',
        role: 'user',
        content: [{ type: 'input_text', text: `Inspect made-up change set ${index + 1}.` }],
      },
    },
    {
      timestamp: iso(start + 1000),
      type: 'response_item',
      payload: {
        type: 'function_call',
        name: 'shell',
        arguments: JSON.stringify({ command: [`printf`, `short-session-${index + 1}`] }),
        call_id: `call-${sid}`,
      },
    },
    {
      timestamp: iso(start + 1200),
      type: 'response_item',
      payload: {
        type: 'function_call_output',
        call_id: `call-${sid}`,
        output: 'Synthetic result: no real source was read.',
      },
    },
    {
      timestamp: iso(start + 2000),
      type: 'response_item',
      payload: {
        type: 'message',
        role: 'assistant',
        content: [
          {
            type: 'output_text',
            text: `## Short Codex summary ${index + 1}\n\nThe made-up review is complete.`,
          },
        ],
      },
    },
  ];
  const day = iso(BASE).slice(0, 10).replace(/-/g, '/');
  const file = `codex/sessions/${day}/rollout-${iso(start).slice(11, 19).replace(/:/g, '-')}-${sid}.jsonl`;
  jsonl(root, file, records);
}

export function writeLong(out) {
  fs.mkdirSync(out, { recursive: true });
  const now = write(out);
  for (const dir of ['claude', 'codex', 'proc', 'work', 'roles']) {
    fs.rmSync(path.join(out, dir), { recursive: true, force: true });
  }
  fs.rmSync(path.join(out, 'index.json'), { force: true });

  put(out, 'proc/sys/kernel/hostname', 'synthetic-ui\n');
  put(out, 'proc/locks', '');
  for (const name of REPOS) fs.mkdirSync(path.join(out, 'work', name, '.git'), { recursive: true });

  const cwd = Object.fromEntries(REPOS.map((name) => [name, path.join(out, 'work', name)]));
  const marathonStart = ms(160); // 02:40 UTC; the last of its 60 turns ends before the pinned 12:40 clock.
  const relayStart = ms(360); // 06:00 UTC.
  const marathonTools = Array.from({ length: 60 }, (_, turn) => (turn < 45 ? 17 : 16));
  const relayTools = Array.from({ length: 30 }, () => 10);
  const marathon = makeLongClaude({
    sid: 'marathon',
    cwd: cwd.aster,
    start: marathonStart,
    turns: 60,
    toolCounts: marathonTools,
    isMarathon: true,
  });
  const relay = makeLongClaude({
    sid: 'relay',
    cwd: cwd.beacon,
    start: relayStart,
    turns: 30,
    toolCounts: relayTools,
    isMarathon: false,
  });
  if (marathon.records.length !== 3000 || marathon.calls !== 1005)
    throw new Error('marathon fixture counts drifted');
  if (relay.records.length !== 1500 || relay.calls !== 300)
    throw new Error('relay fixture counts drifted');
  if (new Date(Date.parse(marathon.records.at(-1).timestamp)).getTime() > now)
    throw new Error('marathon runs past the pinned clock');

  const marathonFile = jsonl(
    out,
    `claude/projects/${slug(cwd.aster)}/marathon.jsonl`,
    marathon.records,
  );
  jsonl(out, `claude/projects/${slug(cwd.beacon)}/relay.jsonl`, relay.records);

  let subagentLines = 0;
  for (let i = 0; i < SUBAGENT_TURNS.length; i++) {
    const turn = SUBAGENT_TURNS[i];
    const id = `marathon-agent-${i + 1}`;
    const toolUseId = `toolu-marathon-${String(marathonTools.slice(0, turn).reduce((n, x) => n + x, 0)).padStart(4, '0')}`;
    subagentLines += writeSubagent(out, {
      id,
      parent: 'marathon',
      cwd: cwd.aster,
      start: marathonStart + turn * TURN_SPAN + 60_000,
      toolUseId,
    });
  }

  for (let i = 0; i < 20; i++) {
    const repo = REPOS[i % REPOS.length];
    const sid = `short-${String(i + 1).padStart(2, '0')}`;
    const start = ms(40 + i * 5);
    const session = { sid, cwd: cwd[repo], branch: `sample/${sid}`, start, index: i };
    if (i % 2 === 0) writeShortClaude(out, session);
    else writeShortCodex(out, session);
  }

  return {
    now,
    marathonFile,
    counts: {
      marathonJsonlEntries: marathon.records.length,
      marathonTurns: 60,
      marathonToolCalls: marathon.calls,
      marathonBashReadEditGrepCalls: marathon.calls - SUBAGENT_TURNS.length,
      marathonSubagentSpawns: SUBAGENT_TURNS.length,
      marathonToolOutputBytesRange: [Math.min(...OUTPUT_SIZES), Math.max(...OUTPUT_SIZES)],
      relayJsonlEntries: relay.records.length,
      relayToolCalls: relay.calls,
      relayTurns: 30,
      subagentFiles: SUBAGENT_TURNS.length,
      subagentJsonlEntriesEach: 100,
      subagentJsonlEntriesTotal: subagentLines,
      shortSessions: 20,
      repositories: REPOS.length,
      shortClaudeSessions: 10,
      shortCodexSessions: 10,
    },
  };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const out = process.argv[2];
  if (!out) {
    console.error('usage: node long.mjs OUT_DIR');
    process.exit(2);
  }
  console.log(writeLong(path.resolve(out)).now);
}
