// Errors mode: the session's "N errors" steps through every failed step, on its own server and fixture (the other checks'
// servers stay untouched). The fixture is the sample plus a `faults` lane: 1300 Bash calls in four turns, seven of them
// failed, spread over seven pages of the transcript, so most failures are on pages the session page hasn't loaded.
//
// Asserted, at 390×844 light and 1280×860 light (the dark schemes are drawn and looked at, not stepped through):
//   - choosing Find, then "Failed steps" shows "Error 1 of N", N the count of the meta line's "N failed" (and the model's), and says so in a polite live region;
//   - the first failure, on a page not loaded, is loaded and marked; stepping to one on another unloaded page loads it and
//     centres it between the bar and the bottom of the view (within 2 px);
//   - n and p, the Next button, Enter and Shift+Enter step; focus stays in the bar; each current step is the expected one;
//   - no step expands at any point; the current step is below the bar and above the jump button;
//   - a failed call written while the mode is open makes it "Error k of N+1" with k unchanged;
//   - Escape closes it: the badge is back, a group opened before is open again and nothing else is, the scroll position
//     is back within 2 px, and no step stays marked;
//   - leaving the mode by navigation while it holds a far page, then coming back, opens the session at its end;
//   - phone: nothing overflows sideways and the bar's buttons are at least 44 px;
//   - no page errors.
//
//   SEMON_BIN   the semon binary built with the test-clock feature (default: target/debug/semon)
//   SEMON_UI_OUT where the report and screenshots go (default: ./out)
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { ENV, launch, reporter, overflow } from "../lib.mjs";
import { write } from "../fixture.mjs";
import { serve, model, open, appear } from "./live.mjs";

// The first three failures share the transcript's first page, far from the last page the session opens on; the fourth is
// two pages past them, with a turn's worth of steps after it to centre in.
const CALLS = 1300, TURN = 325, FAILED = [2, 60, 140, 520, 700, 1000, 1290];
const iso = (t) => new Date(t).toISOString();
const sleep = (n) => new Promise((r) => setTimeout(r, n));

// The faults lane, as Claude Code writes it: a title, then per turn your message and its calls, each answered.
function faults(dir, now) {
  const cwd = path.join(dir, "roles", "faults"); fs.mkdirSync(cwd, { recursive: true });
  const file = path.join(dir, "claude/projects", cwd.replace(/[^A-Za-z0-9]/g, "-"), "faults.jsonl"); fs.mkdirSync(path.dirname(file), { recursive: true });
  let seq = 0;
  const base = (t, type, extra) => ({ parentUuid: null, isSidechain: false, type, timestamp: iso(t), sessionId: "faults", cwd, version: "2.1.0", uuid: "u-faults-" + seq++, ...extra });
  const call = (t, i, failed) => [
    base(t, "assistant", { message: { id: "msg-faults-" + i, model: "claude-sonnet-5", role: "assistant", type: "message", content: [{ type: "tool_use", id: "toolu-f" + i, name: "Bash", input: { command: "step " + i } }] } }),
    base(t + 500, "user", { message: { role: "user", content: [{ type: "tool_result", tool_use_id: "toolu-f" + i, content: failed ? "exit 1: step " + i + " failed" : "ok " + i, ...(failed ? { is_error: true } : {}) }] } }),
  ];
  const start = now - 3 * 3600000, lines = [{ type: "custom-title", customTitle: "Faults", sessionId: "faults" }];
  for (let i = 0; i < CALLS; i++) {
    const t = start + i * 4000;
    if (i % TURN === 0) lines.push(base(t, "user", { origin: { kind: "human" }, message: { role: "user", content: "Run batch " + (i / TURN + 1) + "." } }));
    lines.push(...call(t + 1000, i, FAILED.includes(i)));
  }
  lines.push(base(start + CALLS * 4000, "assistant", { message: { id: "msg-faults-end", model: "claude-sonnet-5", role: "assistant", type: "message", content: [{ type: "text", text: "All batches ran." }] } }));
  fs.writeFileSync(file, lines.map((l) => JSON.stringify(l) + "\n").join(""));
  // A late failed call, a minute after the fixture's now (the server's clock is ten minutes past it).
  return { append: (i) => fs.appendFileSync(file, call(now + 60000, i, true).map((l) => JSON.stringify(l) + "\n").join("")) };
}

