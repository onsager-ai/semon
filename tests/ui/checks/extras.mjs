// What only the served viewer does, on the extras fixture (fixture.mjs --extras), phone light unless noted:
//   - paging: the `backlog` lane (471 entries, one unreadable line) opens on its last page with "Load earlier" and no "Started" divider; each
//     "Load earlier" adds entries above without moving what was on screen; the first page ends with the "Started"
//     divider and every one of its 10 turns, with one gap divider where the log lost a line. A deep link (/s/claude/backlog?turn=<an older turn>) lands on that turn,
//     in view, with "Load earlier" above and "Load later" below, and "Load later" reaches the last turn.
//   - a Codex call with no exit status (deps) draws as neither failed nor succeeded: no failed styling, "exit unknown ·",
//     and its group summary counts no failure.
//   - a command longer than its summary (harbor) shows its "Command" section with the whole command.
//   - View all whose fetch fails shows the preview with the "Couldn't load the full text" note; when the fetch works,
//     no note, and the sheet's text is longer than the preview's.
//   - injection: on every screen reached (Home, Timeline, Sessions, Machines, every session including the payload lane,
//     its subagent and the failed send's stub, with every step, card and child run opened, and the details menu, and
//     every trace), the document holds exactly one script (/viewer.js), no img and no iframe,
//     nothing set window.__xss, and the payload shows as text. The payload lane and the failed-send stub also load from
//     their real URLs.
//   - no page errors.
import path from "node:path";
import { ENV, served, goto, data, reporter } from "../lib.mjs";
import { XSS, XSS_KEY } from "../fixture.mjs";

const inView = (page, sel) => page.evaluate((sel) => { const e = document.querySelector(sel), bar = document.querySelector("#topbar").getBoundingClientRect(); if (!e) return null; const r = e.getBoundingClientRect(); return r.top >= bar.bottom - 1 && r.top < innerHeight - 40; }, sel);
const pager = (page) => page.evaluate(() => [...document.querySelectorAll(".turns > .list > button.more")].map((b) => ({ text: b.textContent, first: b.parentElement === document.querySelector(".turns").firstElementChild, last: b.parentElement === document.querySelector(".turns").lastElementChild })));

