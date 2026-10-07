import type { createLocalControl } from './localControl';
import { parseModel } from '../lib/model';
import type { ViewerModelStore } from '../state/model';
import type { TranscriptStore } from '../state/transcript';
import type { ViewerHost } from '../viewer-host';
import type { Account } from '../lib/account';
import type { EffectScope } from '../app/effects';
import type { createDomain } from '../domain/calculations';
import type { createAnalytics } from './analytics';
import type { Entry } from '../domain/types';
import { dictionary, object, text } from '../domain/validate';
import { parseAccount, requestJson } from '../lib';
import type { ApplicationRoute } from '../navigation/routes';
import type { PageDirection } from '../state/transcript';
interface TransportHost {
  disposed: boolean;
  controlOwner: Pick<ReturnType<typeof createLocalControl>, 'prepare' | 'adopt'>;
  scope: EffectScope;
  now: number;
  admin: { href: string; label: string } | null;
  account: Account | null;
  viewerHost: ViewerHost | null;
  machinesPath: string | null;
  clock: (t: number) => string;

  transcripts: Pick<TranscriptStore, 'entries' | 'marks' | 'spread' | 'fetch'>;

  analytics: Pick<ReturnType<typeof createAnalytics>, 'fetchAnalytics' | 'scheduleAnalytics'>;

  domain: Pick<ReturnType<typeof createDomain>, 'nameOf'>;

