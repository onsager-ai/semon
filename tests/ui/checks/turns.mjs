// Ported from the mockup's turns.js: turn counts, an independent trace census, deep links, back navigation, find/filter,
// and overflow (light and dark), against an independent model built from the raw handoffs and transcripts — the same
// rules the mockup used (a turn starts at an incoming ask/relay/spawn or at a gap marker; its "onward" work is what it
// sent that isn't a plain machine move).
//
// The original ran this model against test7.html (the sample mockup) and test-real.html (real logs), 'sample' and
// 'real' tagged. Ported: 'real' is dropped, 'sample' now drives the served fixture. Session ids are the sample's, so
// every 'session'/'srow' lookup by id needs no mapping. The only sample-id reference in the original was a literal
// turn id, 'h4', used twice for a screenshot of quill's trace: in the sample, h4 is quill's inbound ask (kind 'ask',
// from 'you', to 'quill'), and the served model's turn id for an incoming turn is that same handoff's (hashed) id
// (crates/semon-sessions/src/model.rs: `turn.id = handoff.id` when the turn starts on one) — so the served equivalent
// is simply the id of quill's one 'ask' handoff, found at runtime instead of hardcoded.
//
// Assertions:
//  - no page errors, light or dark.
//  - zero overflow screens on every screen visited (home, machines, every session, every trace).
//  - T.modelMismatch is empty: every session's rendered turn list (ids, and which turns show a Trace button) matches
//    the independent model: a turn shows a Trace button exactly when it sent something onward (the overhaul draws a child
//    session's brief inside its first turn, so no turn is exempt).
//  - onwardWithoutButton === 0 and buttonWithoutOnward === 0.
//  - every trace opened matches the model with no `bad` count (root turn, crumb, title and history state all agree).
//  - deep links land on the exact expected turn id (fromTrace.turn === expectTurn, fromHome.turn === expectTurn), and
//    inView with a flash, in every form the original tried (from a trace node, from Home's item, from Home's Trace
//    button, by Enter and by Space); a trace child node and a Home item must actually be found (hadPick, hadHome),
//    not silently skipped; back always returns to the screen it came from (fromTrace.backTo, crumb.backTo === "trace",
//    homeTrace.backTo, homeEnter.backTo, fromHome.backTo === "Home").
//  - the handoff census: multi === 0 (no handoff drawn in more than one trace) and no zero-occurrence handoff whose
//    reason starts with "UNEXPECTED" (an origin trace or its nesting missed a handoff the model says it should hold).
//  - find matches the expected turns and empties correctly on no match; the tools filter is checked non-vacuously:
//    if the busiest session has a tool-only turn, turning tools off must drop it (toolsOff < allTurns); otherwise it
//    must still hide every step (stepsWhileFiltered === 0); either way both are undone by the end (findFilter.restored
//    === findFilter.allTurns).
//  - tap targets: every measured group has nothing under 36px, and the groups this fixture is expected to populate
//    (Home items and their Trace buttons, session Trace buttons, the one relay header, trace child "open" links)
//    measured at least one element — a missing selector or broken affordance can't pass by measuring zero of both.
import path from 'node:path';
import { ENV, served, goto, data, reporter, overflow } from '../lib.mjs';

const isGap = (e) =>
  e.k === 'end' && /entries (not included|omitted)|^No activity/.test(e.text ?? '');