export default async function (browser) {
  const r = reporter("extras");
  const R = r.results;
  const D = await data({ extras: true });

  // ---- Paging -------------------------------------------------------------------------------------------------------
  {
    const page = await served(browser, { extras: true, path: "/s/claude/backlog" });
    await page.waitForFunction(() => !!document.querySelector(".turns"));
    const turns = D.turns.filter((t) => t.sid === "backlog");
    const count = () => page.evaluate(() => ({ turns: document.querySelectorAll(".turns > .turn").length, started: [...document.querySelectorAll(".turns > .divider")].some((d) => d.textContent.startsWith("Started")) }));
    const P = { open: { ...(await count()), pager: await pager(page) }, clicks: [] };
    r.expect(turns.length === 10, "backlog has 10 turns in the model, not " + turns.length);
    r.expect(P.open.pager.length === 1 && P.open.pager[0].text === "Load earlier" && P.open.pager[0].first, "the last page opens with Load earlier at the top: " + JSON.stringify(P.open.pager));
    r.expect(!P.open.started, "no Started divider on a page that doesn't start the transcript");
    r.expect(P.open.turns > 0 && P.open.turns < 10, "the last page holds some of the turns: " + P.open.turns);
    for (let k = 0; k < 5 && (await pager(page)).some((b) => b.text === "Load earlier"); k++) {
      // Bring the button into view first (the click would scroll to it), then note where the anchor is. The anchor is the
      // second turn: the first may continue a turn whose start is on the page being loaded.
      await page.locator(".turns > .list > button.more").scrollIntoViewIfNeeded(); await page.waitForTimeout(80);
      const anchor = await page.evaluate(() => { const t = document.querySelectorAll(".turns > .turn")[1]; return { id: t.dataset.turn, top: t.getBoundingClientRect().top }; });
      const before = await count();
      await page.click(".turns > .list > button.more"); await page.waitForFunction((n) => document.querySelectorAll(".turns > .turn").length > n || ![...document.querySelectorAll(".turns > .list > button.more")].some((b) => b.textContent === "Load earlier"), before.turns);
      await page.waitForTimeout(100);
      const after = await page.evaluate((id) => document.querySelector('.turn[data-turn="' + CSS.escape(id) + '"]')?.getBoundingClientRect().top ?? null, anchor.id);
      P.clicks.push({ before: before.turns, after: (await count()).turns, anchorMoved: after == null ? null : Math.round(after - anchor.top) });
    }
    P.done = { ...(await count()), pager: await pager(page) };
    r.expect(P.clicks.length > 0 && P.clicks.every((c) => c.after >= c.before), "Load earlier adds turns: " + JSON.stringify(P.clicks));
    r.expect(P.clicks.every((c) => c.anchorMoved !== null && Math.abs(c.anchorMoved) <= 2), "what was on screen stays put after Load earlier: " + JSON.stringify(P.clicks));
    r.expect(P.done.turns === 10 && P.done.started && P.done.pager.length === 0, "the first page starts with the Started divider and holds all 10 turns: " + JSON.stringify(P.done));
    P.gaps = await page.evaluate(() => [...document.querySelectorAll(".turns > .divider")].filter((d) => d.textContent.startsWith("Some entries not included")).length);
    r.expect(P.gaps === 1, "the unreadable line shows as one gap divider between turns: " + P.gaps);
    await page.screenshot({ path: path.join(ENV.out, "extras-backlog-start.png") });
    // A deep link to an older turn.
    const older = turns[1];
    await page.goto(ENV.extraBase + "/s/claude/backlog?turn=" + encodeURIComponent(older.id), { waitUntil: "load" });
    await page.waitForFunction((id) => !!document.querySelector('.turn[data-turn="' + CSS.escape(id) + '"]'), older.id);
    await page.waitForTimeout(300);
    P.deep = { inView: await inView(page, '.turn[data-turn="' + older.id.replace(/"/g, '\\"') + '"]'), pager: await pager(page), url: await page.evaluate(() => location.pathname + location.search) };
    r.expect(P.deep.inView === true, "the deep-linked turn lands in view");
    r.expect(P.deep.pager.some((b) => b.text === "Load earlier" && b.first) && P.deep.pager.some((b) => b.text === "Load later" && b.last), "a middle page has Load earlier above and Load later below: " + JSON.stringify(P.deep.pager));
    r.expect(P.deep.url === "/s/claude/backlog?turn=" + encodeURIComponent(older.id), "the URL keeps the turn: " + P.deep.url);
    for (let k = 0; k < 5 && (await pager(page)).some((b) => b.text === "Load later"); k++) {
      await page.click(".turns > .list:last-child > button.more"); await page.waitForTimeout(250);
    }
    P.later = await page.evaluate((id) => ({ last: !!document.querySelector('.turn[data-turn="' + CSS.escape(id) + '"]'), text: document.querySelector(".turns").textContent.includes("Backlog triaged: 460 issues read."), pager: [...document.querySelectorAll(".turns > .list > button.more")].map((b) => b.textContent) }), turns.at(-1).id);
    r.expect(P.later.last && P.later.text && !P.later.pager.includes("Load later"), "Load later reaches the last turn: " + JSON.stringify(P.later));
    R.paging = P;
    r.expect(page.errors.length === 0, "paging: page errors " + page.errors.join(" | "));
    await page.context().close();
  }

  // ---- Steps: no exit status, a long command, View all ------------------------------------------------------------------
  {
    const page = await served(browser, { extras: true, path: "/s/codex/deps" });
    await page.waitForFunction(() => !!document.querySelector(".turns"));
    await page.evaluate(() => document.querySelectorAll('.tsum[aria-expanded="false"]').forEach((b) => b.click()));
    R.unknown = await page.evaluate(() => {
      const step = [...document.querySelectorAll(".step")].find((s) => s.querySelector(".sd")?.textContent.startsWith("exit unknown · "));
      const sum = step?.closest(".tgroup")?.querySelector(".tsum");
      return step ? { err: step.classList.contains("err"), sd: step.querySelector(".sd").textContent, groupFailed: !!sum?.querySelector(".tf"), grouped: !!sum } : null;
    });
    r.expect(R.unknown !== null, "a step with no exit status reads \"exit unknown · …\"");
    r.expect(R.unknown && !R.unknown.err && R.unknown.grouped && !R.unknown.groupFailed, "an unknown exit is neither failed nor counted as failed: " + JSON.stringify(R.unknown));

    await page.goto(ENV.extraBase + "/s/claude/harbor", { waitUntil: "load" }); await page.waitForFunction(() => !!document.querySelector(".turns"));
    await page.evaluate(() => document.querySelectorAll('.tsum[aria-expanded="false"]').forEach((b) => b.click()));
    R.command = await page.evaluate(() => {
      const b = [...document.querySelectorAll(".step > button")].find((x) => x.querySelector(".sa")?.textContent.endsWith("…"));
      if (!b) return null; b.click(); const out = b.parentElement.querySelector(".out");
      return { label: out.querySelector(".io")?.textContent ?? null, input: out.querySelector("pre.in")?.textContent ?? "", summary: b.querySelector(".sa").textContent };
    });
    r.expect(R.command !== null, "harbor has a step whose summary is cut");
    r.expect(R.command && R.command.label === "Command" && R.command.input.length > R.command.summary.length && R.command.input.includes("--nocapture"), "the long command shows whole under Command: " + JSON.stringify(R.command && { label: R.command.label, input: R.command.input.length, summary: R.command.summary.length }));

    // View all on the cut output: first with /api/entry failing, then working.
    const openAll = async () => {
      // The long command's step: its output is longer than the server's preview.
      await page.evaluate(() => { const b = [...document.querySelectorAll(".step > button")].find((x) => x.querySelector(".sa")?.textContent.endsWith("…")); b?.parentElement.setAttribute("data-long", ""); if (b && b.getAttribute("aria-expanded") === "false") b.click(); });
      await page.waitForTimeout(100);
      const btn = page.locator(".step[data-long] .viewall:visible").first();
      r.expect(await btn.count() === 1, "harbor's server-cut output offers View all");
      await btn.click(); await page.waitForSelector("dialog.viewer[open]"); await page.waitForTimeout(200);
      return page.evaluate(() => { const d = document.querySelector("dialog.viewer"); return { notes: [...d.querySelectorAll(".vnote")].map((n) => n.textContent), out: [...d.querySelectorAll("pre")].at(-1)?.textContent.length ?? 0 }; });
    };
    const preview = await page.evaluate(() => { const b = [...document.querySelectorAll(".step > button")].find((x) => x.querySelector(".sa")?.textContent.endsWith("…")); if (b?.getAttribute("aria-expanded") === "false") b.click(); return [...(b?.parentElement.querySelectorAll(".out pre.clip") ?? [])].at(-1)?.textContent.length ?? 0; });
    r.expect(preview > 0 && preview <= 1536 + 3, "the preview is the server's cut: " + preview);
    await page.route("**/api/entry**", (x) => x.abort());
    const failed = await openAll(); await page.click(".viewer .vclose"); await page.waitForTimeout(250);
    await page.unroute("**/api/entry**");
    const ok = await openAll(); await page.click(".viewer .vclose"); await page.waitForTimeout(250);
    R.viewAll = { preview, failed, ok };
    r.expect(failed.notes.length === 1 && failed.notes[0].startsWith("Couldn't load the full text") && failed.out === preview, "a failed fetch shows the preview with only its note: " + JSON.stringify(failed));
    r.expect(ok.notes.length === 0 && ok.out > preview, "a working fetch shows the whole text, longer than the preview: " + JSON.stringify(ok));
    r.expect(page.errors.length === 0, "steps: page errors " + page.errors.join(" | "));
    await page.context().close();
  }

  // ---- Code-mode exec: unwrapped operations and its script control -----------------------------------------------
  {
    const page = await served(browser, { extras: true, path: "/s/codex/code-mode" });
    await page.waitForFunction(() => !!document.querySelector(".turns"));
    const data = await page.evaluate(async () => {
      const token = new URLSearchParams(location.search).get("t");
      const response = await fetch("/api/tx?sid=code-mode&t=" + encodeURIComponent(token));
      const totals = await response.json();
      document.querySelectorAll('.tsum[aria-expanded="false"]').forEach((button) => button.click());
      return {
        calls: totals.calls,
        errors: totals.errors,
        steps: [...document.querySelectorAll(".step")].map((step) => ({
          arg: step.querySelector(".sa")?.textContent ?? "",
          err: step.classList.contains("err"),
        })),
        scriptButtons: [...document.querySelectorAll(".viewscript")].map((button) => button.textContent),
      };
    });
    R.codeMode = data;
    r.expect(data.calls === 3 && data.errors === 0, "three indexed operations are counted: " + JSON.stringify({ calls: data.calls, errors: data.errors }));
    r.expect(data.steps.map((step) => step.arg).join("|") === "git status|sed -n '1,9p' a.rs|src/code-mode.rs", "the steps show unwrapped commands and the changed path: " + JSON.stringify(data.steps));
    r.expect(data.steps.length === 3 && data.steps.every((step) => !step.err), "three ordinary, successful tool steps are shown");
    r.expect(data.scriptButtons.length === 1 && data.scriptButtons[0] === "View script", "the operation group has one View script control: " + JSON.stringify(data.scriptButtons));
    await page.click(".viewscript"); await page.waitForSelector("dialog.viewer[open]");
    R.codeMode.script = await page.locator(".viewer pre.script").textContent();
    r.expect(R.codeMode.script.includes("Promise.allSettled") && R.codeMode.script.includes("git status"), "View script opens the source in the existing sheet");
    await page.click(".viewer .vclose");
    r.expect(page.errors.length === 0, "code-mode page errors: " + page.errors.join(" | "));
    await page.context().close();
  }

  // ---- Injection -------------------------------------------------------------------------------------------------------
  {
    const X = { screens: 0, bad: [], payloadShown: 0 };
    const scan = async (page, where) => {
      const s = await page.evaluate((text) => ({ scripts: [...document.querySelectorAll("script")].map((x) => x.getAttribute("src")), img: document.querySelectorAll("img").length, iframe: document.querySelectorAll("iframe").length, xss: window.__xss ?? null, shown: document.body.textContent.includes(text) }), "<script>window.__xss=2</script>");
      X.screens++; if (s.shown) X.payloadShown++;
      if (s.scripts.length !== 1 || s.scripts[0] !== "/viewer.js" || s.img || s.iframe || s.xss !== null) X.bad.push(where + ": " + JSON.stringify(s));
    };
    const openEverything = (page) => page.evaluate(() => { for (let k = 0; k < 3; k++) document.querySelectorAll('.cw-toggle[aria-expanded="false"], .tsum[aria-expanded="false"], .step > button[aria-expanded="false"]').forEach((x) => x.click()); document.querySelectorAll(".hcard .more:not([hidden]), .hop .more:not([hidden])").forEach((x) => x.click()); });
    for (const [size, dark] of [["phone", false], ["desktop", true]]) {
      const page = await served(browser, { extras: true, size, dark });
      for (const v of ["home", "timeline", "sessions", "machines"]) { await goto(page, { v }, D); await scan(page, size + " " + v); }
      const sids = [...Object.keys(D.SESS), "unsent:" + XSS];
      for (const id of sids) {
        await goto(page, { v: "session", id }, D); await page.waitForTimeout(100); await openEverything(page); await page.waitForTimeout(80);
        await scan(page, size + " session " + id.slice(0, 20));
        const traces = await page.evaluate(() => [...document.querySelectorAll(".turn-end .tracebtn")].map((b) => b.closest(".turn").dataset.turn));
        for (const t of traces) { await goto(page, { v: "trace", sid: id, turn: t }, D); await openEverything(page); await scan(page, size + " trace " + t.slice(0, 20)); }
      }
      // The payload lane's details menu.
      await goto(page, { v: "session", id: XSS_KEY }, D); await openEverything(page);
      await page.click("#more-btn"); await page.waitForTimeout(150); await scan(page, size + " details menu");
      await page.keyboard.press("Escape");
      X[size + "Errors"] = page.errors;
      r.expect(page.errors.length === 0, size + " injection walk: page errors " + page.errors.join(" | "));
      await page.context().close();
    }
    // The payload lane and the failed send's stub from their real URLs.
    for (const [id, harness] of [[XSS_KEY, "claude"], ["unsent:" + XSS, "claude"]]) {
      const page = await served(browser, { extras: true, path: "/s/" + harness + "/" + encodeURIComponent(id) });
      await page.waitForFunction(() => !!document.querySelector(".turns"));
      const title = await page.evaluate(() => document.querySelector("#topbar .t")?.textContent ?? null);
      await scan(page, "url " + id.slice(0, 20));
      r.expect(title === XSS, "the real URL opens " + id.slice(0, 24) + "… with its name as text: " + JSON.stringify(title));
      r.expect(page.errors.length === 0, "url " + id.slice(0, 20) + ": page errors " + page.errors.join(" | "));
      await page.context().close();
    }
    R.injection = X;
    r.expect(X.bad.length === 0, "screens with a script, img or iframe from content, or __xss set: " + X.bad.slice(0, 5).join(" || "));
    r.expect(X.payloadShown > 10, "the payload shows as text on the screens that carry it: " + X.payloadShown + " of " + X.screens);
  }
  return r.done();
}
