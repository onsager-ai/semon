import fs from 'node:fs';
import path from 'node:path';
const iso = (t) => new Date(t).toISOString();
export function logs(dir) {
  const projects = path.join(dir, 'claude/projects'),
    listing = () => fs.readdirSync(projects, { recursive: true }); // (read afresh: sessions come and go)
  const file = (sid) => {
    const f = listing().find(
      (p) => path.basename(p) === sid + '.jsonl' && !p.includes('subagents'),
    );
    if (!f) throw new Error('no ' + sid + '.jsonl');
    return path.join(projects, f);
  };
  const cwdOf = (f) =>
    fs
      .readFileSync(f, 'utf8')
      .split('\n')
      .filter(Boolean)
      .map((l) => {
        try {
          return JSON.parse(l).cwd;
        } catch {
          return null;
        }
      })
      .find(Boolean);
  const said = (sid, cwd, t, text) =>
    JSON.stringify({
      parentUuid: null,
      isSidechain: false,
      type: 'assistant',
      timestamp: iso(t),
      sessionId: sid,
      cwd,
      version: '2.1.0',
      uuid: 'u-order-' + sid + '-' + t,
      message: {
        id: 'msg-order-' + sid + '-' + t,
        model: 'claude-opus-5-5',
        role: 'assistant',
        type: 'message',
        content: [{ type: 'text', text }],
      },
    }) + '\n';
  let n = 0;
  return {
    has: (sid) =>
      listing().some((p) => path.basename(p) === sid + '.jsonl' && !p.includes('subagents')),
    // The session says something at `t`: it is the most recent one from then on.
    bump: (sid, t) => {
      const f = file(sid);
      fs.appendFileSync(f, said(sid, cwdOf(f), t, 'Order check: more work ' + n++));
    },
    // The session starts a subagent (its Agent call and the subagent's own file), as live.mjs does.
    spawn: (sid, child, t) => {
      const f = file(sid),
        cwd = cwdOf(f),
        sub = path.join(path.dirname(f), sid, 'subagents', 'agent-' + child + '.jsonl');
      const call = 'toolu-order-' + child;
      fs.appendFileSync(
        f,
        JSON.stringify({
          parentUuid: null,
          isSidechain: false,
          type: 'assistant',
          timestamp: iso(t),
          sessionId: sid,
          cwd,
          version: '2.1.0',
          uuid: 'u-order-' + call,
          message: {
            id: 'msg-order-' + call,
            model: 'claude-opus-5-5',
            role: 'assistant',
            type: 'message',
            content: [
              {
                type: 'tool_use',
                id: call,
                name: 'Agent',
                input: {
                  description: 'Order child',
                  subagent_type: 'general-purpose',
                  prompt: 'Order check: a child run',
                  run_in_background: true,
                },
              },
            ],
          },
        }) +
          '\n' +
          JSON.stringify({
            parentUuid: null,
            isSidechain: false,
            type: 'user',
            timestamp: iso(t + 500),
            sessionId: sid,
            cwd,
            version: '2.1.0',
            uuid: 'u-order-r-' + call,
            message: {
              role: 'user',
              content: [
                {
                  type: 'tool_result',
                  tool_use_id: call,
                  content: 'Async agent launched successfully.',
                },
              ],
            },
            toolUseResult: { status: 'async_launched', agentId: child },
          }) +
          '\n',
      );
      fs.mkdirSync(path.dirname(sub), { recursive: true });
      fs.writeFileSync(
        sub.replace(/\.jsonl$/, '.meta.json'),
        JSON.stringify({
          agentType: 'general-purpose',
          description: 'Order child',
          toolUseId: call,
        }),
      );
      const line = (at2, type, message) =>
        JSON.stringify({
          parentUuid: null,
          isSidechain: true,
          type,
          timestamp: iso(at2),
          sessionId: sid,
          agentId: child,
          cwd,
          version: '2.1.0',
          uuid: 'u-order-' + child + '-' + at2,
          message,
        }) + '\n';
      fs.writeFileSync(
        sub,
        line(t + 1000, 'user', { role: 'user', content: 'Order check: a child run' }) +
          line(t + 20000, 'assistant', {
            id: 'msg-order-' + child,
            model: 'claude-opus-5-5',
            role: 'assistant',
            type: 'message',
            content: [{ type: 'text', text: 'Order check: the child says hello.' }],
          }),
      );
    },
    // A new session in a repo of its own.
    create: (sid, t) => {
      const cwd = path.join(dir, 'work', 'order-' + sid),
        f = path.join(projects, cwd.replace(/[^A-Za-z0-9]/g, '-'), sid + '.jsonl');
      fs.mkdirSync(path.join(cwd, '.git'), { recursive: true });
      fs.mkdirSync(path.dirname(f), { recursive: true });
      fs.writeFileSync(
        f,
        JSON.stringify({
          parentUuid: null,
          isSidechain: false,
          type: 'user',
          timestamp: iso(t),
          sessionId: sid,
          cwd,
          gitBranch: 'main',
          version: '2.1.0',
          uuid: 'u-order-' + sid,
          message: { role: 'user', content: 'Order check: a new session ' + sid },
        }) +
          '\n' +
          said(sid, cwd, t + 1000, 'Order check: hello from ' + sid),
      );
    },
  };
}
