import type { Background, Entry } from '../domain/types';
import { parseTranscriptPage, type TranscriptPage } from '../state/transcript-wire';
interface TailResult {
  cut: number | null;
  reload?: boolean;
  patched?: Entry[];
}
interface LiveUpdatesHost {
  modelStore: import('../state/model').ViewerModelStore;
  H: import('../domain/types').Handoff[];
  handKey: (h: import('../domain/types').Handoff) => string;
  LIVE: import('../lib/live').LiveState;
  cardKeys: () => Map<string, string>;
  SESS: Record<string, import('../domain/types').Session>;
  TXCACHE: import('../lib/routes').TranscriptCache<
    import('../domain/types').Entry,
    import('../domain/types').TranscriptMeta
  >;
  originHandoff: (sid: string) => import('../domain/types').Handoff | undefined;
  navigation: import('../navigation/routes').NavigationController;
  adopt: (value: unknown) => import('../lib/model').ModelWire;
  remember: (_m: import('../lib/model').ModelWire) => void;
  STALE_BRIEFS: Set<string>;
  TX: Record<string, import('../domain/types').Entry[]>;
  viewed: () => Set<string>;
  spread: (sid: string) => void;
  dropTx: (sid: string) => void;
  soft: <T>(p: Promise<T>) => Promise<void | T>;
  TXM: Record<string, import('../domain/types').TranscriptMeta>;
  TOK: Record<string, string>;
  shrank: (a: string | undefined, b: string | undefined) => boolean;
  refresh: (dirty?: ReadonlySet<string> | null) => void;
  refreshAnalytics: (asked?: boolean) => Promise<void>;
  errorsLive: () => Promise<void> | null;
  TURNS: Record<string, import('../domain/types').Turn[]>;
  isGap: (e: import('../domain/types').Entry) => boolean;
  HOLDS: Map<string, import('../domain/types').Turn>;
  api: (path: string, signal?: AbortSignal | undefined, unchanged?: boolean) => Promise<unknown>;
  enc: (uriComponent: string | number | boolean) => string;
  txEntry: (e: import('../domain/types').Entry) => import('../domain/types').Entry;
  fetchTx: (
    sid: string,
    q?: string,
    where?: import('../state/transcript').PageDirection | undefined,
    signal?: AbortSignal | undefined,
    onPage?: (() => void) | undefined,
  ) => Promise<void>;
}
/** Owns liveUpdates behavior through explicit application ports. */
export function createLiveUpdates(host: LiveUpdatesHost) {
  const applyModelDelta = (value: unknown) => host.modelStore.apply(value);
  function update(value: unknown) {
    const m = applyModelDelta(value);
    const oldH = new Map(host.H.map((h) => [h.id, host.handKey(h)])),
      oldT = host.LIVE.turns,
      oldCards = host.cardKeys(),
      names = new Map(Object.values(host.SESS).map((x) => [x.id, x.name]));
    const hadOrigins = new Set([...host.TXCACHE.keys()].filter((sid) => !!host.originHandoff(sid)));
    const hadOrigin =
      host.navigation.route.v === 'session' &&
      !!host.SESS['id' in host.navigation.route ? host.navigation.route.id : ''] &&
      !!host.originHandoff('id' in host.navigation.route ? host.navigation.route.id : '');
    host.adopt(m);
    host.remember(m);
    for (const sid of host.STALE_BRIEFS) if (!host.SESS[sid]) host.STALE_BRIEFS.delete(sid);
    for (const sid of [...host.TXCACHE.keys()])
      if (!hadOrigins.has(sid) && host.originHandoff(sid)) host.TXCACHE.delete(sid);
    // A page that had no origin and now has one (its parent's spawn arrived) loads its transcript again: the first prompt it drew as
    // a message is the brief, which the intro now shows. A failed request is retried on the next poll, which backs off, up to
    // LATE_TRIES requests in all; after that the page stays as drawn (the brief shows twice until a reload) and polls as usual.
    if (host.LIVE.late !== ('id' in host.navigation.route ? host.navigation.route.id : '')) {
      if (host.LIVE.late) host.TXCACHE.delete(host.LIVE.late);
      host.LIVE.late = null;
    }
    if (
      host.navigation.route.v === 'session' &&
      !hadOrigin &&
      !!host.SESS['id' in host.navigation.route ? host.navigation.route.id : ''] &&
      !!host.originHandoff('id' in host.navigation.route ? host.navigation.route.id : '')
    ) {
      host.LIVE.late = 'id' in host.navigation.route ? host.navigation.route.id : '';
      host.LIVE.lateTries = 0;
      host.STALE_BRIEFS.add('id' in host.navigation.route ? host.navigation.route.id : '');
      host.TXCACHE.delete('id' in host.navigation.route ? host.navigation.route.id : '');
    }
    if (host.LIVE.late && !host.TX[host.LIVE.late]) host.LIVE.late = null; // nothing loaded to load again: the page loads it with its origin
    const changedH = new Set(
      host.H.filter((h) => oldH.get(h.id) !== host.handKey(h)).map((h) => h.id),
    );
    const newCards = host.cardKeys(),
      changedCards = new Set(
        [...newCards]
          .filter(([id, k]) => oldCards.has(id) && oldCards.get(id) !== k)
          .map(([id]) => id),
      );
    const view = host.viewed(),
      grown = new Set<string>(),
      cuts = new Map<string, number>(),
      patched = new Map<string, Entry[]>();
    let full = Object.values(host.SESS).some((x) => names.has(x.id) && names.get(x.id) !== x.name); // a new name shows in every turn
    for (const sid of Object.keys(host.TX)) {
      if (view.has(sid) && host.SESS[sid]) host.spread(sid);
      else host.dropTx(sid);
    }
    // Only transcripts whose mark in the model moved are asked for, one at a time.
    let chain = Promise.resolve();
    for (const sid of view)
      if (host.TX[sid] && host.LIVE.late === sid)
        chain = chain.then(() =>
          host.soft(
            reloadLate(sid).then(() => {
              grown.add(sid);
              full = true;
            }),
          ),
        );
      else if (
        host.TX[sid] &&
        host.TXM[sid].tok != null &&
        host.TOK[sid] != null &&
        host.shrank(host.TXM[sid].tok, host.TOK[sid])
      )
        chain = chain.then(() =>
          host.soft(
            reload(sid).then(() => {
              grown.add(sid);
              full = true;
            }),
          ),
        );
      else if (
        host.TX[sid] &&
        host.TXM[sid].to >= host.TXM[sid].total &&
        host.TXM[sid].tok !== host.TOK[sid]
      )
        chain = chain.then(() =>
          host.soft(
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
        host.TX[sid] &&
        host.TXM[sid].to < host.TXM[sid].total &&
        host.TXM[sid].watchTok !== host.TOK[sid]
      )
        chain = chain.then(() =>
          host.soft(
            watchLater(sid).then((r) => {
              if (r?.reload) full = true;
            }),
          ),
        );
    return chain.then(() => {
      host.LIVE.version = m.version;
      host.refresh(full ? null : dirtyTurns(cuts, grown, changedH, oldT, changedCards, patched));
      if (host.navigation.route.v === 'analytics') host.refreshAnalytics();
      const e = host.errorsLive();
      return e && host.soft(e);
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
      !host.TX['id' in host.navigation.route ? host.navigation.route.id : '']
    )
      return null;
    const sid = 'id' in host.navigation.route ? host.navigation.route.id : '',
      dirty = new Set<string>(),
      owner = new Map((host.TURNS[sid] ?? []).flatMap((t) => t.entries.map((e) => [e, t.id])));
    if (cuts.has(sid))
      for (const e of host.TX[sid].slice(cuts.get(sid))) {
        if (host.isGap(e)) return null;
        if (owner.has(e)) dirty.add(owner.get(e)!);
      }
    for (const e of patched?.get(sid) ?? []) {
      if (!owner.has(e)) return null;
      dirty.add(owner.get(e)!);
    }
    for (const t of host.TURNS[sid] ?? []) {
      if (
        oldT.get(t.id) !== host.LIVE.turns.get(t.id) ||
        [t.start, ...t.sent].some((h) => h && changedH.has(h.id)) ||
        t.entries.some((e) => e.k === 'h' && changedH.has(e.id))
      )
        dirty.add(t.id);
    }
    for (const h of host.H)
      if (h.kind === 'spawn' && h.from === sid && changedCards.has(h.id) && host.HOLDS.get(h.id))
        dirty.add(host.HOLDS.get(h.id)!.id);
    return dirty;
  }
  // A deep link keeps its contiguous window. Learn that its unseen tail grew without replacing the reader's page.
  function watchLater(sid: string) {
    const m = host.TXM[sid],
      tok = host.TOK[sid];
    return host.api('/api/tx?sid=' + host.enc(sid) + '&after=' + m.total).then((value) => {
      const p = parseTranscriptPage(value);
      if (host.TXM[sid] !== m) return;
      if (p.total < m.total) return reload(sid);
      m.newer = (m.newer ?? 0) + Math.max(0, p.total - m.total);
      Object.assign(m, { total: p.total, calls: p.calls, errors: p.errors, watchTok: tok });
    });
  }
  // The tail of a transcript loaded to its end: from its first foreground call still running, or the last turn's first call
  // with no result yet, since its result may have landed; else from the end. A file that shrank or was rewritten, or more
  // than five pages of new entries, loads the last page again instead.
  function tail(sid: string): Promise<TailResult> {
    const es = host.TX[sid],
      m = host.TXM[sid],
      tok = host.TOK[sid];
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
      host.api('/api/tx?sid=' + host.enc(sid) + '&after=' + after).then((value) => {
        const p = parseTranscriptPage(value);
        got = got.concat(p.entries.map((e) => host.txEntry({ ...e, sid })));
        last = p;
        if (p.to < p.total && p.entries.length && ++n < 5) return page(p.to);
      });
    return page(cut < es.length ? es[cut].slot! : m.to).then<TailResult>(() => {
      if (host.TX[sid] !== es) return { cut: null }; // "Load earlier" ran meanwhile: the next update catches up
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
      host.TX[sid] = es.slice(0, cut).concat(got);
      Object.assign(m, {
        to: last.to,
        total: last.total,
        calls: last.calls,
        errors: last.errors,
        tok,
      });
      host.spread(sid);
      return { cut, patched };
    });
  }
  // The page's own transcript loads its last page again; a child run's is dropped, and loads again as new child work.
  function reload(sid: string) {
    if (sid !== ('id' in host.navigation.route ? host.navigation.route.id : '')) {
      host.dropTx(sid);
      return Promise.resolve({ cut: null, reload: true });
    }
    return host.fetchTx(sid, '').then(() => ({ cut: null, reload: true }));
  }
  // The reload of a page whose origin arrived late (update): done once it loads; a failure is counted, makes the poll back off, and
  // after LATE_TRIES requests gives up.
  const LATE_TRIES = 4;
  function reloadLate(sid: string) {
    return reload(sid).then(
      (r) => {
        if (host.LIVE.late === sid) host.LIVE.late = null;
        return r;
      },
      (err) => {
        if (host.LIVE.late === sid) {
          if (++host.LIVE.lateTries >= LATE_TRIES) {
            host.LIVE.late = null;
            console.warn(
              'semon: gave up reloading the transcript of ' + sid + ' after its origin arrived',
            );
          } else host.LIVE.retry = true;
        }
        throw err;
      },
    );
  }

  return { reload, tail, applyModelDelta, update };
}
