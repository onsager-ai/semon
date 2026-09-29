// Writes a synthetic ~/.claude, ~/.claude.json, ~/.codex and /proc whose session model reproduces the sample mockup's data: its sessions,
// handoffs, turns, answers, busy intervals and transcripts, as Claude Code and Codex log lines. The sample's data is made up,
// and every line here is made from it: the texts are read from the committed mockup, never from real logs.
//
//   node fixture.mjs OUT_DIR [--extras]   writes OUT_DIR/{.claude.json,claude,codex,proc,work,roles} and prints the `now` to pin
//   node fixture.mjs OUT_DIR --second     writes a second machine's home (`desktop`) for views across machines
//
// --extras writes the sample plus what only the served viewer has to handle, for the check scripts (never for the pixel
// comparison or the gap check): what the mockup's markdown check page (mkmd.js) added (one message in harbor using every
// markdown construct, and an answered two-part question from ledger); a harbor step whose command is longer than its
// summary; a yielded Codex command with a poll that sends input; a Codex call with no exit status in deps; a `backlog` lane of 460 entries (paging); and a lane whose key, name,
// branch, messages, tools, relay, question, answer and subagent all carry an injection payload (XSS).
//
// What the model's rules can't reproduce is listed in gaps.json, and checked by gaps.mjs.
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));

// The sample's data block, evaluated as the mockup evaluates it.
export function sample() {
  const html = fs.readFileSync(path.join(here, "reference/semon-sample.html"), "utf8");
  const start = html.indexOf("  const T = (h, m) => h * 60 + m;");
  const end = html.indexOf("  // ====================================================================================\n  const $ = ");
  if (start < 0 || end < start) throw new Error("the sample's data block moved");
  const values = vm.runInNewContext("(() => {\n" + html.slice(start, end) + "\nreturn { NOW, MACHINE, MACHINE_UP, SESS, H, TX, API_PRICE };\n})()");
  const historyStart = html.indexOf("  const ANALYTICS_HISTORY = {");
  const historyEnd = html.indexOf("  function analyticsSessions()", historyStart);
  if (historyStart < 0 || historyEnd < historyStart) throw new Error("the sample's analytics history moved");
  const history = vm.runInNewContext("(() => { const T = (h, m) => h * 60 + m;\n" + html.slice(historyStart, historyEnd) + "\nreturn { ANALYTICS_HISTORY, ANALYTICS_WAIT_SAMPLES };\n})()");
  return { ...values, ...history };
}

// Sample minutes since midnight map to 2026-09-28 UTC, the date pinned by the approved mockup's allowance sample.
export const BASE = Date.UTC(2026, 8, 28);
export const ms = (minutes, seconds = 0, millis = 0) => BASE + minutes * 60000 + seconds * 1000 + millis;
const T = (h, m) => h * 60 + m;
const iso = (t) => new Date(t).toISOString();

// mkmd.js's message, verbatim.
export const MARKDOWN = ["## A heading with `code`", "Plain line with **bold**, *italic*, ~~gone~~, a [safe link](https://example.com/a), a [bad link](javascript:alert(1)), and https://example.org/x.", "Literal tags stay text: <script>alert(1)</script> and <b>not bold</b>.", "", "1. First step", "2. Second step", "   - nested bullet", "   - another", "3. Third step", "", "* star bullet", "", "> A quoted line", "> and another", "", "---", "", "```rust", "fn a_very_long_function_name_that_needs_horizontal_scrolling(argument_one: u32, argument_two: u32) -> u32 { argument_one + argument_two }", "```", "", "| Col A | Col B |", "|---|---|", "| `x` | cut off…", "", "```", "an unclosed fence at the end…"].join("\n");

// The injection payload, and one without a slash for what becomes a file name (a session key).
export const XSS = '<img src=x onerror="window.__xss=1"><script>window.__xss=2</script><iframe src="javascript:window.__xss=3"></iframe>';
export const XSS_KEY = "<img src=x onerror=window.__xss=4>";

