import type { ViewerModelStore } from '../state/model';
import type { TranscriptStore } from '../state/transcript';
import type { NavigationController } from '../navigation/routes';
import type { createLiveModel } from './liveModel';
import type { createDomain } from '../domain/calculations';
import type { createTransport } from './transport';
import type { createPaging } from './paging';
import type { createTranscriptRevalidation } from './transcriptRevalidation';
import type { createApplicationRefresh } from './applicationRefresh';
import type { createAnalytics } from './analytics';
import type { createSessionChrome } from './sessionChrome';
import type { Background, Entry } from '../domain/types';
import { parseTranscriptPage, type TranscriptPage } from '../state/transcript-wire';
interface TailResult {
  cut: number | null;
  reload?: boolean;
  patched?: Entry[];
}
interface LiveUpdatesHost {
  disposed: boolean;
  navigation: NavigationController;
  isGap: (e: Entry) => boolean;

  sessionChrome: Pick<ReturnType<typeof createSessionChrome>, 'errorsLive'>;

  analytics: Pick<ReturnType<typeof createAnalytics>, 'refreshAnalytics'>;

  applicationRefreshOwner: Pick<ReturnType<typeof createApplicationRefresh>, 'refresh'>;

  transcriptRevalidationOwner: Pick<ReturnType<typeof createTranscriptRevalidation>, 'shrank'>;

  pagingOwner: Pick<ReturnType<typeof createPaging>, 'dropTx'>;

  transportOwner: Pick<
    ReturnType<typeof createTransport>,
    'adopt' | 'spread' | 'TOK' | 'api' | 'enc' | 'txEntry' | 'fetchTx'
  >;

  domain: Pick<ReturnType<typeof createDomain>, 'originHandoff'>;

  transcripts: Pick<TranscriptStore, 'cache' | 'staleBriefs' | 'entries' | 'meta' | 'updates'>;

  liveModelOwner: Pick<
    ReturnType<typeof createLiveModel>,
    'handKey' | 'LIVE' | 'cardKeys' | 'remember' | 'viewed' | 'soft'
  >;

