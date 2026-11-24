// Synthetic presentation changes retain the real, qualified native source proof.
import { createHash } from 'node:crypto';

const digest = (value) => createHash('sha256').update(JSON.stringify(value)).digest('hex');

export async function selectedFixture(page, sid, change, { publication } = {}) {
  let observation;
  const progress = () => publication && digest([sid, publication().revision]);
  await page.route('**/api/sessions?**', async (route) => {
    const args = new URL(route.request().url()).searchParams;
    if (
      !observation ||
      args.get('machine') !== observation.source ||
      args.get('sid') !== sid ||
      args.get('observe') !== 'transcript'
    )
      return route.continue();
    const published = publication?.();
    await route.fulfill({
      json: {
        api: 1,
        machine: observation.source,
        catalog_key: sid,
        read_scope: args.get('scope'),
        transcript: {
          generation: progress() ?? observation.generation,
          errors_generation: observation.errorsGeneration,
          total: published?.total ?? observation.rows.length,
        },
      },
    });
  });
  await page.route('**/api/session-annotations?**', async (route) => {
    const args = new URL(route.request().url()).searchParams;
    if (args.get('sid') !== sid || args.get('machine') !== observation?.source)
      return route.continue();
    const kind = args.get('kind');
    const generation = kind === 'errors' ? observation.errorsGeneration : observation.generation;
    if (args.has('generation') && args.get('generation') !== generation)
      return route.fulfill({ status: 409, json: staleCursor });
    const rows = observation.rows.filter((row) =>
      kind === 'signals' ? row.k === 'signal' : row.k === 'tool' && row.ok === false,
    );
    const first = Number(args.get('after') ?? 0);
    const end = Math.min(rows.length, first + Number(args.get('limit') ?? 100));
    await route.fulfill({
      json: {
        api: 1,
        projection: 7,
        source_key: observation.source,
        catalog_key: sid,
        scope: args.get('scope'),
        kind,
        generation,
        total: rows.length,
        first,
        entries: rows.slice(first, end).map(({ address }) => ({ address })),
      },
    });
  });
  await page.route('**/api/session-transcript?**', async (route) => {
    const url = new URL(route.request().url());
    const args = url.searchParams;
    if (args.get('sid') !== sid) return route.continue();
    const original = new URL(url);
    original.searchParams.delete('address');
    original.searchParams.delete('cursor');
    original.searchParams.delete('limit');
    const headers = { ...route.request().headers() };
    delete headers['if-none-match'];
    const response = await route.fetch({ url: original.href, headers });
    if (response.status() !== 200) return route.fulfill({ response });
    const data = await response.json();
    const addresses = new Map(data.entries.map((row) => [row.entry_id, row.address]));
    await change(data);
    data.identity.generation = progress() ?? data.identity.generation;
    const rows = data.entries.map((row, index) => {
      const entry_id = digest([sid, index, row.entry_id]);
      const address = { ...(row.address ?? addresses.get(row.entry_id)), entry_id };
      return { ...row, entry_id, address };
    });
    observation = {
      source: data.identity.source_key,
      session: data.session,
      rows,
      generation: data.identity.generation,
      errorsGeneration: data.errors_generation,
    };
    const limit = Number(args.get('limit') ?? 32);
    const revision = digest(rows);
    const cursor = args.has('cursor') ? JSON.parse(args.get('cursor')) : null;
    const address = args.has('address') ? JSON.parse(args.get('address')) : null;
    if (cursor && cursor.revision !== revision)
      return route.fulfill({ status: 409, json: staleCursor });
    if (address) {
      data.entries = rows.filter((row) => JSON.stringify(row.address) === JSON.stringify(address));
      if (!data.entries.length) return route.fulfill({ status: 409, json: staleCursor });
      data.position = 'address';
      data.earlier_cursor = null;
    } else {
      const end = cursor?.before ?? rows.length;
      const start = Math.max(0, end - limit);
      data.position = cursor ? 'earlier' : 'latest';
      data.earlier_cursor = start ? JSON.stringify({ before: start, revision }) : null;
      data.entries = rows.slice(start, end);
    }
    const context = data.relationship_context;
    const turnIds = new Set(data.entries.map((row) => row.turn).filter(Boolean));
    const turns = context.turns.filter((turn) => turnIds.has(turn.id));
    const actions = new Set(data.entries.filter((row) => row.k === 'h').map((row) => row.id));
    for (const turn of turns) if (turn.start) actions.add(turn.start);
    data.relationship_context = {
      ...context,
      state: 'incomplete',
      turn_ids: [...turnIds],
      turns,
      handoffs: context.handoffs.filter((row) => actions.has(row.id)),
    };
    await route.fulfill({ response, json: data });
  });
}

const staleCursor = {
  api: 1,
  error: { code: 'stale_cursor', message: 'Selected fixture changed', retryable: true },
};