export function write(out, { extras = false } = {}) {
  const { NOW, SESS, H, TX, ANALYTICS_HISTORY } = sample();
  const HB = Object.fromEntries(H.map((h) => [h.id, h]));
  const brief = (id) => HB[id].brief;
  const rm = (p) => fs.rmSync(p, { recursive: true, force: true });
  for (const d of ["claude", "codex", "proc", "work", "roles"]) rm(path.join(out, d));
  const put = (rel, text) => { const p = path.join(out, rel); fs.mkdirSync(path.dirname(p), { recursive: true }); fs.writeFileSync(p, text); return p; };
  // A line given as { raw } is written as is: an unreadable line the log lost.
  const jsonl = (rel, lines) => {
    // Codex reads session_meta from the first line; history may predate the active session by a week.
    const meta = lines.filter(([, value]) => value.type === "session_meta"), rest = lines.filter(([, value]) => value.type !== "session_meta").sort((a, b) => a[0] - b[0]);
    return put(rel, [...meta, ...rest].map(([, v]) => (v.raw != null ? v.raw : JSON.stringify(v))).join("\n") + "\n");
  };
  // Repositories are directories with a .git; roles work outside any repository.
  const repo = (name) => { fs.mkdirSync(path.join(out, "work", name, ".git"), { recursive: true }); return path.join(out, "work", name); };
  const role = (name) => { fs.mkdirSync(path.join(out, "roles", name), { recursive: true }); return path.join(out, "roles", name); };
  const historyAt = ([daysAgo, minute]) => BASE - daysAgo * 86400000 + minute * 60000;
  put("proc/sys/kernel/hostname", "laptop\n");
  let locks = "";

  // ---- Claude Code ------------------------------------------------------------------------------------------------
  let seq = 0;
  const models = { "opus-5.5": "claude-opus-5-5", "sonnet-5": "claude-sonnet-5", "haiku-4.5": "claude-haiku-4-5" };
  function claude(sid, { cwd, branch, model, tokens, agent }) {
    const lines = [];
    const base = (t, type, extra) => { lines.push([t, { parentUuid: null, isSidechain: !!agent, type, timestamp: iso(t), sessionId: agent ? agent.parent : sid, ...(agent ? { agentId: sid } : {}), cwd, ...(branch ? { gitBranch: branch } : {}), version: "2.1.0", uuid: "u-" + sid + "-" + seq++, ...extra }]); };
    const said = (t, content, extra = {}) => base(t, "assistant", { message: { id: "msg-" + sid + "-" + seq, model: models[model], role: "assistant", type: "message", content }, ...extra });
    const s = {
      lines,
      filler: (t) => base(t, "system", { subtype: "turn_duration" }),
      ask: (t, text) => base(t, "user", { origin: { kind: "human" }, message: { role: "user", content: text } }),
      prompt: (t, text) => base(t, "user", { message: { role: "user", content: text } }),
      peer: (t, from, name, msg, body) => base(t, "user", { origin: { kind: "peer", from: "uds:/run/user/1000/cc-socks/" + from + ".sock", name, msg_id: msg, body }, message: { role: "user", content: body } }),
      text: (t, text) => said(t, [{ type: "text", text }]),
      think: (t, text) => said(t, [{ type: "thinking", thinking: text, signature: "sig" }]),
      tool: (t, id, name, input) => said(t, [{ type: "tool_use", id, name, input }]),
      result: (t, id, content, { error = false, extra = {} } = {}) => base(t, "user", { message: { role: "user", content: [{ type: "tool_result", tool_use_id: id, content, ...(error ? { is_error: true } : {}) }] }, ...extra }),
      title: (t, name) => lines.push([t, { type: "custom-title", customTitle: name, sessionId: sid }]),
      // Busy intervals: a line every four minutes from a to b, so the stamps cluster exactly into [a, b].
      busy: (a, b) => { for (let t = a; t < b; t += 4 * 60000) s.filler(t); s.filler(b); },
    };
    // Token use: one usage record, as the model counts tokens by message id.
    s.tokens = (t) => {
      const target = Object.values(SESS[sid]?.tokensByModel ?? {})[0] ?? { input: tokens[0] * 1e6, cacheWrite: 0, cacheRead: tokens[1] * 1e6, output: tokens[2] * 1e6 };
      const cacheWrite = target.cacheWrite ?? target.cache_write ?? 0, cacheWrite1h = target.cacheWrite1h ?? target.cache_write_1h ?? 0;
      lines.push([t, { type: "assistant", timestamp: iso(t), sessionId: agent ? agent.parent : sid, cwd, uuid: "u-" + sid + "-usage", message: { id: "msg-" + sid + "-usage", model: models[model], role: "assistant", content: [], usage: { input_tokens: target.input, cache_creation_input_tokens: cacheWrite, cache_creation: { ephemeral_5m_input_tokens: cacheWrite - cacheWrite1h, ephemeral_1h_input_tokens: cacheWrite1h }, cache_read_input_tokens: target.cacheRead ?? target.cache_read ?? 0, output_tokens: target.output } } }]);
    };
    s.save = () => {
      if (agent) {
        const dir = "claude/projects/" + agent.slug + "/" + agent.parent + "/subagents/agent-" + sid;
        jsonl(dir + ".jsonl", lines);
        put(dir + ".meta.json", JSON.stringify({ agentType: "general-purpose", description: agent.description ?? SESS[sid].name, toolUseId: agent.tool }));
      } else jsonl("claude/projects/" + slug(cwd) + "/" + sid + ".jsonl", lines);
    };
    const history = ANALYTICS_HISTORY[sid];
    if (history?.started) s.title(historyAt(history.started), SESS[sid]?.name ?? sid);
    for (const [daysAgo, a, b] of history?.busy ?? []) s.busy(historyAt([daysAgo, a]), historyAt([daysAgo, b]));
    return s;
  }
  const slug = (cwd) => cwd.replace(/[^A-Za-z0-9]/g, "-");
  // A live Claude process: its pid file, and a /proc stat whose start time matches.
  let pid = 100;
  function live(sid, status, name = SESS[sid].name) {
    pid++;
    const fields = Array(20).fill("0"); fields[19] = "777";
    put("proc/" + pid + "/stat", pid + " (claude) " + fields.join(" ") + "\n");
    put("claude/sessions/" + pid + ".json", JSON.stringify({ pid, sessionId: sid, cwd: "/", startedAt: BASE, procStart: 777, status, name, kind: "interactive" }));
  }
  const tx = (sid) => TX[sid];

  // principal: a role. Sentinel's relay at 07:12, your morning sweep at 07:30, a relay to Advisor, and a result for you.
  {
    const c = claude("principal", { cwd: role("principal"), model: "opus-5.5", tokens: SESS.principal.tokens });
    c.title(ms(T(7, 12)), "Principal");
    c.peer(ms(T(7, 12)), 102, "Sentinel", "m-h9", brief("h9"));
    c.text(ms(T(7, 14)), tx("principal")[1].text);
    c.ask(ms(T(7, 30)), brief("h8"));
    // The sample includes two masked Claude thinking blocks before this turn's first tool call.
    c.think(ms(T(7, 30), 12), "");
    c.think(ms(T(7, 30), 20), "");
    c.think(ms(T(7, 30), 40), "");
    c.tool(ms(T(7, 31)), "toolu-p1", "Bash", { command: tx("principal")[6].arg, description: "List open PRs" });
    c.result(ms(T(7, 31), 2, 300), "toolu-p1", tx("principal")[6].out);
    c.busy(ms(T(7, 30)), ms(T(7, 48)));
    c.busy(ms(T(9, 8)), ms(T(9, 12)));
    c.tool(ms(T(9, 10)), "toolu-h10", "SendMessage", { to: "Advisor", message: brief("h10") });
    c.result(ms(T(9, 10), 0, 200), "toolu-h10", "Message sent to Advisor", { extra: { toolUseResult: { success: true, msg_id: "m-h10" } } });
    c.busy(ms(T(9, 46)), ms(T(9, 50)));
    c.text(ms(T(9, 48)), tx("principal")[8].text);
    c.text(ms(T(9, 50)), brief("h11"));
    c.tokens(ms(T(9, 50)));
    c.save(); live("principal", "idle");
  }
  // sentinel: a role that watches CI and relays new failures to Principal; it is running a check now.
  {
    const c = claude("sentinel", { cwd: role("sentinel"), model: "sonnet-5", tokens: SESS.sentinel.tokens });
    const t = tx("sentinel");
    c.title(ms(T(6, 0)), "Sentinel");
    c.prompt(ms(T(6, 0)), t[0].text);
    c.text(ms(T(6, 1)), t[1].text);
    for (const [a, b] of SESS.sentinel.busy) c.busy(ms(a), ms(b));
    c.tool(ms(T(7, 10)), "toolu-s1", "Bash", { command: t[2].arg });
    c.result(ms(T(7, 10), 1, 200), "toolu-s1", t[2].out);
    c.tool(ms(T(7, 12)), "toolu-h9", "SendMessage", { to: "Principal", message: brief("h9") });
    c.result(ms(T(7, 12), 0, 200), "toolu-h9", "Message sent to Principal", { extra: { toolUseResult: { success: true, msg_id: "m-h9" } } });
    c.tool(ms(T(12, 39), 59), "toolu-s2", "Bash", { command: t[4].arg });
    c.tokens(ms(T(12, 38)));
    c.save(); live("sentinel", "busy");
  }
  // advisor: a role that answered Principal's relay. Its process has exited.
  {
    const c = claude("advisor", { cwd: role("advisor"), model: "opus-5.5", tokens: SESS.advisor.tokens });
    c.title(ms(T(9, 10)), "Advisor");
    c.peer(ms(T(9, 10)), 101, "Principal", "m-h10", brief("h10"));
    c.think(ms(T(9, 10), 30), tx("advisor")[1].text);
    c.busy(ms(T(9, 10)), ms(T(9, 45)));
    c.text(ms(T(9, 45)), tx("advisor")[2].text);
    c.tokens(ms(T(9, 44)));
    c.save();
  }
  // harbor: your ask, a fix, a Codex run and a reviewer subagent; running the suite again now.
  const harborCwd = repo("harbor");
  {
    const c = claude("harbor", { cwd: harborCwd, branch: "main", model: "opus-5.5", tokens: SESS.harbor.tokens });
    const t = tx("harbor");
    c.title(ms(T(8, 5)), "harbor");
    c.busy(ms(T(8, 5)), ms(T(8, 12)));
    c.ask(ms(T(11, 40)), brief("h1"));
    c.think(ms(T(11, 40), 9), t[1].text);
    c.text(ms(T(11, 40), 10), t[2].text);
    c.tool(ms(T(11, 40), 11), "toolu-b1", "Bash", { command: t[3].arg });
    c.result(ms(T(11, 40), 49, 200), "toolu-b1", t[3].out, { error: true });
    c.tool(ms(T(11, 41)), "toolu-r1", "Read", { file_path: path.join(harborCwd, t[4].arg) });
    c.result(ms(T(11, 41), 0, 100), "toolu-r1", t[4].out);
    const d = t[5].diff;
    c.tool(ms(T(11, 42)), "toolu-e1", "Edit", { file_path: path.join(harborCwd, t[5].arg), old_string: d.filter(([k]) => k !== "add").map(([, x]) => x.slice(1)).join("\n"), new_string: d.filter(([k]) => k !== "del").map(([, x]) => x.slice(1)).join("\n") });
    c.result(ms(T(11, 42)), "toolu-e1", "The file " + t[5].arg + " has been updated.");
    c.tool(ms(T(11, 43)), "toolu-b2", "Bash", { command: t[6].arg });
    c.result(ms(T(11, 43), 41, 600), "toolu-b2", t[6].out);
    if (extras) {
      // A command longer than its one-line summary: the step shows its Command.
      c.tool(ms(T(11, 44)), "toolu-b4", "Bash", { command: "cd " + harborCwd + " && RUST_BACKTRACE=1 RUST_LOG=harbor_sync=debug cargo test -p harbor-sync --features offline-queue,flush-metrics -- --test-threads=4 --nocapture queue::retry_backoff queue::retry_limit" });
      // Its output is longer than the served preview (1536 bytes), so View all reads the rest from the server.
      c.result(ms(T(11, 44), 12, 300), "toolu-b4", Array.from({ length: 80 }, (_, i) => "test queue::retry_case_" + String(i).padStart(3, "0") + " ... ok").join("\n") + "\n\ntest result: ok. 80 passed; 0 failed");
    }
    c.busy(ms(T(11, 40)), ms(T(12, 40)));
    c.tool(ms(T(11, 52)), "toolu-h2", "Bash", { command: "codex exec --full-auto < /tmp/handoff-offline-sync.md", run_in_background: true });
    c.tool(ms(T(12, 31)), "toolu-h3", "Agent", { description: SESS["h-review"].name, subagent_type: "general-purpose", prompt: brief("h3"), run_in_background: true });
    c.result(ms(T(12, 31), 0, 500), "toolu-h3", "Async agent launched successfully.", { extra: { toolUseResult: { status: "async_launched", agentId: "h-review" } } });
    c.tool(ms(T(12, 33)), "toolu-h20", "Agent", { description: SESS["h-failed"].name, subagent_type: "general-purpose", prompt: brief("h20"), run_in_background: true });
    c.result(ms(T(12, 36)), "toolu-h20", "Failed before producing a reproducer: the review sandbox could not read the test fixture.", { error: true });
    if (extras) c.text(ms(T(12, 31), 30), MARKDOWN);
    c.text(ms(T(12, 36), 1), t[10].text);
    c.tool(ms(T(12, 39), 18), "toolu-b3", "Bash", { command: SESS.harbor.activity[1] });
    c.tokens(ms(T(12, 32)));
    c.save(); live("harbor", "busy");
    // h-review: the reviewer subagent, reading the diff as it lands.
    const r = claude("h-review", { cwd: harborCwd, model: "sonnet-5", tokens: SESS["h-review"].tokens, agent: { parent: "harbor", slug: slug(harborCwd), tool: "toolu-h3" } });
    const rt = tx("h-review");
    r.prompt(ms(T(12, 31)), brief("h3"));
    r.tool(ms(T(12, 32)), "toolu-v1", "Bash", { command: rt[1].arg });
    r.result(ms(T(12, 32), 0, 200), "toolu-v1", rt[1].out);
    r.busy(ms(T(12, 31)), ms(T(12, 40)));
    r.tool(ms(T(12, 39), 58), "toolu-v2", "Read", { file_path: path.join(harborCwd, SESS["h-review"].activity[1]) });
    r.tool(ms(T(12, 36)), "toolu-h19", "Agent", { description: SESS["h-review-codex"].name, subagent_type: "general-purpose", prompt: brief("h19"), run_in_background: true });
    r.result(ms(T(12, 36), 10), "toolu-h19", "Async agent launched successfully.", { extra: { toolUseResult: { status: "async_launched", agentId: "h-review-codex" } } });
    r.tokens(ms(T(12, 33)));
    r.save();
    const f = claude("h-failed", { cwd: harborCwd, model: "sonnet-5", tokens: SESS["h-failed"].tokens, agent: { parent: "harbor", slug: slug(harborCwd), tool: "toolu-h20" } });
    f.prompt(ms(T(12, 33)), brief("h20"));
    const failedTool = tx("h-failed")[1];
    f.tool(ms(T(12, 35), 57, 200), "toolu-f1", "Read", { file_path: path.join(harborCwd, failedTool.arg) });
    f.result(ms(T(12, 36)), "toolu-f1", failedTool.out, { error: true });
    f.tokens(ms(T(12, 36)));
    f.save();
  }
  // quill: your ask, a planning subagent, a Codex run that failed two layout tests, and a question for you.
  const quillCwd = repo("quill");
  {
    const c = claude("quill", { cwd: quillCwd, branch: "feat/pdf-export", model: "opus-5.5", tokens: SESS.quill.tokens });
    const t = tx("quill");
    c.title(ms(T(10, 10)), "quill");
    c.ask(ms(T(10, 10)), brief("h4"));
    c.busy(ms(T(10, 10)), ms(T(10, 41)));
    c.tool(ms(T(10, 40)), "toolu-h5", "Agent", { description: SESS["q-plan"].name, subagent_type: "Plan", prompt: brief("h5") });
    c.result(ms(T(11, 15)), "toolu-h5", HB.h5.result, { extra: { toolUseResult: { status: "completed", agentId: "q-plan" } } });
    c.text(ms(T(11, 16)), t[2].text);
    c.tool(ms(T(11, 20)), "toolu-h6", "Bash", { command: "codex exec --full-auto < /tmp/handoff-pdf-export.md" });
    c.busy(ms(T(11, 15)), ms(T(11, 21)));
    c.result(ms(T(12, 5)), "toolu-h6", "PR #219 pushed; codex exited.");
    c.tool(ms(T(12, 6)), "toolu-q1", "Bash", { command: t[4].arg });
    c.result(ms(T(12, 6), 1, 100), "toolu-q1", t[4].out, { error: true });
    c.busy(ms(T(12, 5)), ms(T(12, 32)));
    c.tool(ms(T(12, 32)), "toolu-h7", "AskUserQuestion", { questions: [{ question: brief("h7"), header: "Next step", multiSelect: false, options: [{ label: "Send it back to Codex", description: "With the two failing cases" }, { label: "Look at RTL first", description: "Before anything else changes" }] }] });
    c.tokens(ms(T(12, 30)));
    c.save(); live("quill", "idle");
    const p = claude("q-plan", { cwd: quillCwd, model: "opus-5.5", tokens: SESS["q-plan"].tokens, agent: { parent: "quill", slug: slug(quillCwd), tool: "toolu-h5" } });
    p.prompt(ms(T(10, 40)), brief("h5"));
    p.busy(ms(T(10, 40)), ms(T(11, 15)));
    p.text(ms(T(11, 14)), tx("q-plan")[1].text);
    p.tokens(ms(T(11, 14)));
    p.save();
  }
  // atlas: your ask; a plan applied, and an apply that never reported back; a message for you.
  {
    const cwd = repo("atlas");
    const c = claude("atlas", { cwd, branch: "infra/staging-net", model: "opus-5.5", tokens: SESS.atlas.tokens });
    const t = tx("atlas");
    c.title(ms(T(10, 30)), "atlas");
    c.ask(ms(T(10, 30)), brief("h12"));
    c.tool(ms(T(10, 31)), "toolu-a1", "Bash", { command: t[1].arg });
    c.result(ms(T(10, 31), 48), "toolu-a1", t[1].out);
    c.tool(ms(T(10, 33)), "toolu-a2", "Bash", { command: t[2].arg });
    c.busy(ms(T(10, 30)), ms(T(11, 2)));
    c.busy(ms(T(11, 7)), ms(T(11, 8)));
    c.text(ms(T(11, 7), 30), t[4].text);
    c.text(ms(T(11, 8)), brief("h14"));
    c.tokens(ms(T(11, 8)));
    c.save(); live("atlas", "idle");
  }
  // ledger: your ask, a subagent that found the call sites, a fix and its tests. Its process has exited.
  {
    const cwd = repo("ledger");
    const c = claude("ledger", { cwd, branch: "fix/rounding", model: "opus-5.5", tokens: SESS.ledger.tokens });
    const t = tx("ledger");
    c.title(ms(T(7, 10)), "ledger");
    c.ask(ms(T(7, 10)), brief("h16"));
    c.busy(ms(T(7, 10)), ms(T(7, 23)));
    c.tool(ms(T(7, 22)), "toolu-h17", "Agent", { description: SESS["l-sub"].name, subagent_type: "Explore", prompt: brief("h17") });
    c.result(ms(T(7, 31)), "toolu-h17", HB.h17.result, { extra: { toolUseResult: { status: "completed", agentId: "l-sub" } } });
    c.busy(ms(T(7, 31)), ms(T(8, 2)));
    const d = t[2].diff;
    c.tool(ms(T(7, 40)), "toolu-e2", "Edit", { file_path: path.join(cwd, t[2].arg), old_string: d.filter(([k]) => k !== "add").map(([, x]) => x.slice(1)).join("\n"), new_string: d.filter(([k]) => k !== "del").map(([, x]) => x.slice(1)).join("\n") });
    c.result(ms(T(7, 40)), "toolu-e2", "The file " + t[2].arg + " has been updated.");
    c.tool(ms(T(7, 50)), "toolu-l1", "Bash", { command: t[3].arg });
    c.result(ms(T(7, 50), 6, 800), "toolu-l1", t[3].out);
    if (extras) {
      c.tool(ms(T(8, 1)), "toolu-lq", "AskUserQuestion", { questions: [{ question: "Ship the rounding fix?", header: "Ship", multiSelect: false, options: [{ label: "Ship it (Recommended)", description: "Merge now" }, { label: "Wait", description: "Hold for review" }] }, { question: "How should it land?", header: "Land", multiSelect: false, options: [{ label: "Squash the commits", description: "One commit" }, { label: "Keep the history", description: "As is" }] }] });
      c.result(ms(T(8, 1), 30), "toolu-lq", "User has answered your questions.", { extra: { toolUseResult: { answers: { "Ship the rounding fix?": "Ship it (Recommended)", "How should it land?": "Squash the commits" } } } });
    }
    c.text(ms(T(8, 2)), brief("h18"));
    c.tokens(ms(T(8, 2)));
    c.save();
    const l = claude("l-sub", { cwd, model: "haiku-4.5", tokens: SESS["l-sub"].tokens, agent: { parent: "ledger", slug: slug(cwd), tool: "toolu-h17" } });
    const lt = tx("l-sub");
    l.prompt(ms(T(7, 22)), brief("h17"));
    l.tool(ms(T(7, 24)), "toolu-g1", "Grep", { pattern: lt[1].arg });
    l.result(ms(T(7, 24), 0, 200), "toolu-g1", lt[1].out);
    l.busy(ms(T(7, 22)), ms(T(7, 31)));
    l.text(ms(T(7, 30)), lt[2].text);
    l.tokens(ms(T(7, 30)));
    l.save();
  }

  // ---- Codex ---------------------------------------------------------------------------------------------------------
  function codex(id, start, { cwd, branch, tokens, nickname, agentPath }) {
    const lines = [];
    const at = (t, type, payload) => lines.push([t, { timestamp: iso(t), type, payload }]);
    lines.push([start - 1, { timestamp: iso(start), type: "session_meta", payload: { id, timestamp: iso(start), cwd, originator: "codex_exec", cli_version: "0.120.0", source: "exec", ...(nickname ? { agent_nickname: nickname } : {}), ...(agentPath ? { agent_path: agentPath } : {}), git: { branch } } }]);
    at(start, "turn_context", { cwd, model: "gpt-6-luna", approval_policy: "never" });
    // No code: an output with no exit status, as some Codex calls record.
    const out = (text, code) => (code == null ? text : JSON.stringify({ output: text, metadata: { exit_code: code, duration_seconds: 0 } }));
    const c = {
      user: (t, text) => at(t, "response_item", { type: "message", role: "user", content: [{ type: "input_text", text }] }),
      harness: (t, label) => at(t, "response_item", { type: "message", role: "user", content: [{ type: "input_text", text: "<" + label + ">\n- github\n</" + label + ">" }] }),
      text: (t, text) => at(t, "response_item", { type: "message", role: "assistant", content: [{ type: "output_text", text }] }),
      think: (t, text) => at(t, "response_item", { type: "reasoning", summary: [{ type: "summary_text", text }], encrypted_content: null }),
      shell: (t, callId, command) => at(t, "response_item", { type: "function_call", name: "shell", arguments: JSON.stringify({ command: command.split(" ") }), call_id: callId }),
      call: (t, callId, name, input) => at(t, "response_item", { type: "function_call", name, arguments: JSON.stringify(input), call_id: callId }),
      code: (t, callId, script) => at(t, "response_item", { type: "custom_tool_call", status: "completed", call_id: callId, name: "exec", input: script }),
      scriptResult: (t, callId, result) => at(t, "response_item", { type: "custom_tool_call_output", call_id: callId, output: [
        { type: "input_text", text: "Script completed\nWall time 1.0s\nOutput:\n" },
        { type: "input_text", text: JSON.stringify({ chunk_id: "fixture", wall_time_seconds: 1, original_token_count: 1, ...result }) },
      ] }),
      item: (t, item) => at(t, "event_msg", { type: "item_completed", item }),
      patch: (t, callId, patch) => at(t, "response_item", { type: "custom_tool_call", status: "completed", call_id: callId, name: "apply_patch", input: patch }),
      output: (t, callId, text, code, custom) => at(t, "response_item", { type: custom ? "custom_tool_call_output" : "function_call_output", call_id: callId, output: out(text, code) }),
      busy: (a, b) => { for (let t = a; t < b; t += 4 * 60000) at(t, "turn_context", { cwd, model: "gpt-6-luna", approval_policy: "never" }); at(b, "turn_context", { cwd, model: "gpt-6-luna", approval_policy: "never" }); },
      handback: (t, text) => at(t, "response_item", { type: "agent_message", author: agentPath, content: [{ type: "output_text", text }] }),
      failed: (t) => at(t, "event_msg", { type: "task_complete", error: { message: "the layout suite failed" } }),
      tokens: (t) => {
        const modelsUsed = Object.values(SESS[id]?.tokensByModel ?? {});
        const use = modelsUsed.reduce((sum, value) => ({ input: sum.input + value.input, cache_read: sum.cache_read + (value.cacheRead ?? value.cache_read ?? 0), output: sum.output + value.output }), { input: 0, cache_read: 0, output: 0 });
        const input = modelsUsed.length ? use.input : Math.round(tokens[0] * 1e6), cached = modelsUsed.length ? use.cache_read : Math.round(tokens[1] * 1e6), output = modelsUsed.length ? use.output : Math.round(tokens[2] * 1e6);
        const limits = SESS[id]?.rate_limits;
        const window = (source) => ({ window_minutes: source.minutes, used_percent: source.used_percent, resets_in_seconds: (Date.parse(source.resets_at) - Date.parse(limits.recorded_at)) / 1000 });
        at(t, "event_msg", { type: "token_count", info: { total_token_usage: { input_tokens: input + cached, cached_input_tokens: cached, output_tokens: output, reasoning_output_tokens: 0, total_tokens: input + cached + output }, ...(limits ? { rate_limits: { primary: window({ minutes: 300, ...limits.five_hour }), secondary: window({ minutes: 10080, ...limits.weekly }) } } : {}) } });
      },
      save: () => jsonl("codex/sessions/2026/09/28/rollout-2026-09-28T" + iso(start).slice(11, 19).replace(/:/g, "-") + "-" + id + ".jsonl", lines),
    };
    const history = ANALYTICS_HISTORY[id];
    if (history?.started) at(historyAt(history.started), "turn_context", { cwd, model: "gpt-6-luna", approval_policy: "never" });
    for (const [daysAgo, a, b] of history?.busy ?? []) c.busy(historyAt([daysAgo, a]), historyAt([daysAgo, b]));
    return c;
  }
  // A running Codex run holds its thread's writer lock: a lock file, and a /proc/locks line naming its inode.
  function locked(id) {
    const file = put("codex/thread-writer-locks/" + id + ".lock", "");
    const st = fs.statSync(file, { bigint: true }), dev = st.dev;
    const major = ((dev >> 8n) & 0xfffn) | ((dev >> 32n) & ~0xfffn), minor = (dev & 0xffn) | ((dev >> 12n) & ~0xffn);
    locks += (locks.split("\n").length) + ": FLOCK  ADVISORY  WRITE " + (200 + locks.split("\n").length) + " " + major.toString(16).padStart(2, "0") + ":" + minor.toString(16).padStart(2, "0") + ":" + st.ino + " 0 EOF\n";
  }
  const patchOf = (file, diff) => "*** Begin Patch\n*** Update File: " + file + "\n@@\n" + diff.map(([, x]) => x).join("\n") + "\n*** End Patch\n";
  // h-codex: harbor's Codex run, implementing the offline flush; a patch is being applied now.
  {
    const t = tx("h-codex"), c = codex("h-codex", ms(T(11, 52)), { cwd: harborCwd, branch: "feat/offline-sync", tokens: SESS["h-codex"].tokens });
    c.user(ms(T(11, 52)), "Semon-Parent: claude:harbor:toolu-h2\n" + brief("h2"));
    c.harness(ms(T(11, 52), 1), t[1].label);
    c.think(ms(T(11, 52), 19), t[2].text);
    c.patch(ms(T(11, 53)), "call-x1", patchOf(t[3].arg, t[3].diff));
    c.output(ms(T(11, 53)), "call-x1", "Success. Updated the following files:\nM " + t[3].arg, 0, true);
    c.shell(ms(T(11, 55)), "call-x2", t[4].arg);
    c.output(ms(T(11, 55), 19, 400), "call-x2", t[4].out, 101);
    c.busy(ms(T(11, 52)), ms(T(12, 40)));
    c.patch(ms(T(12, 39), 54), "call-x3", "*** Begin Patch\n*** Update File: " + t[5].arg + "\n@@\n-        self.pending.pop_front()\n+        self.pending.pop_front().filter(|batch| !batch.acked)\n*** End Patch\n");
    c.tokens(ms(T(12, 30)));
    c.save(); locked("h-codex");
  }
  // h-review-codex: the review's Codex run, launched by its child Claude session.
  {
    const rt = tx("h-review-codex"), c = codex("h-review-codex", ms(T(12, 36)), { cwd: harborCwd, branch: "feat/offline-sync", tokens: SESS["h-review-codex"].tokens, nickname: SESS["h-review-codex"].name });
    c.user(ms(T(12, 36)), "Semon-Parent: claude:h-review:toolu-h19\n" + brief("h19"));
    c.call(ms(T(12, 37)), "call-h19", "Read", { file_path: path.join(harborCwd, rt[1].arg) });
    c.output(ms(T(12, 37), 0, 300), "call-h19", rt[1].out, 0);
    c.think(ms(T(12, 39), 30), "");
    c.busy(ms(T(12, 36)), ms(T(12, 40)));
    c.tokens(ms(T(12, 38)));
    c.save(); locked("h-review-codex");
  }
  // q-codex: quill's Codex run; the layout suite failed and it handed back.
  {
    const t = tx("q-codex"), c = codex("q-codex", ms(T(11, 20)), { cwd: quillCwd, branch: "feat/pdf-export", tokens: SESS["q-codex"].tokens, agentPath: "workers/pdf-export" });
    c.user(ms(T(11, 20)), "Semon-Parent: claude:quill:toolu-h6\n" + brief("h6"));
    c.harness(ms(T(11, 20), 1), t[1].label);
    c.shell(ms(T(11, 22)), "call-y1", t[2].arg);
    c.output(ms(T(11, 22), 14, 200), "call-y1", t[2].out, 0);
    c.shell(ms(T(11, 58)), "call-y2", t[3].arg);
    c.output(ms(T(12, 1), 12), "call-y2", t[3].out, 1);
    c.busy(ms(T(11, 20)), ms(T(12, 5)));
    c.text(ms(T(12, 2)), t[4].text);
    c.handback(ms(T(12, 5)), HB.h6.result);
    c.failed(ms(T(12, 5)));
    c.tokens(ms(T(12, 4)));
    c.save();
  }
  // deps: a Codex run you started directly, auditing meridian's dependencies.
  {
    const t = tx("deps"), c = codex("deps", ms(T(12, 20)), { cwd: repo("meridian"), branch: "chore/deps-audit", tokens: SESS.deps.tokens, nickname: SESS.deps.name });
    c.user(ms(T(12, 20)), brief("h15"));
    c.harness(ms(T(12, 20), 1), t[1].label);
    c.busy(ms(T(12, 20)), ms(T(12, 40)));
    if (extras) {
      c.shell(ms(T(12, 30)), "call-z0", "cargo metadata --format-version 1 --no-deps");
      c.output(ms(T(12, 30), 2, 100), "call-z0", "{\"packages\":[{\"name\":\"meridian\"}]}", null);
    }
    c.shell(ms(T(12, 39), 49), "call-z1", t[2].arg);
    c.tokens(ms(T(12, 38)));
    c.save(); locked("deps");
  }
  if (extras) {
    // result-card: an idle session that replied to your own message.
    {
      const c = claude("result-card", { cwd: role("result-card"), model: "opus-5.5", tokens: [0, 0, 0] });
      c.title(ms(T(8, 48)), "Result card");
      c.ask(ms(T(8, 49)), "Give one compact answer");
      c.text(ms(T(8, 50)), "Unique result text for the transcript check.");
      c.save(); live("result-card", "idle", "Result card");
    }
    // code-mode: one script whose two commands and file change are indexed as three transcript steps.
    {
      const cwd = repo("meridian"), c = codex("code-mode", ms(T(8, 0)), { cwd, branch: "feat/code-mode", tokens: [0, 0, 0] });
      const script = "const r = await Promise.allSettled([\ntools.exec_command({cmd:\"git status\",workdir:\"/w\"}),\ntools.exec_command({cmd:\"sed -n '1,9p' a.rs\",workdir:\"/w\"}),\n]);";
      c.user(ms(T(8, 0)), "Run the indexed operations");
      c.code(ms(T(8, 1)), "script-call", script);
      c.item(ms(T(8, 2)), { type: "CommandExecution", id: "command-1", command: ["/bin/zsh", "-lc", "git status"], cwd: "file://" + cwd, exit_code: 0, duration: { secs: 1, nanos: 200000000 }, aggregated_output: "## feature/code-mode\n" });
      c.item(ms(T(8, 3)), { type: "CommandExecution", id: "command-2", command: ["/bin/zsh", "-lc", "sed -n '1,9p' a.rs"], cwd: "file://" + path.join(cwd, "src"), exit_code: 0, duration: { secs: 0, nanos: 400000000 }, aggregated_output: "a\n" });
      c.item(ms(T(8, 4)), { type: "FileChange", id: "change-1", changes: { [path.join(cwd, "src/code-mode.rs")]: { type: "update", unified_diff: "@@ -1 +1 @@\n-old\n+new\n", move_path: null } } });
      c.output(ms(T(8, 5)), "script-call", "Script completed", null, true);
      c.text(ms(T(8, 6)), "Three operations completed.");
      c.save();
    }
    // yielded-ui: a long command outlives its yield, then a poll sends input before completion.
    {
      const cwd = repo("meridian"), start = ms(T(8, 10)), c = codex("yielded-ui", start, { cwd, branch: "feat/yielded-ui", tokens: [0, 0, 0] });
      const pid = 4242, command = "printf " + "x".repeat(220);
      const poll = (chars) => "const r = await tools.write_stdin({session_id:" + pid + ",chars:" + JSON.stringify(chars) + ",yield_time_ms:1000});\ntext(JSON.stringify(r));\n";
      c.user(start, "Run a long command, then send input to its poll.");
      c.code(start + 1000, "yield-start", "const r = await tools.exec_command({cmd:" + JSON.stringify(command) + ",workdir:\"" + cwd + "\",yield_time_ms:1000});\ntext(JSON.stringify(r));\n");
      c.scriptResult(start + 2000, "yield-start", { session_id: pid, output: "started\n" });
      c.code(start + 3000, "yield-input", poll("y\n"));
      c.scriptResult(start + 4000, "yield-input", { session_id: pid, output: "received\n" });
      c.code(start + 5000, "yield-finish", poll(""));
      c.item(start + 5500, { type: "CommandExecution", id: "yield-ui-exec", process_id: String(pid), command: ["/bin/zsh", "-lc", command], cwd: "file://" + cwd, status: "completed", exit_code: 0, duration: { secs: 5, nanos: 0 }, aggregated_output: "started\nreceived\nfinished\n" });
      c.scriptResult(start + 6000, "yield-finish", { output: "finished\n", exit_code: 0 });
      c.text(start + 7000, "The long command accepted input and finished.");
      c.save();
    }
    // codex-cut: outputs Codex cut before the model saw them. A plain call's whole output is short, cut in its middle under
    // Codex's warning header behind unified exec's frame; a code-mode command's output is
    // long, cut by the 1 MiB collection cap.
    {
      const cwd = repo("meridian"), start = ms(T(8, 20)), c = codex("codex-cut", start, { cwd, branch: "feat/codex-cut", tokens: [0, 0, 0] });
      const cases = (from, to) => Array.from({ length: to - from }, (_, i) => "test suite::case_" + String(from + i).padStart(3, "0") + " ... ok");
      const log = (from, to) => Array.from({ length: to - from }, (_, i) => "[" + String(from + i).padStart(4, "0") + "] compiled unit " + (from + i)).join("\n");
      c.user(start, "Run the whole suite, then show me the build log.");
      c.call(start + 1000, "cut-plain", "exec_command", { cmd: "cargo test --workspace" });
      // Unified exec's form (codex-rs core/src/tools/context.rs): a frame, `Output:`, then Codex's warning header and cut.
      c.output(start + 2000, "cut-plain", "Chunk ID: 3f9a1c\nWall time: 4.2100 seconds\nProcess exited with code 0\nOriginal token count: 24000\nOutput:\nWarning: truncated output (original token count: 24000)\nTotal output lines: 900\n\n" + cases(0, 4).join("\n") + "\n…19500 tokens truncated…" + cases(896, 900).join("\n") + "\ntest result: ok. 900 passed; 0 failed\n", null);
      c.text(start + 3000, "The suite passed. Now the build log.");
      c.code(start + 4000, "cut-script", "const r = await tools.exec_command({cmd:\"cat build.log\",workdir:\"" + cwd + "\"});\ntext(r.output);");
      c.item(start + 4500, { type: "CommandExecution", id: "cut-item", command: ["/bin/zsh", "-lc", "cat build.log"], cwd: "file://" + cwd, exit_code: 0, duration: { secs: 2, nanos: 0 }, aggregated_output: log(0, 40) + "\n... 1048576 bytes omitted ...\n" + log(9960, 10000) + "\n" });
      c.output(start + 5000, "cut-script", "Script completed", null, true);
      c.text(start + 6000, "The build log ran to the end.");
      c.save();
    }
    // backlog: a long transcript, two and a half pages of tool calls in ten turns, with one unreadable line.
    const b = claude("backlog", { cwd: role("backlog"), model: "sonnet-5", tokens: [0.01, 0.2, 0.01] });
    b.title(ms(T(5, 0)), "backlog");
    for (let i = 0; i < 460; i++) {
      const t = ms(T(5, 0), i * 4);
      // One unreadable line before batch 6: the transcript shows where the log lost entries.
      if (i === 230) b.lines.push([t - 1, { raw: '{"type":"assistant","message":{"content":[{"type":"text","text":"cut of' }]);
      if (i % 46 === 0) b.ask(t, "Triage batch " + (i / 46 + 1) + " of the issue backlog.");
      b.tool(t + 1000, "toolu-k" + i, "Grep", { pattern: "issue-" + i });
      b.result(t + 1200, "toolu-k" + i, "issues/" + i + ".md");
    }
    b.text(ms(T(5, 31)), "Backlog triaged: 460 issues read.");
    b.save();
    // The payload lane: live and working, so its name, activity and every handoff reach the model.
    const x = claude(XSS_KEY, { cwd: role("xss"), branch: XSS, model: "opus-5.5", tokens: [0.01, 0.1, 0.01] });
    x.ask(ms(T(12, 30)), XSS);
    x.prompt(ms(T(12, 30), 5), "note " + XSS);
    x.think(ms(T(12, 30), 10), XSS);
    x.text(ms(T(12, 31)), XSS);
    x.tool(ms(T(12, 31), 10), "toolu-x1", "mcp__probe__" + XSS, { a: XSS, b: XSS });
    x.result(ms(T(12, 31), 11), "toolu-x1", XSS);
    x.tool(ms(T(12, 32)), "toolu-x2", "Edit", { file_path: role("xss") + "/a.md", old_string: XSS, new_string: XSS + "!" });
    x.result(ms(T(12, 32), 1), "toolu-x2", "updated");
    x.tool(ms(T(12, 33)), "toolu-x3", "SendMessage", { to: XSS, message: XSS });
    x.result(ms(T(12, 33), 1), "toolu-x3", "no such peer", { extra: { toolUseResult: { success: false } } });
    x.tool(ms(T(12, 34)), "toolu-x4", "AskUserQuestion", { questions: [{ question: XSS, header: "x", multiSelect: false, options: [{ label: XSS, description: XSS }, { label: "b", description: "b" }] }] });
    x.result(ms(T(12, 34), 30), "toolu-x4", "answered", { extra: { toolUseResult: { answers: { [XSS]: XSS } } } });
    x.tool(ms(T(12, 35)), "toolu-x5", "Agent", { description: XSS, subagent_type: "general-purpose", prompt: XSS });
    x.result(ms(T(12, 36)), "toolu-x5", XSS, { extra: { toolUseResult: { status: "completed", agentId: "xss-sub" } } });
    x.busy(ms(T(12, 30)), ms(T(12, 40)));
    x.tool(ms(T(12, 39), 30), "toolu-x6", "Bash", { command: XSS });
    x.save(); live(XSS_KEY, "busy", XSS);
    const xs = claude("xss-sub", { cwd: role("xss"), model: "haiku-4.5", tokens: [0, 0.01, 0], agent: { parent: XSS_KEY, slug: slug(role("xss")), tool: "toolu-x5", description: XSS } });
    xs.prompt(ms(T(12, 35)), XSS);
    xs.text(ms(T(12, 35), 30), XSS);
    xs.save();
  }
  put(".claude.json", JSON.stringify({ projects: { "/fixture/principal": { lastSessionId: "principal", lastStartTime: ms(T(9, 45)), lastCost: 40, lastDuration: 300000, lastAPIDuration: 260000, lastToolDuration: 40000, lastLinesAdded: 12, lastLinesRemoved: 3, lastModelUsage: {} } } }));
  put("proc/locks", locks);
  return ms(NOW);
}

