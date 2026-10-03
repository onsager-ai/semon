import { served, data, goto, settled, reporter } from '../lib.mjs';

const CODEX_SID = 'h-codex',
  CLAUDE_SID = 'harbor';

function turnFrom(base, id, keepStart = false) {
  return {
    ...structuredClone(base),
    id,
    sid: base.sid,
    start: keepStart ? base.start : null,
    u: !keepStart,
    text: keepStart ? '' : 'Synthetic transcript turn',
    sent: [],
    last: false,
    end: null,
  };
}

function add(entries, k, turn, fields = {}) {
  const entry = { k, ...fields, slot: entries.length };
  if (turn) entry.turn = turn;
  entries.push(entry);
}

const tool = (entries, turn, title, command) =>
  add(entries, 'tool', turn, {
    name: 'exec_command',
    title,
    arg: command,
    in: command,
    out: 'ok',
    ok: true,
    exit: 0,
    secs: '1s',
  });

export default async function codexThinkingCheck(browser) {
  const D = await data(),
    r = reporter('codex-thinking'),
    R = {};
  const codexBase = D.model.turns.find((t) => t.sid === CODEX_SID && t.start);
  const origin = codexBase && D.H.find((h) => h.id === codexBase.start && h.to === CODEX_SID);
  const claudeBase = D.model.turns.find((t) => t.sid === CLAUDE_SID);
  r.expect(
    D.SESS[CODEX_SID]?.harness === 'codex' && !!codexBase,
    'the h-codex fixture session or its started turn is missing',
  );
  r.expect(
    D.SESS[CLAUDE_SID]?.harness === 'claude' && !!claudeBase,
    'the harbor Claude fixture session or its turn is missing',
  );
  r.expect(
    !!origin && origin.kind === 'spawn',
    'h-codex no longer has its expected parent spawn for nested rendering',
  );
  if (!codexBase || !claudeBase || !origin) return r.done();

  const codexTurn = codexBase.id,
    boundaryTurn = 'codex-thinking-boundary',
    liveTurn = 'codex-thinking-live';
  const turns = [
    turnFrom(codexBase, codexTurn, true),
    turnFrom(codexBase, boundaryTurn),
    turnFrom(codexBase, liveTurn),
  ];
  const entries = [];
  add(entries, 'think', null, { text: '# Prelude' });
  add(entries, 'think', null, { text: '# Prelude\n# Before turns' });
  add(entries, 'think', 'codex-thinking-removed-turn-a', { text: '# Plan' });
  add(entries, 'think', null, { text: '# Plan\n# Stage A' });
  add(entries, 'think', 'codex-thinking-removed-turn-b', { text: '# Plan\n# Stage A\n# Stage B' });
  add(entries, 'think', null, { text: '# Plan\n# Stage A\n# Stage B\n# Stage C' });
  add(entries, 'think', codexTurn, { text: '# Plan', secs: '1s' });
  add(entries, 'think', null, { text: '# Plan\n# Stage A', secs: '2s' });
  add(entries, 'think', null, { text: '# Plan\n# Stage A\n# Stage B', secs: '3s' });
  add(entries, 'think', null, { text: '# Plan\n# Stage A\n# Stage B', secs: '4s' });
  add(entries, 'think', null, { text: '# Plan\n# Stage A\n# Stage B\n# Stage C', secs: '5s' });
  add(entries, 'think', null, { text: '# Plan\n# Stage A\n# Stage B\n# Stage C', secs: '6s' });
  add(entries, 'think', null, { text: '# Plan\n# Stage A\n# Stage B\n# Stage C', secs: '7s' });

  add(entries, 'think', boundaryTurn, {
    text: '# Plan\n# Stage A\n# Stage B\n# Stage C\n# Next turn',
    secs: '1s',
  });
  tool(entries, null, 'Fixture command A', 'echo alpha');
  tool(entries, null, 'Fixture command B', 'echo beta');
  add(entries, 'think', null, { text: '# After tool', secs: '1s' });
  add(entries, 'think', null, { text: '# After tool\n# Tool continuation', secs: '2s' });
  add(entries, 'harness', null, { label: 'Codex' });
  add(entries, 'think', null, { text: '# After tool\n# Tool continuation', secs: '1s' });
  add(entries, 'think', null, {
    text: '# After tool\n# Tool continuation\n# Harness continuation',
    secs: '2s',
  });
  add(entries, 'u', null, { text: 'A user message boundary' });
  add(entries, 'think', null, { text: '# After tool\n# Tool continuation', secs: '1s' });
  add(entries, 'think', null, {
    text: '# After tool\n# Tool continuation\n# User continuation',
    secs: '2s',
  });
  add(entries, 'end', null, { text: 'Turn completed' });
  add(entries, 'think', null, { text: '# After end', secs: '1s' });
  add(entries, 'think', null, { text: '# After end\n# End continuation', secs: '2s' });
  add(entries, 'think', null, { text: '# Plan', secs: '1s' });
  add(entries, 'think', null, { text: '# Planning', secs: '2s' });
  add(entries, 'think', null, { text: '# Lines\n# B', secs: '1s' });
  add(entries, 'think', null, { text: '# Lines\n# BC', secs: '2s' });
  add(entries, 'think', null, { text: '# Shrink\n# Continues', secs: '1s' });
  add(entries, 'think', null, { text: '# Shrink', secs: '2s' });
  add(entries, 'think', null, { text: '# Mask boundary', secs: '1s' });
  add(entries, 'think', null, { text: '' });
  add(entries, 'think', null, { text: '# Mask boundary\n# Mask continuation', secs: '2s' });
  add(entries, 'think', null, { text: '# Pending boundary', secs: '1s' });
  add(entries, 'think', null, { text: '', pending: true, status: 'thinking' });
  add(entries, 'think', null, { text: '# Pending boundary\n# Pending continuation', secs: '2s' });
  add(entries, 'think', 'codex-thinking-unresolved-turn', {
    text: '# Unowned after a turn',
    secs: '1s',
  });
  add(entries, 'think', null, { text: '# Unowned after a turn\n# Later', secs: '2s' });

  tool(entries, liveTurn, 'Live fixture command', 'echo keep-open');
  add(entries, 'think', null, { text: '# Live plan', secs: '2s' });
  add(entries, 'think', null, { text: '# Live plan\n# Stage A', secs: '4s' });
  const liveTail = [
    { k: 'think', text: '# Live plan\n# Stage A\n# Stage B', secs: '8s', slot: entries.length },
  ];
  const liveTailSourceRaw = JSON.stringify(liveTail),
    tailResponseRaw = JSON.stringify([{ ...liveTail[0], turn: liveTurn }]);

  const claudeTurn = 'claude-thinking-unchanged';
  const claudeEntries = [
    { k: 'think', turn: claudeTurn, text: '# Claude plan', secs: '1s', slot: 0 },
    { k: 'think', text: '# Claude plan\n# Continues', secs: '2s', slot: 1 },
  ];
  const claudeTurns = [turnFrom(claudeBase, claudeTurn)];
  const codexRaw = JSON.stringify(entries),
    tailRaw = tailResponseRaw;
  let epoch = 0,
    claudeEnabled = false;
  const modelFor = () => {
    const model = structuredClone(D.model);
    model.turns = model.turns
      .filter((t) => t.sid !== CODEX_SID && (!claudeEnabled || t.sid !== CLAUDE_SID))
      .concat(turns, ...(claudeEnabled ? [claudeTurns] : []));
    model.version = 'codex-thinking-' + epoch + (claudeEnabled ? '-claude' : '');
    model.tx = { ...(model.tx ?? {}), [CODEX_SID]: epoch ? '1.2' : '1.1' };
    if (claudeEnabled) model.tx[CLAUDE_SID] = '1.1';
    return model;
  };
  const entriesFor = (sid) =>
    sid === CODEX_SID
      ? epoch
        ? entries.concat(liveTail)
        : entries
      : claudeEnabled && sid === CLAUDE_SID
        ? claudeEntries
        : null;
  const page = await served(browser);
  await page.addInitScript((sid) => {
    window.__thinkingTxRaw = [];
    window.__thinkingTxObjects = [];
    const json = Response.prototype.json;
    Response.prototype.json = function (...args) {
      const url = this.url;
      return json.apply(this, args).then((value) => {
        const parsed = new URL(url, location.href);
        if (parsed.pathname === '/api/tx' && parsed.searchParams.get('sid') === sid) {
          window.__thinkingTxRaw.push(JSON.stringify(value.entries));
          window.__thinkingTxObjects.push(value.entries);
        }
        return value;
      });
    };
  }, CODEX_SID);
  await page.route('**/api/model**', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(modelFor()),
    }),
  );
  await page.route('**/api/tx**', async (route) => {
    const url = new URL(route.request().url()),
      sid = url.searchParams.get('sid'),
      all = entriesFor(sid);
    if (!all) return route.fallback();
    let selected = all,
      from = 0,
      total = all.length;
    const turnId = url.searchParams.get('turn');
    if (turnId) {
      from = all.findIndex((e) => e.turn === turnId);
      if (from < 0)
        return route.fulfill({ status: 404, contentType: 'text/plain', body: 'turn not found' });
      selected = all.slice(from);
    } else if (url.searchParams.has('after')) {
      from = Number(url.searchParams.get('after')) || 0;
      selected = all.slice(from);
      let owner = null;
      for (const entry of all.slice(0, from + 1)) if (entry.turn) owner = entry.turn;
      if (selected.length && selected[0].turn == null && owner)
        selected = [{ ...selected[0], turn: owner }, ...selected.slice(1)];
    }
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        entries: selected,
        from,
        to: from + selected.length,
        total,
        calls: all.filter((e) => e.k === 'tool').length,
        errors: all.filter((e) => e.k === 'tool' && e.ok === false).length,
      }),
    });
  });
  await page.reload();
  await settled(page);
  await goto(page, { v: 'session', id: CODEX_SID }, D);

  const initialRaw = await page.evaluate(() => window.__thinkingTxRaw[0] ?? null);
  R.rawInput = {
    present: !!initialRaw,
    unchanged: initialRaw === codexRaw,
    count: initialRaw ? JSON.parse(initialRaw).length : 0,
  };
  r.expect(
    R.rawInput.present && R.rawInput.unchanged && R.rawInput.count === entries.length,
    'the original Codex transcript response changed before display preparation',
  );

  R.initial = await page.evaluate(
    ({ firstTurn, secondTurn }) => {
      const turns = [...document.querySelectorAll('.turns > .turn')];
      const byId = (id) => turns.find((x) => x.dataset.turn === id);
      const rows = (id) =>
        [...(byId(id)?.querySelectorAll('.thought:not(.masked)') ?? [])].map((n) => ({
          key: n.dataset.e,
          label: n.querySelector('.think-label')?.textContent?.replace(/\s+/g, ' ').trim() ?? '',
          headings: [...n.querySelectorAll('.think-text [role=heading]')].map((h) =>
            h.textContent.trim(),
          ),
        }));
      const second = byId(secondTurn),
        tx = second?.querySelector(':scope > .tx');
      const order = [...(tx?.children ?? [])].map((n) =>
        n.classList.contains('tgroup')
          ? 'tools'
          : n.classList.contains('harness-note')
            ? 'harness'
            : n.matches('.msg.user')
              ? 'user'
              : n.classList.contains('divider')
                ? 'end'
                : n.classList.contains('thought')
                  ? 'thought'
                  : n.className,
      );
      return {
        first: rows(firstTurn),
        secondFirst: rows(secondTurn)[0] ?? null,
        secondRows: rows(secondTurn),
        order,
        unowned: [...document.querySelectorAll('.turns > .thought:not(.masked)')].map((n) => ({
          key: n.dataset.e,
          headings: [...n.querySelectorAll('.think-text [role=heading]')].map((h) =>
            h.textContent.trim(),
          ),
        })),
        masked: second?.querySelectorAll(':scope > .tx > .thought.masked').length ?? 0,
        maskedLabel:
          second?.querySelector(':scope > .tx > .thought.masked')?.textContent.trim() ?? '',
        pending: second?.querySelectorAll(':scope > .tx > .think-pending').length ?? 0,
        pendingLabel:
          second?.querySelector(':scope > .tx > .think-pending')?.textContent.trim() ?? '',
        group: second
          ? (() => {
              const g = second.querySelector('.tgroup'),
                b = g?.querySelector('.tsum');
              return {
                count: g?.querySelectorAll('.step').length ?? 0,
                summary: g?.querySelector('.tt')?.textContent?.replace(/\s+/g, ' ').trim() ?? '',
                titles: [...(g?.querySelectorAll('.step .sa.st') ?? [])].map((x) => x.textContent),
                statuses: [...(g?.querySelectorAll('.step .sd') ?? [])].map((x) =>
                  x.textContent.replace(/\s+/g, ' ').trim(),
                ),
                errors: g?.querySelectorAll('.step.err').length ?? 0,
                expanded: b?.getAttribute('aria-expanded') === 'true',
                stepsVisible: g ? !g.querySelector('.steps').hidden : false,
              };
            })()
          : null,
        t2: byId(secondTurn)?.dataset.turn ?? null,
      };
    },
    { firstTurn: codexTurn, secondTurn: boundaryTurn },
  );
  const prelude = R.initial.unowned.filter((x) => x.headings[0] === 'Prelude');
  const unresolvedBeforeOwner = R.initial.unowned.filter((x) => x.headings[0] === 'Plan');
  r.expect(
    prelude.length === 1 &&
      JSON.stringify(prelude[0].headings) === JSON.stringify(['Prelude', 'Before turns']) &&
      prelude[0].key === CODEX_SID + '#0',
    'an unowned contiguous transcript prelude did not keep one stable display row: ' +
      JSON.stringify(prelude),
  );
  r.expect(
    JSON.stringify(unresolvedBeforeOwner) ===
      JSON.stringify([
        { key: CODEX_SID + '#2', headings: ['Plan'] },
        { key: CODEX_SID + '#3', headings: ['Plan', 'Stage A'] },
        { key: CODEX_SID + '#4', headings: ['Plan', 'Stage A', 'Stage B'] },
        { key: CODEX_SID + '#5', headings: ['Plan', 'Stage A', 'Stage B', 'Stage C'] },
      ]),
    'missing explicit turn owners merged with each other or following ownerless rows: ' +
      JSON.stringify(unresolvedBeforeOwner),
  );
  r.expect(
    R.initial.first.length === 1 &&
      JSON.stringify(R.initial.first[0].headings) ===
        JSON.stringify(['Plan', 'Stage A', 'Stage B', 'Stage C']) &&
      R.initial.first[0].label === 'Thinking · 7s',
    'growing and equal Codex snapshots did not keep the latest complete text and duration in one row: ' +
      JSON.stringify(R.initial.first),
  );
  r.expect(
    R.initial.first[0]?.key === codexTurn + '#0' && R.initial.t2 === boundaryTurn,
    'the coalesced first row lost its stable key or turn-opening anchor',
  );
  r.expect(
    JSON.stringify(R.initial.secondFirst?.headings) ===
      JSON.stringify(['Plan', 'Stage A', 'Stage B', 'Stage C', 'Next turn']) &&
      R.initial.secondFirst?.key === boundaryTurn + '#0',
    'adjacent same-prefix thoughts from different turns were merged',
  );
  r.expect(
    R.initial.group?.count === 2 &&
      R.initial.group.summary === 'Ran 2 commands' &&
      R.initial.group.expanded === false &&
      R.initial.group.stepsVisible === false,
    'intervening tool calls did not remain an intact collapsed group: ' +
      JSON.stringify(R.initial.group),
  );
  r.expect(
    JSON.stringify(R.initial.group?.titles) ===
      JSON.stringify(['Fixture command A', 'Fixture command B']),
    'the grouped tool labels changed',
  );
  r.expect(
    JSON.stringify(R.initial.group?.statuses) === JSON.stringify(['exit 0 · 1s', 'exit 0 · 1s']) &&
      R.initial.group?.errors === 0,
    'the grouped tool status changed',
  );
  const afterTool = R.initial.secondRows.filter((x) => x.headings[0] === 'After tool');
  r.expect(
    afterTool.length === 3 &&
      JSON.stringify(afterTool.map((x) => x.headings.length)) === JSON.stringify([2, 3, 3]),
    'tool, harness, or user boundaries failed to end a Codex snapshot chain: ' +
      JSON.stringify(afterTool),
  );
  r.expect(
    R.initial.order.indexOf('tools') < R.initial.order.indexOf('harness') &&
      R.initial.order.indexOf('harness') < R.initial.order.indexOf('user') &&
      R.initial.order.indexOf('user') < R.initial.order.indexOf('end'),
    'tool, harness, user, and end markers changed order: ' + JSON.stringify(R.initial.order),
  );
  const findRows = (first) =>
    R.initial.secondRows.filter((x) => x.headings[0] === first).map((x) => x.headings);
  r.expect(
    findRows('Unowned after a turn').length === 2,
    'unowned thoughts after a known turn were merged without clear ownership: ' +
      JSON.stringify(findRows('Unowned after a turn')),
  );
  r.expect(
    R.initial.secondRows.filter((x) => JSON.stringify(x.headings) === JSON.stringify(['Plan']))
      .length === 1 &&
      R.initial.secondRows.filter(
        (x) => JSON.stringify(x.headings) === JSON.stringify(['Planning']),
      ).length === 1,
    'a partial lexical prefix was incorrectly coalesced',
  );
  r.expect(
    JSON.stringify(findRows('Lines')) ===
      JSON.stringify([
        ['Lines', 'B'],
        ['Lines', 'BC'],
      ]),
    'a prefix ending inside a line was incorrectly coalesced',
  );
  r.expect(
    JSON.stringify(findRows('Shrink')) === JSON.stringify([['Shrink', 'Continues'], ['Shrink']]),
    'a shrinking summary was incorrectly coalesced',
  );
  r.expect(
    R.initial.masked === 1 && findRows('Mask boundary').length === 2,
    'masked thinking did not break the Codex snapshot chain',
  );
  r.expect(
    R.initial.maskedLabel === 'Thinking hidden by the harness',
    'masked Codex thinking changed its existing quiet label',
  );
  r.expect(
    R.initial.pending === 1 && findRows('Pending boundary').length === 2,
    'pending thinking did not break the Codex snapshot chain',
  );
  r.expect(
    R.initial.pendingLabel === 'Thinking…',
    'pending Codex thinking changed its existing label',
  );

  await page.evaluate((id) => {
    const node = [...document.querySelectorAll('.turn')].find((x) => x.dataset.turn === id);
    window.__thinkingUnchangedTurn = [...document.querySelectorAll('.turn')].find(
      (x) => x.dataset.turn === 'codex-thinking-boundary',
    );
    node?.querySelector('.step > button')?.click();
  }, liveTurn);
  R.liveBefore = await page.evaluate((id) => {
    const turn = [...document.querySelectorAll('.turn')].find((x) => x.dataset.turn === id),
      thought = turn?.querySelector('.thought:not(.masked)');
    return {
      key: thought?.dataset.e ?? null,
      turn: thought?.closest('.turn')?.dataset.turn ?? null,
      toolOpen: turn?.querySelector('.step > button')?.getAttribute('aria-expanded') === 'true',
    };
  }, liveTurn);
  r.expect(
    R.liveBefore.key === liveTurn + '#1' && R.liveBefore.turn === liveTurn && R.liveBefore.toolOpen,
    'the live fixture did not establish its stable thought key and expanded tool state',
  );
  epoch = 1;
  await page.evaluate(() => window.dispatchEvent(new Event('semon:refresh')));
  await page.waitForFunction(
    (id) => {
      const turn = [...document.querySelectorAll('.turn')].find((x) => x.dataset.turn === id),
        thought = turn?.querySelector('.thought:not(.masked) .think-text');
      return thought?.textContent.includes('Stage B');
    },
    liveTurn,
    { timeout: 10000, polling: 50 },
  );
  R.liveAfter = await page.evaluate((id) => {
    const turn = [...document.querySelectorAll('.turn')].find((x) => x.dataset.turn === id),
      thought = turn?.querySelector('.thought:not(.masked)'),
      body = thought?.querySelector('.think-text');
    return {
      key: thought?.dataset.e ?? null,
      turn: thought?.closest('.turn')?.dataset.turn ?? null,
      label: thought?.querySelector('.think-label')?.textContent?.replace(/\s+/g, ' ').trim() ?? '',
      headings: [...(body?.querySelectorAll('[role=heading]') ?? [])].map((h) =>
        h.textContent.trim(),
      ),
      toolOpen: turn?.querySelector('.step > button')?.getAttribute('aria-expanded') === 'true',
      keptOtherTurn:
        window.__thinkingUnchangedTurn ===
        [...document.querySelectorAll('.turn')].find(
          (x) => x.dataset.turn === 'codex-thinking-boundary',
        ),
    };
  }, liveTurn);
  R.rawAfterLive = await page.evaluate(() =>
    window.__thinkingTxObjects.map((raw, i) => ({
      before: window.__thinkingTxRaw[i],
      after: JSON.stringify(raw),
    })),
  );
  r.expect(
    JSON.stringify(R.liveAfter.headings) === JSON.stringify(['Live plan', 'Stage A', 'Stage B']) &&
      R.liveAfter.headings.every((h, i, a) => a.indexOf(h) === i) &&
      R.liveAfter.label === 'Thinking · 8s',
    'an appended live summary repeated a heading or lost its latest duration: ' +
      JSON.stringify(R.liveAfter),
  );
  r.expect(
    R.liveAfter.key === R.liveBefore.key &&
      R.liveAfter.turn === liveTurn &&
      R.liveAfter.toolOpen &&
      R.liveAfter.keptOtherTurn,
    'the live turn patch lost the oldest key, tool state, or an untouched turn: ' +
      JSON.stringify(R.liveAfter),
  );
  r.expect(
    R.rawAfterLive.every((x) => x.before === x.after) &&
      R.rawAfterLive.some((x) => x.before === tailRaw),
    'display preparation mutated a raw API snapshot or changed the appended entry: ' +
      JSON.stringify(R.rawAfterLive),
  );
  r.expect(
    JSON.stringify(liveTail) === liveTailSourceRaw,
    'the after-page fixture mutated its raw source entry while adding its page-start owner',
  );

  await page.reload();
  await settled(page);
  await goto(page, { v: 'session', id: CODEX_SID }, D);
  R.reload = await page.evaluate((id) => {
    const turn = [...document.querySelectorAll('.turn')].find((x) => x.dataset.turn === id),
      thought = turn?.querySelector('.thought:not(.masked)'),
      body = thought?.querySelector('.think-text');
    return {
      key: thought?.dataset.e ?? null,
      turn: thought?.closest('.turn')?.dataset.turn ?? null,
      label: thought?.querySelector('.think-label')?.textContent?.replace(/\s+/g, ' ').trim() ?? '',
      headings: [...(body?.querySelectorAll('[role=heading]') ?? [])].map((h) =>
        h.textContent.trim(),
      ),
    };
  }, liveTurn);
  r.expect(
    R.reload.key === R.liveBefore.key &&
      R.reload.turn === liveTurn &&
      JSON.stringify(R.reload.headings) === JSON.stringify(R.liveAfter.headings) &&
      R.reload.label === R.liveAfter.label,
    'the full reload changed the retained snapshot key, turn anchor, text, or duration',
  );

  await page.reload();
  await settled(page);
  await goto(page, { v: 'session', id: origin.from }, D);
  R.parentHandoff = await page.evaluate(
    (id) => !!document.querySelector('.child-card[data-h="' + CSS.escape(id) + '"]'),
    origin.id,
  );
  r.expect(
    R.parentHandoff,
    "the parent retains the child card while the child's coalesced thinking lives on its own page",
  );
  r.expect(
    (await page.locator('.child-work').count()) === 0,
    'parent cards must not load or duplicate child transcripts',
  );

  claudeEnabled = true;
  await page.reload();
  await settled(page);
  await goto(page, { v: 'session', id: CLAUDE_SID }, D);
  R.claude = await page.evaluate((id) => {
    const turn = [...document.querySelectorAll('.turn')].find((x) => x.dataset.turn === id);
    return [...(turn?.querySelectorAll('.thought:not(.masked)') ?? [])].map((n) =>
      [...n.querySelectorAll('.think-text [role=heading]')].map((h) => h.textContent.trim()),
    );
  }, claudeTurn);
  r.expect(
    JSON.stringify(R.claude) === JSON.stringify([['Claude plan'], ['Claude plan', 'Continues']]),
    'Claude thinking was changed by Codex-only coalescing: ' + JSON.stringify(R.claude),
  );
  r.expect(page.errors.length === 0, 'page errors: ' + page.errors.join(' | '));
  R.pageErrors = page.errors;
  r.results = R;
  await page.context().close();
  return r.done();
}