  modelStore: Pick<ViewerModelStore, 'sessions' | 'turn' | 'adopt'>;
}
/** Owns transport behavior through explicit application ports. */
export function createTransport(host: TransportHost) {
  let serverNow = 0,
    fetchedAt = 0;
  let TOK: Record<string, string> = {}; // per session: its transcript's growth mark in the model; a loaded transcript is tailed only when it moved
  const enc = encodeURIComponent;
  const safePath = (href: unknown) =>
    typeof href === 'string' &&
    href.startsWith('/') &&
    !href.startsWith('//') &&
    !href.includes('\\') &&
    !/[\u0000-\u001f\u007f-\u009f]/.test(href) &&
    href.length <= 512;
  const accountOf = parseAccount;
  // The embedding page's menu, `window.semonEmbed.account`, validated like a server's. A rejected one is reported once on
  // the console, since the embedding page gets no other sign of it.
  let embedWarned = false;
  function embeddedAccount() {
    let source: unknown;
    try {
      const config: unknown = Reflect.get(window, 'semonEmbed');
      source =
        config && typeof config === 'object' && 'account' in config ? config.account : undefined;
    } catch {
      source = undefined;
    }
    const account = accountOf(source);
    if (source != null && !account && !embedWarned) {
      embedWarned = true;
      console.warn('semon: window.semonEmbed.account was rejected (see the account menu rules)');
    }
    return account;
  }
  // An error carries the HTTP status (0: no response), so live polling can tell a 403 from a dropped connection.
  async function api(path: string, signal?: AbortSignal, unchanged = false): Promise<unknown> {
    if (host.disposed) throw new DOMException('Viewer destroyed', 'AbortError');
    const controller = host.scope.request(),
      release = () => host.scope.releaseRequest(controller);
    const deadline = host.scope.timeout(() => controller.abort(), 15000);
    try {
      const response = await requestJson(
        path,
        signal ? AbortSignal.any([signal, controller.signal]) : controller.signal,
        unchanged,
      );
      if (host.disposed) throw new DOMException('Viewer destroyed', 'AbortError');
      return response;
    } finally {
      host.scope.clearTimeout(deadline);
      release();
    }
  }
  // now follows the client clock from the model's `now`, so every "ago" keeps moving; a running tool's age follows now.
  function tick() {
    host.now = serverNow + (Date.now() - fetchedAt);
    for (const s of Object.values(host.modelStore.sessions))
      if (s.activity && s.activity[3] != null)
        s.activity[2] = Math.floor((host.now - s.activity[3]) / 1000);
  }
  function adopt(value: unknown) {
    if (host.disposed) throw new DOMException('Viewer destroyed', 'AbortError');
    // Prepare every auxiliary field before the model owner changes its raw baseline.
    const m = parseModel(value);
    const control = host.controlOwner.prepare(m.control);
    const marks = m.tx == null ? {} : dictionary(m.tx, text);
    const admin = m.admin == null ? null : object(m.admin);
    const nextAdmin =
      admin && typeof admin.href === 'string' && safePath(admin.href)
        ? { href: admin.href, label: text(admin.label) }
        : null;
    const account =
      accountOf(m.account) ?? accountOf(host.viewerHost?.account) ?? embeddedAccount();
    const nav = m.nav == null ? null : object(m.nav);
    const machinesPath =
      host.viewerHost?.machinesPath ??
      (nav && typeof nav.machines === 'string' && safePath(nav.machines) ? nav.machines : null);
    host.modelStore.adopt(m, () => {
      if (!(host.viewerHost?.controlStream || host.viewerHost?.catalogControlStream))
        host.controlOwner.adopt(control);
      serverNow = m.now;
      fetchedAt = Date.now();
      TOK = marks;
      host.transcripts.marks = marks;
      host.admin = nextAdmin;
      host.account = account;
      host.machinesPath = machinesPath;
      tick();
      for (const sid of Object.keys(host.transcripts.entries)) host.transcripts.spread(sid);
    });
    host.viewerHost?.modelAccount?.(account);
    host.viewerHost?.modelNavigation?.(m);
    return m;
  }
  // Each loaded entry goes to its turn: an entry that starts a turn (or a page) names it. Its key, its turn and place in
  // it, stays the same while the transcript only grows: live updates find what was open and where the reader was by it.
  const spread = (sid: string) => host.transcripts.spread(sid);
  // A return line arrives as data; it reads as the mockup's "Returned to … · HH:MM".
  const txEntry = (e: Entry): Entry =>
    e.k === 'end' && e.ret
      ? {
          k: 'end',
          text:
            'Returned to ' +
            host.domain.nameOf(e.ret.to) +
            (e.ret.failed ? ' · failed' : '') +
            (e.ret.at != null ? ' · ' + host.clock(e.ret.at) : ''),
          turn: e.turn,
        }
      : e;
  // where: "before" and "after" extend the loaded range; otherwise the page replaces it.
  const fetchTx = (
    sid: string,
    q: string = '',
    where: PageDirection | undefined = undefined,
    signal: AbortSignal | undefined = undefined,
    onPage: (() => void) | undefined = undefined,
  ) => host.transcripts.fetch(sid, q, where, signal, onPage);
  // What a route needs before it can draw: a session's page (the one holding a deep-linked turn).
  // `signal` cancels what a navigation asked for when the reader goes elsewhere first.
  function load(r: ApplicationRoute, signal: AbortSignal | undefined = undefined) {
    if (r.v === 'analytics')
      return host.analytics.fetchAnalytics().then(() => {
        host.analytics.scheduleAnalytics();
      }); // the range's answer, from the server
    if (r.v !== 'session' || !host.modelStore.sessions[r.id]) return null;
    const t = r.turn ? host.modelStore.turn.get(r.turn) : null,
      deep = t && t.sid === r.id && !t.entries.length;
    if (host.transcripts.entries[r.id] && !deep) return null;
    return fetchTx(r.id, deep ? 'turn=' + enc(t.id) : '', undefined, signal);
  }
  // The last few transcripts opened, kept when the reader leaves them, so opening one again draws it at once. (A transcript
  // still in TX, which only a model update prunes, draws from there just the same.) A transcript is kept only when it was
  // loaded to its end, and the cache is bounded by entries and by estimated memory: two bytes for each character of an entry's
  // text, since JavaScript strings are UTF-16. Opening one takes it out of the cache; leaving it puts it back at the newest end.

  return {
    api,
    txEntry,
    get TOK() {
      return TOK;
    },
    set TOK(value: Record<string, string>) {
      TOK = value;
    },
    fetchTx,
    enc,
    adopt,
    load,
    spread,
    tick,
    get fetchedAt() {
      return fetchedAt;
    },
    set fetchedAt(value: number) {
      fetchedAt = value;
    },
    accountOf,
  };
}