// A second machine, for views across machines: `desktop`, with a session that finished this morning and one working
// now, in its own repository. Its ids can't collide with the sample's. Prints the same `now` as the sample.
export function writeSecond(out) {
  const { NOW } = sample();
  const rm = (p) => fs.rmSync(p, { recursive: true, force: true });
  for (const d of ["claude", "codex", "proc", "work"]) rm(path.join(out, d));
  const put = (rel, text) => { const p = path.join(out, rel); fs.mkdirSync(path.dirname(p), { recursive: true }); fs.writeFileSync(p, text); return p; };
  put("proc/sys/kernel/hostname", "desktop\n");
  put("proc/locks", "");
  const cwd = path.join(out, "work", "foundry");
  fs.mkdirSync(path.join(cwd, ".git"), { recursive: true });
  const project = "claude/projects/" + cwd.replace(/[^A-Za-z0-9]/g, "-") + "/";
  let seq = 0;
  const line = (sid, t, type, extra) => ({ parentUuid: null, isSidechain: false, type, timestamp: iso(t), sessionId: sid, cwd, gitBranch: "main", version: "2.1.0", uuid: "u-second-" + seq++, ...extra });
  const said = (sid, t, content) => line(sid, t, "assistant", { message: { id: "msg-second-" + seq, model: "claude-sonnet-5", role: "assistant", type: "message", content } });
  const save = (sid, lines) => put(project + sid + ".jsonl", lines.map((l) => JSON.stringify(l)).join("\n") + "\n");
  const review = "5e1c0a2e-0d1c-4b7e-9a51-2f6c1d0e7a01", build = "5e1c0a2e-0d1c-4b7e-9a51-2f6c1d0e7a02";
  save(review, [
    { type: "custom-title", customTitle: "Nightly review", sessionId: review },
    line(review, ms(T(8, 10)), "user", { origin: { kind: "human" }, message: { role: "user", content: "Review last night's foundry run" } }),
    said(review, ms(T(8, 14)), [{ type: "text", text: "Reviewed: two flaky tests, both in the cache layer. Nothing else failed." }]),
  ]);
  save(build, [
    { type: "custom-title", customTitle: "Release build", sessionId: build },
    line(build, ms(T(12, 20)), "user", { origin: { kind: "human" }, message: { role: "user", content: "Cut the 0.9 release build" } }),
    said(build, ms(T(12, 22)), [{ type: "tool_use", id: "toolu-second-1", name: "Bash", input: { command: "cargo build --release" } }]),
  ]);
  const pid = 4100, fields = Array(20).fill("0"); fields[19] = "777";
  put("proc/" + pid + "/stat", pid + " (claude) " + fields.join(" ") + "\n");
  put("claude/sessions/" + pid + ".json", JSON.stringify({ pid, sessionId: build, cwd, startedAt: ms(T(12, 20)), procStart: 777, status: "busy", name: "Release build", kind: "interactive" }));
  return ms(NOW);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const out = process.argv[2];
  if (!out) { console.error("usage: node fixture.mjs OUT_DIR [--extras | --second]"); process.exit(2); }
  if (process.argv.includes("--second")) console.log(writeSecond(path.resolve(out)));
  else console.log(write(path.resolve(out), { extras: process.argv.includes("--extras") }));
}