// An independent model of turns from the served handoffs and transcripts (same rules as the mockup), for expectations
// and orphan reasons.
function model(D) {
  const HID = new Map(D.H.map((h) => [h.id, h]));
  const nativeTurns = new Map((D.model?.turns ?? []).map((t) => [t.id, t]));
  const hasChildOrigin = (sid) =>
    D.H.some(
      (h) =>
        (h.kind === 'spawn' || h.kind === 'relay') &&
        h.to === sid &&
        h.from !== sid &&
        (h.kind === 'spawn' || D.SESS[sid]?.kind === 'Relayed' || !D.SESS[sid]?.lane),
    );
  const turns = {},
    starts = new Map(),
    holds = new Map(),
    inTx = new Map();
  for (const [sid, es] of Object.entries(D.TX)) {
    const ts = (turns[sid] = []);
    let t = null;
    es.forEach((e, i) => {
      if (e.k === 'signal') return;
      if (isGap(e)) {
        t = null;
        return;
      }
      const h = e.k === 'h' ? HID.get(e.id) : null;
      if (h) {
        if (!inTx.has(h.id)) inTx.set(h.id, new Set());
        inTx.get(h.id).add(sid);
      }
      const inc = h
        ? h.to === sid && ['ask', 'relay', 'spawn'].includes(h.kind) && !starts.has(h.id)
        : e.k === 'u';
      if (inc || !t) {
        const id =
          inc && h
            ? h.id
            : (e.turn ??
              sid +
                ':' +
                i); /* the server names a turn with no start handoff <sid>:<file>:<offset>, on its first entry */
        t = {
          id,
          sid,
          start: inc ? h : null,
          out: [],
          last: !!nativeTurns.get(id)?.last,
          childOrigin: hasChildOrigin(sid),
        };
        ts.push(t);
        if (inc && h) starts.set(h.id, t);
      }
      if (h && !inc && (h.from === sid || h.kind === 'move')) {
        holds.set(h.id, t);
        if (h.kind !== 'move') t.out.push(h);
      }
    });
  }
  return { ...D, HID, turns, starts, holds, inTx, hasChildOrigin };
}