  modelStore: Pick<ViewerModelStore, 'handoffs' | 'sessions' | 'turns' | 'holds' | 'apply'>;
}
/** Owns liveUpdates behavior through explicit application ports. */
export function createLiveUpdates(host: LiveUpdatesHost) {
  let generation = 0;
  const applyModelDelta = (value: unknown) => host.modelStore.apply(value);
  function update(value: unknown, resynchronize = false) {
    if (host.disposed) return Promise.resolve();
    const m = applyModelDelta(value);
    const oldH = new Map(
        host.modelStore.handoffs.map((h) => [h.id, host.liveModelOwner.handKey(h)]),
      ),
      oldT = host.liveModelOwner.LIVE.turns,
      oldCards = host.liveModelOwner.cardKeys(),
      names = new Map(Object.values(host.modelStore.sessions).map((x) => [x.id, x.name]));
    const hadOrigins = new Set(
      [...host.transcripts.cache.keys()].filter((sid) => !!host.domain.originHandoff(sid)),
    );
    const hadOrigin =
      host.navigation.route.v === 'session' &&
      !!host.modelStore.sessions['id' in host.navigation.route ? host.navigation.route.id : ''] &&
      !!host.domain.originHandoff('id' in host.navigation.route ? host.navigation.route.id : '');
    host.transportOwner.adopt(m);
    const revision = ++generation;
    host.liveModelOwner.remember(m);
    for (const sid of host.transcripts.staleBriefs)
      if (!host.modelStore.sessions[sid]) host.transcripts.staleBriefs.delete(sid);
    for (const sid of [...host.transcripts.cache.keys()])
      if (!hadOrigins.has(sid) && host.domain.originHandoff(sid))
        host.transcripts.cache.delete(sid);
    // A page that had no origin and now has one (its parent's spawn arrived) loads its transcript again: the first prompt it drew as
    // a message is the brief, which the intro now shows. A failed request is retried on the next poll, which backs off, up to
    // LATE_TRIES requests in all; after that the page stays as drawn (the brief shows twice until a reload) and polls as usual.
    if (
      host.liveModelOwner.LIVE.late !==
      ('id' in host.navigation.route ? host.navigation.route.id : '')
    ) {
      if (host.liveModelOwner.LIVE.late)
        host.transcripts.cache.delete(host.liveModelOwner.LIVE.late);
      host.liveModelOwner.LIVE.late = null;
    }
    if (
      host.navigation.route.v === 'session' &&
      !hadOrigin &&
      !!host.modelStore.sessions['id' in host.navigation.route ? host.navigation.route.id : ''] &&
      !!host.domain.originHandoff('id' in host.navigation.route ? host.navigation.route.id : '')
    ) {
      host.liveModelOwner.LIVE.late = 'id' in host.navigation.route ? host.navigation.route.id : '';
      host.liveModelOwner.LIVE.lateTries = 0;
      host.transcripts.staleBriefs.add(
        'id' in host.navigation.route ? host.navigation.route.id : '',
      );
      host.transcripts.cache.delete('id' in host.navigation.route ? host.navigation.route.id : '');
    }
    if (host.liveModelOwner.LIVE.late && !host.transcripts.entries[host.liveModelOwner.LIVE.late])
      host.liveModelOwner.LIVE.late = null; // nothing loaded to load again: the page loads it with its origin
    const changedH = new Set(
      host.modelStore.handoffs
        .filter((h) => oldH.get(h.id) !== host.liveModelOwner.handKey(h))
        .map((h) => h.id),
    );
    const newCards = host.liveModelOwner.cardKeys(),
      changedCards = new Set(
        [...newCards]
          .filter(([id, k]) => oldCards.has(id) && oldCards.get(id) !== k)
          .map(([id]) => id),
      );
    const view = host.liveModelOwner.viewed(),
      grown = new Set<string>(),
      cuts = new Map<string, number>(),
      patched = new Map<string, Entry[]>();
    let full =
      resynchronize ||
      Object.values(host.modelStore.sessions).some(
        (x) => names.has(x.id) && names.get(x.id) !== x.name,
      ); // a new name shows in every turn
    for (const sid of Object.keys(host.transcripts.entries)) {
      if (view.has(sid) && host.modelStore.sessions[sid]) host.transportOwner.spread(sid);
      else host.pagingOwner.dropTx(sid);
    }
    // Only transcripts whose mark in the model moved are asked for, one at a time.
    let chain = Promise.resolve();
    for (const sid of view)
      if (host.transcripts.entries[sid] && host.liveModelOwner.LIVE.late === sid)
        chain = chain.then(() =>
          host.liveModelOwner.soft(
            reloadLate(sid).then(() => {
              grown.add(sid);
              full = true;
            }),
          ),
        );
      else if (
        host.transcripts.entries[sid] &&
        host.transcripts.meta[sid].tok != null &&
        host.transportOwner.TOK[sid] != null &&
        host.transcriptRevalidationOwner.shrank(
          host.transcripts.meta[sid].tok,
          host.transportOwner.TOK[sid],
        )
      )
        chain = chain.then(() =>
          host.liveModelOwner.soft(
            reload(sid).then(() => {
              grown.add(sid);
              full = true;
            }),
          ),
        );
      else if (
        host.transcripts.entries[sid] &&
        host.transcripts.meta[sid].to >= host.transcripts.meta[sid].total &&
        host.transcripts.meta[sid].tok !== host.transportOwner.TOK[sid]
      )
        chain = chain.then(() =>
          host.liveModelOwner.soft(
            tail(sid).then((r) => {
              grown.add(sid);
              if (r.cut != null) cuts.set(sid, r.cut);
              if (r.patched?.length) patched.set(sid, r.patched);
              if (r.reload) full = true;
            }),
          ),
        );
    for (const sid of view)
      if (
        host.transcripts.entries[sid] &&
        host.transcripts.meta[sid].to < host.transcripts.meta[sid].total &&
        host.transcripts.meta[sid].watchTok !== host.transportOwner.TOK[sid]
      )
        chain = chain.then(() =>
          host.liveModelOwner.soft(
            watchLater(sid).then((r) => {
              if (r?.reload) full = true;
            }),
          ),
        );
    return chain.then(() => {
      if (host.disposed || revision !== generation) return;
      host.liveModelOwner.LIVE.version = m.version;
      host.applicationRefreshOwner.refresh(
        full ? null : dirtyTurns(cuts, grown, changedH, oldT, changedCards, patched),
      );
      if (host.navigation.route.v === 'analytics') host.analytics.refreshAnalytics();
      const e = host.sessionChrome.errorsLive();
      return e && host.liveModelOwner.soft(e);
    });
  }
  // The turns of the session page an update changed: those holding entries its tail brought (from the cut on), or a background
  // call it updated, those whose record or handoffs changed, and those holding the spawn of a child run that grew. Null: draw
  // them all.
  function dirtyTurns(
    cuts: Map<string, number>,
    grown: Set<string>,
    changedH: Set<string>,
    oldT: Map<string, string>,
    changedCards: Set<string>,
    patched: Map<string, Entry[]>,
  ) {
    if (
      host.navigation.route.v !== 'session' ||
      !host.transcripts.entries['id' in host.navigation.route ? host.navigation.route.id : '']
    )
      return null;
    const sid = 'id' in host.navigation.route ? host.navigation.route.id : '',
      dirty = new Set<string>(),
      owner = new Map(
        (host.modelStore.turns[sid] ?? []).flatMap((t) => t.entries.map((e) => [e, t.id])),
      );
    if (cuts.has(sid))
      for (const e of host.transcripts.entries[sid].slice(cuts.get(sid))) {
        if (host.isGap(e)) return null;
        if (owner.has(e)) dirty.add(owner.get(e)!);
      }
    for (const e of patched?.get(sid) ?? []) {
      if (!owner.has(e)) return null;
      dirty.add(owner.get(e)!);
    }
    for (const t of host.modelStore.turns[sid] ?? []) {
      if (
        oldT.get(t.id) !== host.liveModelOwner.LIVE.turns.get(t.id) ||
        [t.start, ...t.sent].some((h) => h && changedH.has(h.id)) ||
        t.entries.some((e) => e.k === 'h' && changedH.has(e.id))
      )
        dirty.add(t.id);
    }
    for (const h of host.modelStore.handoffs)
      if (
        h.kind === 'spawn' &&
        h.from === sid &&
        changedCards.has(h.id) &&
        host.modelStore.holds.get(h.id)
      )
        dirty.add(host.modelStore.holds.get(h.id)!.id);
    return dirty;
  }
  // A deep link keeps its contiguous window. Learn that its unseen tail grew without replacing the reader's page.
  function watchLater(sid: string) {
    const m = host.transcripts.meta[sid],
      tok = host.transportOwner.TOK[sid];
    return host.transportOwner
      .api('/api/tx?sid=' + host.transportOwner.enc(sid) + '&after=' + m.total)
      .then((value) => {
        const p = parseTranscriptPage(value);
        if (host.transcripts.meta[sid] !== m) return;
        if (p.total < m.total) return reload(sid);
        m.newer = (m.newer ?? 0) + Math.max(0, p.total - m.total);
        Object.assign(m, { total: p.total, calls: p.calls, errors: p.errors, watchTok: tok });
        host.transcripts.updates.transcript(sid);
      });
  }
  // The tail of a transcript loaded to its end: from its first foreground call still running, or the last turn's first call
  // with no result yet, since its result may have landed; else from the end. A file that shrank or was rewritten, or more
  // than five pages of new entries, loads the last page again instead.
  function tail(sid: string): Promise<TailResult> {
    const es = host.transcripts.entries[sid],
      m = host.transcripts.meta[sid],
      tok = host.transportOwner.TOK[sid];
    let cut = es.findIndex((e) => e.live && e.slot != null);
    if (cut < 0) cut = es.length;
    for (let i = es.length - 1; i >= 0; i--) {
      if (es[i].unfinished && es[i].slot != null) cut = Math.min(cut, i);
      if (es[i].turn) break;
    }
    let got: Entry[] = [],
      last: TranscriptPage | null = null,
      n = 0;
    const page = (after: number): Promise<void> =>
      host.transportOwner
        .api('/api/tx?sid=' + host.transportOwner.enc(sid) + '&after=' + after)
        .then((value) => {
          const p = parseTranscriptPage(value);
          got = got.concat(p.entries.map((e) => host.transportOwner.txEntry({ ...e, sid })));
          last = p;
          if (p.to < p.total && p.entries.length && ++n < 5) return page(p.to);
        });
    return page(cut < es.length ? es[cut].slot! : m.to).then<TailResult>(() => {
      if (host.transcripts.entries[sid] !== es) return { cut: null }; // "Load earlier" ran meanwhile: the next update catches up
      if (!last) return { cut: null };
      if (
        last.total < m.total ||
        last.to < last.total ||
        (cut < es.length && got[0]?.slot !== es[cut].slot)
      )
        return reload(sid);
      // A background call loaded before the cut is not fetched again (it may be pages back): its finish row in the tail, or the
      // page's list of calls still running (bg_running), says how it stands. Its turn is drawn again (patched).
      const ends = new Map(
          got.flatMap((e) => (e.k === 'bgend' && e.bg ? [[e.call, e.bg] as const] : [])),
        ),
        still = new Set(last.bg_running ?? []),
        patched = [];
      for (const e of es.slice(0, cut))
        if (e.bg) {
          const next: Background =
            ends.get(e.tid ?? '') ??
            (still.has(e.tid ?? '')
              ? { ...e.bg, state: 'running', secs: e.bg.secs ?? '—' }
              : e.bg.state === 'running'
                ? { state: 'unknown' }
                : e.bg);
          if (JSON.stringify(next) !== JSON.stringify(e.bg)) {
            e.bg = next;
            patched.push(e);
          }
        }
      host.transcripts.entries[sid] = es.slice(0, cut).concat(got);
      Object.assign(m, {
        to: last.to,
        total: last.total,
        calls: last.calls,
        errors: last.errors,
        tok,
      });
      host.transportOwner.spread(sid);
      host.transcripts.updates.transcript(sid);
      return { cut, patched };
    });
  }
  // The page's own transcript loads its last page again; a child run's is dropped, and loads again as new child work.
  function reload(sid: string) {
    if (sid !== ('id' in host.navigation.route ? host.navigation.route.id : '')) {
      host.pagingOwner.dropTx(sid);
      return Promise.resolve({ cut: null, reload: true });
    }
    return host.transportOwner.fetchTx(sid, '').then(() => ({ cut: null, reload: true }));
  }
  // The reload of a page whose origin arrived late (update): done once it loads; a failure is counted, makes the poll back off, and
  // after LATE_TRIES requests gives up.
  const LATE_TRIES = 4;
  function reloadLate(sid: string) {
    return reload(sid).then(
      (r) => {
        if (host.liveModelOwner.LIVE.late === sid) host.liveModelOwner.LIVE.late = null;
        return r;
      },
      (err) => {
        if (err?.status === 403) throw err;
        if (host.liveModelOwner.LIVE.late === sid) {
          if (++host.liveModelOwner.LIVE.lateTries >= LATE_TRIES) {
            host.liveModelOwner.LIVE.late = null;
            console.warn(
              'semon: gave up reloading the transcript of ' + sid + ' after its origin arrived',
            );
            // This observation has exhausted its own bounded retries. Keep the
            // transcript explicitly stale, but let unrelated model/status reads
            // resume instead of extending generic recovery backoff indefinitely.
            return { cut: null, reload: true };
          } else host.liveModelOwner.LIVE.retry = true;
        }
        throw err;
      },
    );
  }

  return { reload, tail, applyModelDelta, update };
}