// Errors mode is entered from Find: its "Failed steps" chip (the meta line's "N failed" is a label, not a control).
// From the bar's own line: the "N failed" label (a control that looks like the others), or on a phone, where the line is not drawn, the ⋯ menu's item.
const enterFromLine = async (page, opts) => { if (opts.size === "phone") { await page.click("#more-btn"); await page.click("dialog.session-menu .menu-errors"); } else await page.click("#topbar .lab-errs"); };
const enter = async (page) => { await page.click("#find-btn"); await page.click('.find-chips .chip[data-filter="failures"]'); };

// What the page shows of the mode and the transcript.
const state = (page) => page.evaluate(() => {
  const norm = (s) => String(s ?? "").replace(/ /g, " ").replace(/\s+/g, " ").trim();
  const sc = window.__sc(), bar = document.querySelector("#topbar").getBoundingClientRect(), cur = document.querySelectorAll("#page .step.err-current");
  const b = cur[0]?.querySelector(":scope > button")?.getBoundingClientRect(), bottom = matchMedia("(max-width: 760px)").matches ? innerHeight : document.querySelector("#main").getBoundingClientRect().bottom;
  const jump = document.querySelector("#jump-bottom"), jr = jump && !jump.hidden ? jump.getBoundingClientRect() : null;
  return {
    label: norm(document.querySelector("#topbar .errnav-count")?.textContent) || null,
    live: norm([...document.querySelectorAll("[aria-live='polite'].sr-only")].map((n) => n.textContent).join("|")),
    current: cur.length, command: cur[0]?.querySelector(".sa")?.textContent ?? null,
    off: b ? Math.round(((b.top + b.bottom) / 2 - (bar.bottom + bottom) / 2) * 10) / 10 : null,
    below: b ? b.top >= bar.bottom - 0.5 : null, clear: b ? !jr || b.bottom <= jr.top + 0.5 : null,
    top: sc.scrollTop, max: sc.scrollHeight - sc.clientHeight,
    expanded: document.querySelectorAll("#page .step > button[aria-expanded='true']").length,
    focus: !!document.activeElement?.closest("#topbar .errnav-bar"), focusId: document.activeElement?.id ?? null, focusLab: document.activeElement?.classList.contains("lab-errs") === true,
    bar: !!document.querySelector("#topbar .meta-line"),
    badge: (() => { const m = [...document.querySelectorAll("#topbar .meta-line .lab")].map((x) => /^(\d+) failed$/.exec(norm(x.textContent))).find(Boolean); return m ? m[1] + " errors" : null; })(),
    groups: [...document.querySelectorAll("#page .tgroup")].map((g) => g.querySelector(":scope > .tsum")?.getAttribute("aria-expanded") === "true"),
  };
});

