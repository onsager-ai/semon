import { served, data, goto, reporter, closePage } from '../lib.mjs';
import { selectedFixture } from '../selected-fixture.mjs';

export default async function codexThinking(browser) {
  const r = reporter('codex-thinking'),
    D = await data(),
    sid = 'h-codex';
  for (const size of ['phone', 'desktop'])
    for (const dark of [false, true]) {
      const page = await served(browser, { size, dark });
      try {
        const retired = [];
        page.on('request', (request) => {
          if (/\/api\/(model|tx|events)(?:[?/#]|$)/.test(request.url()))
            retired.push(request.url());
        });
        let append = false,
          requests = 0,
          raw;
        await selectedFixture(page, sid, (data) => {
          const base = data.entries.find((row) => row.provenance),
            owner = data.relationship_context.turns[0].id;
          const values = [
            { k: 'think', text: '# Plan', secs: '1s' },
            { k: 'think', text: '# Plan\n# Read the source', secs: '2s' },
            {
              k: 'tool',
              name: 'exec_command',
              arg: 'echo native',
              out: 'recorded',
              ok: true,
              secs: '1s',
            },
            { k: 'think', text: '# After the tool', secs: '0.1s' },
            { k: 'think', text: '', pending: false },
            { k: 'think', text: '# Another thought', secs: '12s' },
          ];
          if (append)
            values.push({ k: 'think', text: '# Another thought\n# Live extension', secs: '13s' });
          data.entries = values.map((entry) => ({
            entry_id: base.entry_id,
            provenance: base.provenance,
            observed_at: base.observed_at,
            ...entry,
            turn: owner,
          }));
          requests++;
          raw = JSON.stringify(data.entries);
        });
        await goto(page, { v: 'session', id: sid }, D);
        await page.waitForSelector('.thought');
        r.expect(
          (await page.locator('.thought:not(.masked)').count()) === 3,
          size + ': tool and masked boundaries stop coalescing',
        );
        r.expect(
          (await page.locator('.thought.masked').count()) === 1,
          size + ': masked native thinking remains visible',
        );
        r.expect(
          await page
            .locator('.think-disclosure')
            .evaluateAll((nodes) =>
              nodes.every((node) => node.getAttribute('aria-expanded') === 'false'),
            ),
          size + ': recorded thinking starts collapsed',
        );
        const first = page.locator('.think-disclosure').first();
        await first.click();
        r.expect(
          (await page.locator('.think-text').first().innerText()).includes('Read the source'),
          size + ': disclosed thought contains the complete adjacent snapshot',
        );
        const before = raw;
        append = true;
        const count = requests;
        await page.waitForFunction(
          () => document.querySelectorAll('.thought:not(.masked)').length === 3,
        );
        for (let n = 0; requests === count && n < 50; n++) await page.waitForTimeout(100);
        r.expect(requests > count, size + ': selected range revalidates without a model poller');
        r.expect(
          before.includes('Read the source') && raw.includes('Live extension'),
          size + ': native wire snapshots remain complete',
        );
        r.expect(
          (await first.getAttribute('aria-expanded')) === 'true',
          size + ': a range update retains disclosure state',
        );
        r.expect(
          retired.length === 0 && page.errors.length === 0,
          size + ': no retired endpoint or page error',
        );
      } finally {
        await closePage(page);
      }
    }
  return r.done();
}