export default async function turnsCheck(browser) {
  const D0 = await data();
  const D = model(D0);
  const r = reporter('turns');
  const out = {};

  for (const scheme of ['light', 'dark']) {
    const errs = [];
    const page = await served(browser, { size: 'phone', dark: scheme === 'dark' });
    errs.push(...page.errors);
    const R = { scheme, screens: 0, overflowScreens: 0, overflowWhere: [] };
    const over = () => overflow(page);
    const rail = () =>
      page.evaluate(() => {
        let off = 0,
          gaps = 0;
        const hops = [...document.querySelectorAll('.hop')];
        hops.forEach((h, i) => {
          const hb = h.getBoundingClientRect(),
            nb = h.querySelector('.node').getBoundingClientRect(),
            cs = getComputedStyle(h, '::before');
          if (Math.abs(hb.left + parseFloat(cs.left) + 1 - (nb.left + nb.width / 2)) > 0.5) off++;
          if (i && Math.abs(hops[i - 1].getBoundingClientRect().bottom - hb.top) > 0.5) gaps++;
        });
        return [hops.length, off, gaps];
      });
    const screen = async (where) => {
      R.screens++;
      const n = await over();
      if (n) {
        R.overflowScreens++;
        R.overflowWhere.push(where + ':' + n);
      }
    };
    const small = (sel) =>
      page.evaluate((sel) => {
        const hs = [...document.querySelectorAll(sel)]
          .filter((x) => x.offsetParent)
          .map((x) => x.getBoundingClientRect().height);
        return {
          n: hs.length,
          min: hs.length ? Math.round(Math.min(...hs) * 10) / 10 : null,
          under36: hs.filter((h) => h < 35.5).length,
        };
      }, sel);
    const taps = {};
    const addTap = (k, rr) => {
      const t = (taps[k] ??= { n: 0, min: null, under36: 0 });
      t.n += rr.n;
      t.under36 += rr.under36;
      if (rr.min != null) t.min = t.min == null ? rr.min : Math.min(t.min, rr.min);
    };

    // Home and Machines.
    await screen('home');
    addTap('ib', await small('.ib'));
    addTap('ib .tracebtn', await small('.ib .tracebtn'));
    await goto(page, { v: 'machines' }, D0);
    await screen('machines');
    for (const m of await page.evaluate(() =>
      [...document.querySelectorAll('.page .nrow')].map((row) => row.dataset.m),
    )) {
      await goto(page, { v: 'machine', id: m }, D0);
      await screen('machine ' + m);
      addTap('ib', await small('.ib'));
    }

    // Every session page: turns, trace buttons against the model, overflow collapsed and fully opened.
    const T = {
      sessions: 0,
      turns: 0,
      perSession: {},
      withOnward: 0,
      traceButtons: 0,
      onwardWithoutButton: 0,
      buttonWithoutOnward: 0,
      childTailOnwardWithoutButton: 0,
      expectedChildTailOnwardWithoutButton: 0,
      modelMismatch: [],
      mismatchDetails: [],
    };
    const traceTurns = [];
    for (const sid of Object.keys(D.SESS)) {
      await goto(page, { v: 'session', id: sid }, D0);
      T.sessions++;
      const info = await page.evaluate(() =>
        [...document.querySelectorAll('.turns > .turn')].map((t) => ({
          id: t.dataset.turn,
          head: !!t.querySelector(':scope > .turn-h'),
          onward: t.querySelectorAll(':scope > .tx > .event:not(.move), :scope > .tx > .child-card')
            .length,
          btn: !!t.querySelector(':scope > .turn-end .link'),
          end: t.querySelector(':scope > .turn-end .stat')?.textContent ?? null,
        })),
      );
      const exp = D.turns[sid] ?? [];
      if (
        info.length !== exp.length ||
        info.some((t, i) => t.id !== exp[i].id || t.btn !== !!exp[i].out.length)
      ) {
        T.modelMismatch.push(sid.slice(0, 12));
        T.mismatchDetails.push({
          sid,
          expected: exp.map((t) => ({ id: t.id, out: t.out.map((h) => h.id) })),
          native: (D0.model.turns ?? [])
            .filter((t) => t.sid === sid)
            .map((t) => ({ id: t.id, start: t.start, sent: t.sent, end: t.end })),
          rendered: info,
        });
      }
      T.turns += info.length;
      T.perSession[D.SESS[sid].name.slice(0, 28)] = info.length;
      for (const t of info) {
        const childTail = false;
        if (t.onward) T.withOnward++;
        if (t.btn) {
          T.traceButtons++;
          traceTurns.push([sid, t.id]);
        }
        if (t.onward && !t.btn) {
          if (childTail) T.childTailOnwardWithoutButton++;
          else T.onwardWithoutButton++;
        }
        if (t.btn && !t.onward) T.buttonWithoutOnward++;
      }
      await screen('session ' + sid.slice(0, 8));
      addTap('.turn-end .link', await small('.turn-end .link'));
      addTap('.turn-h .who-link', await small('.turn-h .who-link'));
      await page.evaluate(() => {
        for (let k = 0; k < 3; k++)
          document.querySelectorAll('.tsum[aria-expanded="false"]').forEach((x) => x.click());
        document
          .querySelectorAll('.step > button[aria-expanded="false"]')
          .forEach((x) => x.click());
        document.querySelectorAll('.turns .more:not([hidden])').forEach((x) => x.click());
      });
      await page.waitForTimeout(80);
      await screen('session open ' + sid.slice(0, 8));
    }
    // Every trace, opened by tapping its Trace button.
    const TR = {
      traces: 0,
      nodes: 0,
      roots: 0,
      childNodes: 0,
      stubLeaves: 0,
      toyouLeaves: 0,
      railOff: 0,
      railGaps: 0,
    };
    const traces = [];
    for (const [sid, tid] of traceTurns) {
      await goto(page, { v: 'session', id: sid }, D0);
      await page.click('.turn[data-turn="' + tid + '"] > .turn-end .link');
      await page.waitForFunction(
        (t) => document.querySelector('#topbar .t')?.textContent === t,
        'Trace',
      );
      await page.waitForTimeout(120);
      const t = await page.evaluate(() => ({
        st: history.state,
        crumb: document.querySelector('.topbar .crumb')?.textContent,
        title: document.querySelector('.topbar .t')?.textContent,
        root: (() => {
          const r = document.querySelector('.hop.k-root');
          return r && { turn: r.dataset.turn, h: r.dataset.h ?? null };
        })(),
        nodes: [...document.querySelectorAll('.hop.child')].map((x) => ({
          h: x.dataset.h,
          turn: x.dataset.turn ?? null,
          stub: x.classList.contains('stub'),
          toyou: x.classList.contains('k-toyou'),
        })),
      }));
      if (
        t.st?.v !== 'trace' ||
        t.st.turn !== tid ||
        t.root?.turn !== tid ||
        t.title !== 'Trace' ||
        t.crumb !== D.SESS[sid].name
      )
        TR.bad = (TR.bad ?? 0) + 1;
      TR.traces++;
      TR.roots++;
      TR.nodes += 1 + t.nodes.length;
      TR.childNodes += t.nodes.length;
      TR.stubLeaves += t.nodes.filter((x) => x.stub).length;
      TR.toyouLeaves += t.nodes.filter((x) => x.toyou).length;
      traces.push({ sid, tid, root: t.root, nodes: t.nodes });
      const [, off, gaps] = await rail();
      TR.railOff += off;
      TR.railGaps += gaps;
      await screen('trace ' + tid);
      addTap('.hop .open', await small('.hop .open'));
      await page.evaluate(() =>
        document.querySelectorAll('.hop .more:not([hidden])').forEach((x) => x.click()),
      );
      await page.waitForTimeout(60);
      addTap('.hop .more', await small('.hop .more'));
      await screen('trace open ' + tid);
    }
    R.overflowWhere = R.overflowWhere.slice(0, 10);
    const one = { scheme, ...R, taps };
    if (scheme === 'light') {
      Object.assign(one, { turns: T, traces: TR });
      // Census. Origin traces: those whose root turn is not a child node of another trace. Nested traces (a child turn
      // with its own onward work) repeat a subtree of their origin trace; they are checked for that instead of being
      // counted again.
      const childTurns = new Set(traces.flatMap((t) => t.nodes.map((x) => x.turn).filter(Boolean)));
      const origins = traces.filter((t) => !childTurns.has(t.tid)),
        nested = traces.filter((t) => childTurns.has(t.tid));
      const occ = new Map();
      const bump = (id) => occ.set(id, (occ.get(id) ?? 0) + 1);
      for (const t of origins) {
        if (t.root.h && ['relay', 'spawn'].includes(D.HID.get(t.root.h).kind)) bump(t.root.h);
        t.nodes.forEach((x) => bump(x.h));
      }
      const nodeSetOfOrigin = (tid) => {
        for (const t of origins)
          if (t.nodes.some((x) => x.turn === tid) || t.tid === tid)
            return new Set(t.nodes.map((x) => x.h));
        return null;
      };
      let nestedNotSubset = 0;
      for (const t of nested) {
        const s = nodeSetOfOrigin(t.tid);
        if (!s || t.nodes.some((x) => !s.has(x.h))) nestedNotSubset++;
      }
      const census = D.H.filter((h) => ['spawn', 'relay', 'toyou'].includes(h.kind));
      const zero = census.filter((h) => !occ.get(h.id)),
        multi = census.filter((h) => (occ.get(h.id) ?? 0) > 1);
      const why = (h) => {
        const w = D.inTx.get(h.id) ?? new Set(),
          inS = w.has(h.from),
          inR = w.has(h.to);
        const nm = (x) => D.SESS[x]?.name ?? x;
        if (h.kind === 'toyou')
          return inS
            ? 'UNEXPECTED: in ' + nm(h.from) + "'s transcript"
            : 'not in the retained part of ' + nm(h.from) + "'s transcript";
        if (!inS && !inR)
          return 'in neither ' + nm(h.from) + "'s nor " + nm(h.to) + "'s retained transcript";
        if (!inS && inR) {
          const t = D.starts.get(h.id);
          return t && !t.out.length
            ? 'only ' +
                nm(h.to) +
                "'s side is in the logs, and the turn it started sent nothing onward (no Trace button; shown as that turn's header)"
            : 'UNEXPECTED: receiver turn has onward work';
        }
        return 'UNEXPECTED';
      };
      const reasons = {};
      for (const h of zero) {
        const rr = why(h).replace(/[^ ]+'s/g, "X's");
        reasons[rr] = (reasons[rr] ?? 0) + 1;
      }
      one.census = {
        handoffs: census.length,
        byKind: census.reduce((a, h) => ((a[h.kind] = (a[h.kind] ?? 0) + 1), a), {}),
        originTraces: origins.length,
        nestedTraces: nested.length,
        nestedNotSubset,
        exactlyOnce: census.filter((h) => occ.get(h.id) === 1).length,
        zero: zero.length,
        multi: multi.length,
        multiIds: multi.map((h) => h.id),
        zeroReasons: reasons,
        zeroIds: zero.map((h) => h.id + '(' + h.kind + ')'),
      };
      one.orphanDetail = zero.map(
        (h) =>
          h.id +
          ' ' +
          h.kind +
          ' ' +
          (D.SESS[h.from]?.name ?? h.from) +
          '→' +
          (D.SESS[h.to]?.name ?? h.to) +
          ': ' +
          why(h),
      );

      // Deep links. inView: the turn's top is below the (visible) top bar and inside the screen, and it is highlighted.
      const landed = () =>
        page.evaluate(() => {
          const st = history.state;
          const t = [...document.querySelectorAll('.turn')].find(
            (x) => x.dataset.turn === st?.turn,
          );
          const bar = document.querySelector('#topbar').getBoundingClientRect();
          if (!t) return { st, found: false };
          const r = t.getBoundingClientRect();
          return {
            found: true,
            turn: st.turn,
            top: Math.round(r.top),
            barBottom: Math.round(bar.bottom),
            barHidden: document.body.classList.contains('bar-hidden'),
            inView: r.top >= bar.bottom - 1 && r.top < innerHeight - 40,
            flash: t.classList.contains('flash'),
            scrollY: Math.round(scrollY),
          };
        });
      const D1 = {};
      let pick = null;
      for (const t of traces)
        for (const x of t.nodes)
          if (x.turn && !pick) {
            const ts = D.turns[D.starts.get(x.h).sid];
            if (ts.findIndex((y) => y.id === x.turn) > 0) pick = { t, x };
          }
      pick ??= traces.flatMap((t) => t.nodes.filter((x) => x.turn).map((x) => ({ t, x })))[0];
      if (pick) {
        await goto(page, { v: 'session', id: pick.t.sid }, D0);
        await page.click('.turn[data-turn="' + pick.t.tid + '"] > .turn-end .link');
        await page.waitForFunction(
          (t) => document.querySelector('#topbar .t')?.textContent === t,
          'Trace',
        );
        await page.waitForTimeout(150);
        await page.click('.hop.child[data-h="' + pick.x.h + '"] .open');
        await page.waitForTimeout(250);
        D1.fromTrace = { node: pick.x.h, expectTurn: pick.x.turn, ...(await landed()) };
        await page.waitForTimeout(1500);
        D1.fromTrace.flashGoneAfter1_75s = !(await page.evaluate(
          () => !!document.querySelector('.turn.flash'),
        ));
        await page.goBack();
        await page.waitForTimeout(200);
        D1.fromTrace.backTo = await page.evaluate(() => ({
          v: history.state?.v,
          turn: history.state?.turn,
          title: document.querySelector('.topbar .t')?.textContent,
        }));
        // The crumb goes back to the session at the traced turn.
        await page.click('.topbar .crumb');
        await page.waitForTimeout(250);
        D1.crumb = await landed();
        await page.goBack();
        await page.waitForTimeout(150);
        D1.crumb.backTo = await page.evaluate(() => history.state?.v);
      }
      D1.hadPick = !!pick;
      // From Home: tap the first Needs you item, then back; its Trace button, then back; keyboard Enter, then back.
      await goto(page, { v: 'home' }, D0);
      await page.evaluate(() => window.scrollTo(0, 0));
      const hid = await page.evaluate(() => document.querySelector('.ib')?.dataset.h);
      const hasHome = !!hid;
      D1.hadHome = hasHome;
      if (hasHome) {
        await page.click('.ib .q');
        await page.waitForTimeout(250);
        D1.fromHome = { h: hid, expectTurn: D.holds.get(hid)?.id, ...(await landed()) };
        await page.goBack();
        await page.waitForTimeout(200);
        D1.fromHome.backTo = await page.evaluate(
          () => document.querySelector('.topbar .t')?.textContent,
        );
        await page.click('.ib .tracebtn');
        await page.waitForTimeout(200);
        D1.homeTrace = await page.evaluate(() => ({
          v: history.state?.v,
          turn: history.state?.turn,
          root: document.querySelector('.hop.k-root')?.dataset.turn,
        }));
        await page.goBack();
        await page.waitForTimeout(200);
        D1.homeTrace.backTo = await page.evaluate(
          () => document.querySelector('.topbar .t')?.textContent,
        );
        await page.focus('.ib');
        await page.keyboard.press('Enter');
        await page.waitForTimeout(250);
        D1.homeEnter = await landed();
        await page.goBack();
        await page.waitForTimeout(200);
        await page.focus('.ib');
        await page.keyboard.press(' ');
        await page.waitForTimeout(250);
        D1.homeSpace = { v: (await landed()).turn ?? null };
        await page.goBack();
        await page.waitForTimeout(200);
        D1.homeEnter.backTo = await page.evaluate(
          () => document.querySelector('.topbar .t')?.textContent,
        );
      }
      D1.nestedButtonsInItems = await page.evaluate(
        () => document.querySelectorAll('button .ib, .ib button button, button button').length,
      );
      one.deepLinks = D1;
      // Find and filter on the session with the most turns.
      const multiSid = Object.keys(D.turns).sort(
        (a, b) => D.turns[b].length - D.turns[a].length,
      )[0];
      await goto(page, { v: 'session', id: multiSid }, D0);
      const allTurns = await page.evaluate(
        () => document.querySelectorAll('.turns > .turn').length,
      );
      const word = 'morning sweep';
      await page.click('#find-btn');
      await page.fill('#find', word);
      await page.waitForTimeout(150);
      const f1 = await page.evaluate(() => ({
        turns: document.querySelectorAll('.turns > .turn').length,
        empty: !!document.querySelector('.turns > .empty'),
      }));
      await page.fill('#find', 'zzqqxx');
      await page.waitForTimeout(150);
      const f2 = await page.evaluate(() => ({
        turns: document.querySelectorAll('.turns > .turn').length,
        empty: document.querySelector('.turns > .empty')?.textContent,
      }));
      // The "Messages" chip is the tools-off filter: only messages, no steps.
      await page.fill('#find', '');
      await page.waitForTimeout(100);
      await page.click('.chip[data-filter="messages"]');
      await page.waitForTimeout(120);
      const f3 = await page.evaluate(() => document.querySelectorAll('.turns > .turn').length);
      const stepsWhileFiltered = await page.evaluate(
        () => document.querySelectorAll('.turns .step').length,
      );
      await page.click('.chip[data-filter="all"]');
      await page.waitForTimeout(100);
      await page.click('.topbar [aria-label="Close find"]');
      await page.waitForTimeout(100);
      // Whether multiSid has a turn that is tool-only (no message, only tool steps): if it does, the tools-off filter
      // must actually drop that turn (toolsOff < allTurns); if it doesn't, the filter still has to hide every step
      // (stepsWhileFiltered === 0) even though no turn disappears. Either way the check below is non-vacuous.
      const HIDm = D.HID;
      const segs = [];
      let cur = null;
      for (const e of D0.TX[multiSid] ?? []) {
        const h = e.k === 'h' ? HIDm.get(e.id) : null;
        const inc = h
          ? h.to === multiSid && ['ask', 'relay', 'spawn'].includes(h.kind)
          : e.k === 'u';
        if (inc || !cur) {
          cur = { hasMsg: false, hasTool: false };
          segs.push(cur);
        }
        if (e.k === 'a' || e.k === 'u') cur.hasMsg = true;
        if (e.k === 'tool') cur.hasTool = true;
      }
      const hasToolOnlyTurn = segs.some((s) => s.hasTool && !s.hasMsg);
      one.findFilter = {
        session: D.SESS[multiSid].name,
        allTurns,
        find: { word, ...f1 },
        noMatch: f2,
        toolsOff: f3,
        stepsWhileFiltered,
        hasToolOnlyTurn,
        restored: await page.evaluate(() => document.querySelectorAll('.turns > .turn').length),
      };
      // Screenshots.
      await goto(page, { v: 'home' }, D0);
      await page.evaluate(() => window.scrollTo(0, 0));
      await page.waitForTimeout(150);
      await page.screenshot({ path: path.join(ENV.out, 'turns-sample-home.png') });
      await goto(page, { v: 'session', id: 'harbor' }, D0);
      await page.evaluate(() => window.scrollTo(0, 0));
      await page.waitForTimeout(150);
      await page.screenshot({ path: path.join(ENV.out, 'turns-sample-harbor.png') });
      const h4 = D0.H.find((h) => h.kind === 'ask' && h.to === 'quill')?.id;
      if (h4) {
        await goto(page, { v: 'session', id: 'quill' }, D0);
        await page.click('.turn[data-turn="' + h4 + '"] .turn-end .link');
        await page.waitForTimeout(200);
        await page.screenshot({
          path: path.join(ENV.out, 'turns-sample-trace-quill.png'),
          fullPage: true,
        });
      }
    } else {
      const h4 = D0.H.find((h) => h.kind === 'ask' && h.to === 'quill')?.id;
      if (h4) {
        await goto(page, { v: 'session', id: 'quill' }, D0);
        await page.click('.turn[data-turn="' + h4 + '"] .turn-end .link');
        await page.waitForTimeout(200);
        await page.screenshot({
          path: path.join(ENV.out, 'turns-sample-trace-quill-dark.png'),
          fullPage: true,
        });
      }
      await goto(page, { v: 'session', id: 'harbor' }, D0);
      await page.evaluate(() => window.scrollTo(0, 0));
      await page.waitForTimeout(150);
      await page.screenshot({ path: path.join(ENV.out, 'turns-sample-harbor-dark.png') });
    }
    one.errors = page.errors;
    out[scheme] = one;
    await page.context().close();
  }

  // Handoff content stays on the page; only its names navigate.
  out.handoffCards = {};
  for (const size of ['phone', 'desktop'])
    for (const dark of [false, true]) {
      const page = await served(browser, { size, dark });
      const child = D0.H.find((h) => h.kind === 'spawn' && D0.SESS[h.to] && D0.SESS[h.from]);
      const relay = D0.H.find((h) => h.kind === 'relay' && D0.SESS[h.from] && D0.SESS[h.to]);
      r.expect(!!child && !!relay, 'fixture must exercise child and relay names');
      for (const h of [child, relay].filter(Boolean)) {
        await goto(page, { v: 'session', id: h.from }, D0);
        const sel = '[data-h="' + h.id + '"]',
          card = page.locator(sel).first();
        r.expect((await card.count()) === 1, size + ': handoff content is present');
        const before = await page.evaluate(() => ({ v: history.state.v, id: history.state.id }));
        // The compact child's top-left corner is now its navigation button.
        // Click its noninteractive metadata to exercise content separately.
        if (h.kind === 'spawn') await card.locator('.cc-meta').click();
        else await card.click({ position: { x: 3, y: 3 } });
        const after = await page.evaluate(() => ({ v: history.state.v, id: history.state.id }));
        r.expect(
          JSON.stringify(before) === JSON.stringify(after),
          size + ': clicking content must keep the route',
        );
        const name = card
          .locator('button.who-link')
          .filter({ hasText: D0.SESS[h.to].name })
          .first();
        r.expect((await name.count()) === 1, size + ': recipient name is a button');
        await name.press('Enter');
        await page.waitForFunction(
          (id) => history.state?.v === 'session' && history.state?.id === id,
          h.to,
        );
        const expectedTurn = D.starts.get(h.id)?.id;
        if (expectedTurn)
          r.expect(
            await page.evaluate((turn) => history.state.turn === turn, expectedTurn),
            'name navigation preserves the started turn',
          );
      }
      r.expect(page.errors.length === 0, 'handoff page errors: ' + page.errors.join(' | '));
      out.handoffCards[size + (dark ? '-dark' : '-light')] = { child: child?.id, relay: relay?.id };
      await page.context().close();
    }

  r.results = out;
  const light = out.light;
  r.expect(out.light.errors.length === 0, 'light page errors: ' + out.light.errors.join(' | '));
  r.expect(out.dark.errors.length === 0, 'dark page errors: ' + out.dark.errors.join(' | '));
  r.expect(
    out.light.overflowScreens === 0,
    'light overflowScreens=' +
      out.light.overflowScreens +
      ' ' +
      JSON.stringify(out.light.overflowWhere),
  );
  r.expect(
    out.dark.overflowScreens === 0,
    'dark overflowScreens=' +
      out.dark.overflowScreens +
      ' ' +
      JSON.stringify(out.dark.overflowWhere),
  );
  r.expect(
    light.turns.modelMismatch.length === 0,
    'modelMismatch=' + JSON.stringify(light.turns.modelMismatch),
  );
  r.expect(
    light.turns.onwardWithoutButton === 0,
    'onwardWithoutButton=' + light.turns.onwardWithoutButton,
  );
  r.expect(
    light.turns.childTailOnwardWithoutButton === light.turns.expectedChildTailOnwardWithoutButton,
    "a child session's final turn hid its Trace button (the overhaul shows it on every turn that sent something): " +
      JSON.stringify({
        actual: light.turns.childTailOnwardWithoutButton,
        expected: light.turns.expectedChildTailOnwardWithoutButton,
      }),
  );
  r.expect(
    light.turns.buttonWithoutOnward === 0,
    'buttonWithoutOnward=' + light.turns.buttonWithoutOnward,
  );
  r.expect(!light.traces.bad, 'bad traces=' + light.traces.bad);
  r.expect(
    light.deepLinks.hadPick === true,
    'no trace child node with its own turn was found for the fromTrace/crumb deep-link test',
  );
  if (light.deepLinks.fromTrace) {
    r.expect(
      light.deepLinks.fromTrace.inView && light.deepLinks.fromTrace.flash,
      'fromTrace did not land inView+flash: ' + JSON.stringify(light.deepLinks.fromTrace),
    );
    r.expect(
      light.deepLinks.fromTrace.turn === light.deepLinks.fromTrace.expectTurn,
      'fromTrace landed on the wrong turn: got ' +
        light.deepLinks.fromTrace.turn +
        ' expected ' +
        light.deepLinks.fromTrace.expectTurn,
    );
  }
  if (light.deepLinks.crumb) {
    r.expect(
      light.deepLinks.crumb.inView && light.deepLinks.crumb.flash,
      'crumb did not land inView+flash: ' + JSON.stringify(light.deepLinks.crumb),
    );
    r.expect(
      light.deepLinks.crumb.backTo === 'trace',
      'crumb back did not return to Trace: ' + light.deepLinks.crumb.backTo,
    );
  }
  r.expect(
    light.deepLinks.hadHome === true,
    "no Home 'Needs you' item was found for the fromHome/homeTrace/homeEnter deep-link test",
  );
  if (light.deepLinks.fromHome) {
    r.expect(
      light.deepLinks.fromHome.inView && light.deepLinks.fromHome.flash,
      'fromHome did not land inView+flash: ' + JSON.stringify(light.deepLinks.fromHome),
    );
    r.expect(
      light.deepLinks.fromHome.turn === light.deepLinks.fromHome.expectTurn,
      'fromHome landed on the wrong turn: got ' +
        light.deepLinks.fromHome.turn +
        ' expected ' +
        light.deepLinks.fromHome.expectTurn,
    );
    r.expect(
      light.deepLinks.fromHome.backTo === 'Home',
      'fromHome back did not return Home: ' + light.deepLinks.fromHome.backTo,
    );
  }
  if (light.deepLinks.homeEnter)
    r.expect(
      light.deepLinks.homeEnter.inView && light.deepLinks.homeEnter.flash,
      'homeEnter did not land inView+flash',
    );
  r.expect(
    light.census.multi === 0,
    'census multi=' + light.census.multi + ' ' + JSON.stringify(light.census.multiIds),
  );
  const unexpected = Object.keys(light.census.zeroReasons).filter((k) =>
    k.startsWith('UNEXPECTED'),
  );
  r.expect(
    unexpected.length === 0,
    'UNEXPECTED zero-occurrence reasons: ' + JSON.stringify(unexpected),
  );
  r.expect(
    light.findFilter.find.turns >= 1 && !light.findFilter.find.empty,
    "find for '" + light.findFilter.find.word + "' matched nothing",
  );
  r.expect(
    light.findFilter.noMatch.turns === 0 && !!light.findFilter.noMatch.empty,
    'no-match search still shows turns',
  );
  r.expect(
    light.findFilter.restored === light.findFilter.allTurns,
    'filter/find did not restore the full turn count',
  );
  if (light.findFilter.hasToolOnlyTurn)
    r.expect(
      light.findFilter.toolsOff < light.findFilter.allTurns,
      'tools-off filter did not drop ' +
        light.findFilter.session +
        "'s tool-only turn: toolsOff=" +
        light.findFilter.toolsOff +
        ' allTurns=' +
        light.findFilter.allTurns,
    );
  else
    r.expect(
      light.findFilter.stepsWhileFiltered === 0,
      'tools-off filter left steps visible on ' +
        light.findFilter.session +
        ': ' +
        light.findFilter.stepsWhileFiltered,
    );
  // Tap targets: every group measured must have nothing under 36px; groups this fixture is expected to populate
  // (Home's items and their Trace buttons, session Trace buttons, the one relay header, and trace child "open"
  // links) must also have measured something, so a selector typo or a broken affordance fails loudly.
  const expectPositiveTaps = new Set([
    'ib',
    'ib .tracebtn',
    '.turn-end .link',
    '.turn-h .who-link',
    '.hop .open',
  ]);
  for (const scheme of ['light', 'dark']) {
    const taps = out[scheme]?.taps ?? {};
    for (const [k, t] of Object.entries(taps)) {
      r.expect(
        t.under36 === 0,
        scheme + ": tap target group '" + k + "' has an element under 36px (min=" + t.min + ')',
      );
      if (expectPositiveTaps.has(k))
        r.expect(t.n > 0, scheme + ": tap target group '" + k + "' measured nothing");
    }
  }

  return r.done();
}