async function scheme(browser, srv, lane, name, opts, r, full) {
  const R = { name }, tag = name + ": ";
  const page = await open(browser, srv, "/s/claude/faults", opts);
  try {
    const m0 = await model(srv), N = m0.sessions.faults?.errors;
    const list = await (await fetch(srv.base + "/api/tx?sid=faults&errors=1&t=" + srv.token)).json();
    const last = await (await fetch(srv.base + "/api/tx?sid=faults&t=" + srv.token)).json();
    R.n = N; R.slots = list.slots; R.loadedFrom = last.from;
    r.expect(N >= FAILED.length && list.errors === N && list.slots.length === N, tag + "the list and the model disagree: " + JSON.stringify({ N, list }));
    r.expect(list.slots[0] < last.from && list.slots[3] < last.from, tag + "the first failures should be on pages not loaded: " + JSON.stringify({ slots: list.slots, from: last.from }));
    await sleep(2300); // past the opening pin to the end
    // Before: the last group opened, and the view scrolled to the middle of it (away from both ends, so putting it back is
    // a real test).
    // The group holding the last batch's last step, not the last group: earlier schemes' late calls make a small one after it.
    await page.evaluate(() => { const g = [...document.querySelectorAll("#page .tgroup")].find((x) => [...x.querySelectorAll(".step .sa")].some((a) => a.textContent === "step 1299"))?.querySelector(":scope > .tsum"); if (g?.getAttribute("aria-expanded") === "false") g.click(); const sc = window.__sc(); sc.scrollTop = Math.round((sc.scrollHeight - sc.clientHeight) / 2); });
    await sleep(300);
    const before = await state(page);
    R.before = { top: before.top, max: before.max, badge: before.badge, groups: before.groups };
    r.expect(before.top > 200 && before.top < before.max - 200, tag + "the view before is not away from both ends: " + JSON.stringify(R.before));
    // The label may be dropped by the line fitter on a narrow bar; when it is drawn it says the model's count.
    r.expect(before.badge === null || before.badge === N + " errors", tag + "the meta line's failed label reads " + before.badge + ", the model " + N);
    r.expect(before.groups.filter(Boolean).length === 1, tag + "exactly one group should be open before: " + JSON.stringify(before.groups));

    await enterFromLine(page, opts);
    const t0 = Date.now();
    R.entered = await appear(page, t0, (want) => document.querySelector("#topbar .errnav-count")?.textContent === want, "Error 1 of " + N, 6000);
    await page.waitForFunction(() => document.querySelector("#page .step.err-current"), null, { timeout: 6000 }).catch(() => {});
    await sleep(250);
    const first = await state(page); R.first = first;
    r.expect(R.entered != null, tag + "the Failed steps chip did not show Error 1 of " + N + ": " + first.label);
    r.expect(first.current === 1 && first.command === "step " + FAILED[0], tag + "the first failed step (on a page not loaded) is not the one marked: " + JSON.stringify(first));
    r.expect(first.live.includes("Error 1 of " + N), tag + "the live region did not announce it: " + first.live);
    r.expect(first.expanded === 0, tag + "a step expanded on entering");
    r.expect(first.below && first.clear, tag + "the first failure is hidden under the bar or the jump button: " + JSON.stringify(first));
    r.expect(first.focus && first.focusId === "err-next", tag + "focus is not on Next in the bar: " + first.focusId);

    const stepTo = async (how, k) => {
      if (how === "click") await page.click("#err-next"); else await page.keyboard.press(how);
      const ok = await appear(page, Date.now(), (want) => document.querySelector("#topbar .errnav-count")?.textContent === want, "Error " + k + " of " + N, 6000);
      await page.waitForFunction((cmd) => document.querySelector("#page .step.err-current .sa")?.textContent === cmd, "step " + FAILED[k - 1], { timeout: 6000 }).catch(() => {});
      await sleep(250);
      const s = await state(page);
      r.expect(ok != null && s.command === "step " + FAILED[k - 1], tag + how + " did not reach error " + k + ": " + JSON.stringify(s));
      r.expect(s.expanded === 0, tag + how + ": a step expanded");
      r.expect(s.focus, tag + how + ": focus left the bar (" + s.focusId + ")");
      return s;
    };
    R.steps = [];
    R.steps.push(await stepTo("n", 2));
    R.steps.push(await stepTo("p", 1));
    R.steps.push(await stepTo("click", 2));
    R.steps.push(await stepTo("Enter", 3)); // focus is on Next
    R.steps.push(await stepTo("Shift+Enter", 2));
    R.steps.push(await stepTo("n", 3));
    // Error 4 is on a page still not loaded; it loads, and the step lands in the middle of the view.
    const pages = page.txs.filter((u) => /[?&]after=/.test(u)).length;
    const fourth = await stepTo("n", 4); R.steps.push(fourth);
    r.expect(page.txs.filter((u) => /[?&]after=/.test(u)).length > pages, tag + "stepping to the fourth failure loaded no page");
    r.expect(fourth.off !== null && Math.abs(fourth.off) <= 2, tag + "the fourth failure is not centred: " + JSON.stringify(fourth));
    r.expect(fourth.below && fourth.clear, tag + "the fourth failure is under the bar or the jump button");
    await sleep(1400); // the ring fades; the marker stays
    await page.screenshot({ path: path.join(ENV.out, "errnav-" + name + ".png") });
    if (opts.size === "phone") {
      R.overflow = await overflow(page);
      R.targets = await page.evaluate(() => [...document.querySelectorAll("#topbar .errnav-bar button")].map((b) => { const x = b.getBoundingClientRect(); return [b.id, Math.round(x.width), Math.round(x.height)]; }));
      r.expect(R.overflow === 0, tag + "sideways overflow in errors mode: " + R.overflow);
      r.expect(R.targets.length === 3 && R.targets.every(([, w, h]) => w >= 44 && h >= 44), tag + "the bar's buttons are under 44 px: " + JSON.stringify(R.targets));
    }
    if (full) {
      // A failed call lands while the mode is open: N grows, the current one stays.
      const t1 = Date.now(); lane.append("late-" + name);
      R.grew = await appear(page, t1, (want) => document.querySelector("#topbar .errnav-count")?.textContent === want, "Error 4 of " + (N + 1), 10000);
      const grown = await state(page); R.grown = grown;
      r.expect(R.grew != null, tag + "a new failure did not make it Error 4 of " + (N + 1) + ": " + grown.label);
      r.expect(grown.command === "step " + FAILED[3], tag + "the current step moved on the update: " + grown.command);
    }
    await page.keyboard.press("Escape");
    await page.waitForFunction(() => !!document.querySelector("#topbar .meta-line"), null, { timeout: 6000 }).catch(() => {});
    await sleep(400);
    const after = await state(page); R.after = { top: after.top, badge: after.badge, bar: after.bar, groups: after.groups, current: after.current, label: after.label };
    r.expect(after.label === null && after.bar, tag + "Escape did not bring the bar back: " + JSON.stringify(R.after));
    // Back from the mode entered from the line: focus is on the label that entered it, or on ⋯ where the phone's bar draws no line.
    r.expect(opts.size === "phone" ? after.focusId === "more-btn" : after.focusLab === true, tag + "after Escape, focus is on " + after.focusId + " (line label: " + after.focusLab + "), not where the mode was entered from");
    r.expect(Math.abs(after.top - before.top) <= 2, tag + "the scroll position moved by " + (after.top - before.top) + " px after Escape");
    // A group the late calls made at the end is new, and closed.
    r.expect(after.groups.length >= before.groups.length && after.groups.every((x, i) => x === (before.groups[i] ?? false)), tag + "what was open before is not what is open after: " + JSON.stringify({ before: before.groups, after: after.groups }));
    r.expect(after.current === 0 && after.expanded === 0, tag + "a step stayed marked or expanded after Escape");
    // Leaving the mode by navigation while it holds a page far from the end: coming back opens at the end, tailed, not on
    // that middle page.
    await enter(page);
    await appear(page, Date.now(), (want) => document.querySelector("#page .step.err-current .sa")?.textContent === want, "step " + FAILED[0], 6000);
    R.farRange = await page.evaluate(() => [...document.querySelectorAll("#page button.more")].map((b) => b.textContent));
    const nav = (v) => page.evaluate((r) => { history.pushState(r, ""); dispatchEvent(new PopStateEvent("popstate", { state: r })); }, v);
    await nav({ v: "home" }); await page.waitForFunction(() => document.querySelector("#topbar .t")?.textContent === "Home");
    await nav({ v: "session", id: "faults" }); await page.waitForFunction(() => !!document.querySelector("#page section[aria-label='Transcript']") && !!document.querySelector("#topbar .meta-line"));
    await sleep(400);
    R.back = await page.evaluate(() => ({ pagers: [...document.querySelectorAll("#page button.more")].map((b) => b.textContent), mode: !!document.querySelector("#topbar .errnav-bar"), end: [...document.querySelectorAll("#page .msg.assistant")].some((m) => m.textContent.includes("All batches ran.")) }));
    r.expect(R.farRange.includes("Load later"), tag + "stepping to the first failure did not replace the range with a middle page: " + JSON.stringify(R.farRange));
    r.expect(!R.back.mode && !R.back.pagers.includes("Load later") && R.back.end, tag + "after leaving errors mode by navigation, the session did not open at its end: " + JSON.stringify(R.back));
    R.errors = page.errors;
    r.expect(page.errors.length === 0, tag + "page errors: " + page.errors.join(" | "));
  } finally {
    await page.context().close();
  }
  return R;
}

export default async function errnavCheck(browser) {
  const r = reporter("errnav"), out = {};
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "semon-errnav-")), now = write(dir), lane = faults(dir, now);
  const srv = await serve(dir, now + 10 * 60000);
  try {
    for (const [name, opts, full] of [["phone-light", { size: "phone", dark: false }, true], ["desktop-light", { size: "desktop", dark: false }, true], ["phone-dark", { size: "phone", dark: true }, false], ["desktop-dark", { size: "desktop", dark: true }, false]]) {
      try { out[name] = await scheme(browser, srv, lane, name, opts, r, full); }
      catch (e) { r.expect(false, name + ": threw " + (e?.stack ?? e)); }
    }
  } finally {
    srv.proc.kill("SIGTERM");
    fs.rmSync(dir, { recursive: true, force: true });
  }
  r.results = out;
  return r.done();
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const browser = await launch();
  let ok = false;
  try { ok = await errnavCheck(browser); } finally { await browser.close(); }
  if (!ok) process.exit(1);
}
