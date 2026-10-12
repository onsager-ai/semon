// Synthetic presentation changes retain the real, qualified native source proof.
// Tests replace only the selected range, never the retired workspace model.
import { createHash } from 'node:crypto';
export async function selectedFixture(page, sid, change, { publication } = {}) {
  let observation;
  const publishedGeneration = () =>
    publication &&
    createHash('sha256')
      .update(JSON.stringify([observation.source, sid, publication().revision]))
      .digest('hex');
  await page.route('**/api/sessions?**', async (route) => {
    const args = new URL(route.request().url()).searchParams;
    if (args.get('sid') !== sid || !observation || args.get('machine') !== observation.source)
      return route.continue();
    const response = await route.fetch();
    if (response.status() !== 200) return route.fulfill({ response });
    const data = await response.json();
    data.items = data.items.map((item) =>
      item.key === sid
        ? {
            ...item,
            details: observation.session.details,
            transcript: {
              generation: publishedGeneration() ?? observation.generation,
              total: publication?.().total ?? observation.rows.length,
            },
          }
        : item,
    );
    await route.fulfill({ response, json: data });
  });
  await page.route('**/api/session-annotations?**', async (route) => {
    const url = new URL(route.request().url()),
      args = url.searchParams;
    if (args.get('sid') !== sid) return route.continue();
    if (
      !observation ||
      (publication && publishedGeneration() !== observation.generation) ||
      (args.has('generation') && args.get('generation') !== observation.generation)
    )
      return route.fulfill({
        status: 409,
        json: {
          api: 1,
          error: { code: 'stale_projection', message: 'Selected fixture changed', retryable: true },
        },
      });
    const kind = args.get('kind'),
      rows = observation.rows.filter((row) =>
        kind === 'signals' ? row.k === 'signal' : row.k === 'tool' && row.ok === false,
      ),
      first = Number(args.get('after') ?? 0),
      end = Math.min(rows.length, first + Number(args.get('limit') ?? 100));
    await route.fulfill({
      json: {
        api: 1,
        source_key: observation.source,
        catalog_key: sid,
        scope: 'retained_history',
        kind,
        generation: observation.generation,
        total: rows.length,
        first,
        entries: rows.slice(first, end).map((row) => ({ slot: row.slot })),
        next_cursor: end < rows.length ? end : null,
      },
    });
  });
  await page.route('**/api/session-transcript?**', async (route) => {
    const url = new URL(route.request().url());
    if (url.searchParams.get('sid') !== sid) return route.continue();
    const original = new URL(url);
    original.searchParams.delete('generation');
    original.searchParams.delete('after');
    original.searchParams.set('limit', '100');
    const headers = { ...route.request().headers() };
    delete headers['if-none-match'];
    const response = await route.fetch({ url: original.href, headers });
    if (response.status() !== 200) return route.fulfill({ response });
    const data = await response.json();
    await change(data);
    const rows = data.entries.map((row, slot) => ({
      ...row,
      slot,
      entry_id: `${sid}:synthetic:${slot}`,
    }));
    data.projection.generation = createHash('sha256').update(JSON.stringify(rows)).digest('hex');
    observation = {
      source: data.identity.source_key,
      session: data.session,
      rows,
      generation: data.projection.generation,
    };
    data.projection.generation = publishedGeneration() ?? data.projection.generation;
    observation.generation = data.projection.generation;
    if (
      url.searchParams.has('generation') &&
      url.searchParams.get('generation') !== data.projection.generation
    )
      return route.fulfill({
        status: 409,
        json: {
          api: 1,
          error: {
            code: 'projection_changed',
            message: 'Selected fixture changed',
            retryable: true,
          },
        },
      });
    data.projection.total = rows.length;
    data.session.transcript = { generation: data.projection.generation, total: rows.length };
    const limit = Number(url.searchParams.get('limit') ?? 60);
    const first = url.searchParams.has('after')
      ? Math.min(Number(url.searchParams.get('after')), rows.length)
      : Math.max(0, rows.length - limit);
    const end = Math.min(first + limit, rows.length);
    data.range = { first, end, next: end < rows.length ? end : null };
    data.entries = rows.slice(first, end);
    const turnIds = new Set(data.entries.map((row) => row.turn).filter(Boolean));
    const context = data.relationship_context;
    const turns = (context.turns ?? []).filter((turn) => turnIds.has(turn.id));
    const actions = new Set(data.entries.filter((row) => row.k === 'h').map((row) => row.id));
    for (const turn of turns) if (turn.start) actions.add(turn.start);
    data.relationship_context = {
      state: 'incomplete',
      turn_ids: [...turnIds],
      turns,
      handoffs: (context.handoffs ?? []).filter((row) => actions.has(row.id)),
      sessions: context.sessions ?? {},
      previews: context.previews ?? {},
    };
    await route.fulfill({ response, json: data });
  });
}
