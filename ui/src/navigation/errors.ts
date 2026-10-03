import { createLiveRegion, requestJson } from '../lib';
import { EffectScope } from '../app/effects';
import type { ApplicationRoute } from './routes';
import type { ScrollSnapshot } from './scroll';
import type { Entry, Session, TranscriptMeta } from '../domain/types';
interface ErrorState {
  mode: 'errors' | 'signals';
  on: boolean;
  sid: string | null;
  slots: number[];
  listed: boolean;
  count: number;
  version: string | null;
  k: number;
  slot: number | null;
  saved: ScrollSnapshot | null;
  range: { tx: Entry[]; m: TranscriptMeta } | null;
  tools: boolean;
  chain: Promise<void | null>;
  gen: number;
  notice?: string | null;
}
export interface ErrorNavigationHost {
  navigation: { route: ApplicationRoute };
  TX: Record<string, Entry[]>;
  TXM: Record<string, TranscriptMeta>;
  TOK: Record<string, string>;
  SESS: Record<string, Session>;
  show: { tools: boolean };
  sidebarOnly: boolean;
  page(): HTMLElement;
  drawSessionBar(): void;
  render(): void;
  keepFocus(fn: () => void): void;
  countOf(session: Session | undefined, field: 'errors'): number | undefined;
  capture(): ScrollSnapshot;
  restore(saved: ScrollSnapshot): void;
  opener(node: HTMLElement): HTMLElement | null;
  resetPagerInput(): void;
  stopOpeningEndPin(): void;
  clearFind(): void;
  centre(node: HTMLElement): void;
  fetchTx(
    sid: string,
    query: string,
    direction?: 'before' | 'after',
    signal?: AbortSignal,
  ): Promise<unknown>;
  dropTx(sid: string): void;
  spread(sid: string): void;
  tail(sid: string): Promise<unknown>;
}
export function createErrorNavigation(host: ErrorNavigationHost) {
  const {
    navigation,
    TX,
    TXM,
    SESS,
    drawSessionBar,
    render,
    keepFocus,
    countOf,
    capture,
    restore,
    opener,
    resetPagerInput,
    stopOpeningEndPin,
    centre,
    fetchTx,
    dropTx,
    spread,
    tail,
  } = host;
  const scope = new EffectScope();
  let pending: AbortController | null = null,
    modeRequest: AbortController | null = null;
  const $ = <T extends HTMLElement = HTMLElement>(selector: string): T | null =>
    host.page().ownerDocument.querySelector<T>(selector);
  const enc = encodeURIComponent,
    SIDEBAR_ONLY = host.sidebarOnly;
  const ERR: ErrorState = {
    mode: 'errors',
    on: false,
    sid: null,
    slots: [],
    listed: false,
    count: 0,
    version: null,
    k: -1,
    slot: null,
    saved: null,
    range: null,
    tools: true,
    chain: Promise.resolve(),
    gen: 0,
  };
  const ERR_NEAR = 400,
    ERR_AROUND = 40; // slots: a page (at most 200 entries) or two away is added; 40 entries of context above
  const errLive = createLiveRegion();
  if (!SIDEBAR_ONLY) document.body.append(errLive);
  const signalCount = (s: Session | undefined) =>
    Object.values(s?.signals ?? {}).reduce((n, x) => n + x, 0);
  const errText = () =>
    ERR.k < 0
      ? ERR.count
        ? 'Finding ' + ERR.mode + '…'
        : 'No ' + ERR.mode
      : (ERR.mode === 'signals' ? 'Signal ' : 'Error ') + (ERR.k + 1) + ' of ' + ERR.count;
  const errOn = (sid: string) => ERR.on && ERR.sid === sid;
  // The label and buttons in place, so focus stays where it is.
  function errLabel(announce: boolean) {
    ERR.notice = null;
    if (navigation.route.v === 'session' && ERR.on) drawSessionBar();
    if (announce) errLive.textContent = errText();
  }
  function releaseMode() {
    if (modeRequest) {
      modeRequest.abort();
      scope.releaseRequest(modeRequest);
      modeRequest = null;
    }
  }
  function releasePending() {
    if (pending) {
      pending.abort();
      scope.releaseRequest(pending);
      pending = null;
    }
  }
  function openErrors(sid: string, mode: ErrorState['mode'] = 'errors') {
    if (ERR.on || navigation.route.v !== 'session' || navigation.route.id !== sid || !TXM[sid])
      return;
    releaseMode();
    modeRequest = scope.request();
    resetPagerInput();
    stopOpeningEndPin();
    host.clearFind();
    Object.assign(ERR, {
      mode,
      on: true,
      sid,
      slots: [],
      listed: false,
      count: mode === 'signals' ? signalCount(SESS[sid]) : (countOf(SESS[sid], 'errors') ?? 0),
      version: null,
      k: -1,
      slot: null,
      saved: capture(),
      range: { tx: TX[sid], m: { ...TXM[sid] } },
      tools: host.show.tools,
      gen: ERR.gen + 1,
    });
    if (!host.show.tools) {
      host.show.tools = true;
      render();
    } else drawSessionBar();
    document.getElementById('err-next')?.focus({ preventScroll: true });
    errLabel(true);
    const gen = ERR.gen;
    fetchErrors(sid).then(
      () => {
        if (ERR.gen !== gen || !ERR.on) return;
        if (ERR.slots.length) {
          ERR.k = 0;
          showError(true);
        } else errLabel(true);
      },
      () => {
        if (ERR.gen === gen && ERR.on) {
          errLive.textContent = "Couldn't list " + ERR.mode;
          ERR.notice = "Couldn't list " + ERR.mode;
          drawSessionBar();
        }
      },
    );
  }
  // The list, or nothing new (304) when the model hasn't moved since it was fetched.
  function fetchErrors(sid: string) {
    releasePending();
    const request = scope.request();
    pending = request;
    const gen = ERR.gen,
      mode = ERR.mode;
    return requestJson(
      '/api/tx?sid=' +
        enc(sid) +
        '&' +
        mode +
        '=1' +
        (ERR.version ? '&since=' + enc(ERR.version) : ''),
      request.signal,
      true,
    )
      .then((x) => {
        if (!x || request.signal.aborted || ERR.gen !== gen || !errOn(sid) || typeof x !== 'object')
          return;
        const slots =
          'slots' in x && Array.isArray(x.slots)
            ? x.slots.filter(
                (value): value is number =>
                  typeof value === 'number' && Number.isInteger(value) && value >= 0,
              )
            : [];
        const count =
          mode === 'errors'
            ? 'errors' in x
              ? x.errors
              : undefined
            : 'signals' in x
              ? x.signals
              : undefined;
        ERR.listed = true;
        ERR.slots = slots;
        ERR.count =
          typeof count === 'number' && Number.isInteger(count)
            ? Math.max(count, slots.length)
            : slots.length;
        ERR.version = 'version' in x && typeof x.version === 'string' ? x.version : null;
        // The current step stays current wherever it now is in the list; one no longer failed gives way to the next after it.
        if (ERR.slot != null) {
          const currentSlot = ERR.slot,
            at = ERR.slots.indexOf(currentSlot),
            after = ERR.slots.findIndex((slot) => slot > currentSlot);
          ERR.k = at >= 0 ? at : !ERR.slots.length ? -1 : after >= 0 ? after : ERR.slots.length - 1;
          if (at < 0) ERR.slot = ERR.k >= 0 ? ERR.slots[ERR.k] : null;
        }
      })
      .catch((error) => {
        if (!request.signal.aborted && pending === request && ERR.gen === gen && errOn(sid))
          throw error;
      })
      .finally(() => {
        scope.releaseRequest(request);
        if (pending === request) pending = null;
      });
  }
  function stepErrors(delta: number) {
    if (!ERR.on || !ERR.slots.length) return;
    const n = ERR.slots.length;
    ERR.k = ERR.k < 0 ? 0 : (((ERR.k + delta) % n) + n) % n;
    showError(true);
  }
  const hasSlot = (sid: string, slot: number) => {
    const m = TXM[sid];
    return !!m && slot >= m.from && slot < m.to;
  };
  // Loads the page holding `slot` when it isn't loaded: resolves true when the loaded range changed.
  function loadSlot(sid: string, slot: number) {
    if (hasSlot(sid, slot)) return Promise.resolve(false);
    const m = TXM[sid];
    if (!m || modeRequest?.signal.aborted) return Promise.resolve(false);
    const up = slot < m.from,
      near = up ? m.from - slot <= ERR_NEAR : slot - m.to < ERR_NEAR;
    let tries = 0;
    const extend = (): Promise<unknown> | null =>
      modeRequest?.signal.aborted || hasSlot(sid, slot) || tries++ >= 3
        ? null
        : fetchTx(
            sid,
            up ? 'before=' + TXM[sid].from : 'after=' + TXM[sid].to,
            up ? 'before' : 'after',
            modeRequest?.signal,
          ).then(extend);
    const around = () =>
      modeRequest?.signal.aborted || hasSlot(sid, slot)
        ? null
        : fetchTx(
            sid,
            'after=' + Math.max(0, slot - ERR_AROUND),
            undefined,
            modeRequest?.signal,
          ).then(() =>
            hasSlot(sid, slot)
              ? null
              : fetchTx(sid, 'after=' + slot, undefined, modeRequest?.signal),
          );
    return Promise.resolve(near ? extend() : null)
      .then(around)
      .then(() => true);
  }
  // The session's own step for a slot: not one in a child run's work drawn inside it.
  function errNode(sid: string, slot: number) {
    const e = (TX[sid] ?? []).find(
      (x) => x.k === (ERR.mode === 'signals' ? 'signal' : 'tool') && x.slot === slot,
    );
    if (!e?.key) return null;
    return (
      [...host.page().querySelectorAll<HTMLElement>('.turns [data-e]')].find(
        (n) => n.dataset.e === e.key,
      ) ?? null
    );
  }
  // Marks the current step (its group opened so it shows); with `ring`, rings it for a moment and centres it under the bar.
  function markError(ring: boolean) {
    for (const n of host.page().querySelectorAll<HTMLElement>('[data-e].err-current'))
      n.classList.remove('err-current', 'err-ring');
    if (
      !ERR.on ||
      ERR.slot == null ||
      navigation.route.v !== 'session' ||
      navigation.route.id !== ERR.sid
    )
      return null;
    const node = errNode(navigation.route.id!, ERR.slot);
    if (!node) return null;
    const g = node.closest<HTMLElement>('.tgroup'),
      sum = g && opener(g);
    if (sum?.getAttribute('aria-expanded') === 'false') sum.click();
    node.classList.add('err-current');
    if (ring) {
      node.classList.remove('err-ring');
      void node.offsetWidth;
      node.classList.add('err-ring');
      scope.timeout(() => node.classList.remove('err-ring'), 1500);
    }
    return node;
  }
  function showError(announce: boolean) {
    resetPagerInput();
    const sid = ERR.sid;
    if (!sid) return;
    const slot = ERR.slots[ERR.k],
      gen = ERR.gen;
    ERR.slot = slot;
    errLabel(announce);
    ERR.chain = ERR.chain
      .then(() => {
        if (!ERR.on || ERR.gen !== gen || ERR.slot !== slot) return null; // a later step or a close since
        stopOpeningEndPin();
        return loadSlot(sid, slot).then(
          (moved) => {
            if (
              !ERR.on ||
              ERR.gen !== gen ||
              ERR.slot !== slot ||
              navigation.route.v !== 'session' ||
              navigation.route.id !== sid
            )
              return;
            if (moved) keepFocus(render); // render marks the current step again
            const node = markError(true);
            if (!node) {
              if (announce) errLive.textContent = errText() + ', not shown in this transcript';
              return;
            }
            centre(node);
            scope.frame(() =>
              scope.frame(() => {
                if (ERR.on && ERR.slot === slot && node.isConnected) centre(node);
              }),
            );
          },
          () => {
            if (ERR.on && ERR.gen === gen) errLive.textContent = "Couldn't load " + errText();
          },
        );
      })
      .catch((e) => {
        scope.timeout(() => {
          throw e;
        });
      }); // a fault on the page, reported as one; the next step still runs
  }
  // Leaves the mode. By navigation (`away`), a range the mode moved is dropped, so the next visit loads the end afresh and
  // is tailed again; closeErrors puts the range from before back instead.
  function dropErrors(away = false) {
    if (!ERR.on) return;
    const sid = ERR.sid;
    if (!sid) return;
    const range = ERR.range,
      m = TXM[sid];
    releasePending();
    releaseMode();
    ERR.on = false;
    ERR.gen++;
    host.show.tools = ERR.tools;
    ERR.saved = ERR.range = null;
    errLive.textContent = '';
    if (away && range && m && (m.from !== range.m.from || m.to < range.m.to)) dropTx(sid);
  }
  function closeErrors() {
    if (!ERR.on) return;
    const sid = ERR.sid;
    if (!sid) return;
    const saved = ERR.saved,
      range = ERR.range,
      m = TXM[sid];
    dropErrors();
    const generation = ERR.gen;
    let p = Promise.resolve<unknown>(undefined);
    // Pages loaded above the range (or a range replaced by a page further off) move its entries' keys: the range from before
    // comes back, caught up with the tail when it reached the end and the session grew since.
    if (range && m && TX[sid] && (m.from !== range.m.from || m.to < range.m.to)) {
      TX[sid] = range.tx;
      TXM[sid] = range.m;
      spread(sid);
      if (range.m.to >= range.m.total && range.m.tok !== host.TOK[sid])
        p = tail(sid).catch(() => null);
    }
    p.then(() => {
      if (
        navigation.route.v !== 'session' ||
        navigation.route.id !== sid ||
        ERR.on ||
        ERR.gen !== generation
      )
        return;
      render();
      if (saved) restore(saved);
      const b0 = $('#topbar .lab-errs') ?? $('#topbar .chip[data-filter="failures"]'),
        b = b0 && !b0.getClientRects().length ? $('#more-btn') : b0;
      /* on a phone the line is not drawn: focus goes to ⋯ */ if (
        b &&
        !b.hidden &&
        document.activeElement !== b &&
        (!document.activeElement ||
          document.activeElement === document.body ||
          !document.activeElement.isConnected)
      )
        b.focus({ preventScroll: true });
    });
  }
  // Keys while the mode is on: n and p (and Enter, Shift+Enter in the bar) step, Escape closes. Not while typing, and not
  // under an open sheet.
  if (!SIDEBAR_ONLY)
    scope.listen(document, 'keydown', (e) => {
      if (
        !ERR.on ||
        e.defaultPrevented ||
        e.altKey ||
        e.ctrlKey ||
        e.metaKey ||
        document.querySelector('dialog[open]')
      )
        return;
      if (!(e.target instanceof HTMLElement)) return;
      // The drawer and an open menu have the keys first: Escape closes them and leaves the mode on.
      if (document.body.classList.contains('drawer-open') || document.querySelector('.menu'))
        return;
      if (e.target.closest?.("input, textarea, select, [contenteditable='true']")) return;
      const inBar = !!e.target.closest?.('#topbar .errnav-bar'),
        onButton = e.target.tagName === 'BUTTON';
      if (e.key === 'Escape') {
        e.preventDefault();
        closeErrors();
        return;
      }
      if (e.key === 'n' || e.key === 'N') {
        e.preventDefault();
        stepErrors(1);
        return;
      }
      if (e.key === 'p' || e.key === 'P') {
        e.preventDefault();
        stepErrors(-1);
        return;
      }
      if (e.key === 'Enter' && (inBar || e.target === document.body)) {
        if (e.shiftKey) {
          e.preventDefault();
          stepErrors(-1);
        } else if (!onButton) {
          e.preventDefault();
          stepErrors(1);
        }
      }
    });
  // Live: a new model lists the errors again; N grows, the current one stays.
  function errorsLive() {
    if (!ERR.on || navigation.route.v !== 'session' || navigation.route.id !== ERR.sid) return null;
    const sid = ERR.sid;
    if (!sid) return null;
    const gen = ERR.gen,
      was = ERR.count;
    return fetchErrors(sid).then(() => {
      if (!ERR.on || ERR.gen !== gen) return;
      if (ERR.k < 0 && ERR.slots.length) {
        ERR.k = 0;
        showError(true);
        return;
      }
      errLabel(ERR.count !== was);
      markError(false);
    });
  }
  return {
    state: ERR,
    text: errText,
    on: errOn,
    open: openErrors,
    step: stepErrors,
    mark: markError,
    drop: dropErrors,
    close: closeErrors,
    live: errorsLive,
    destroy() {
      releasePending();
      releaseMode();
      scope.destroy();
      errLive.remove();
      ERR.on = false;
      ERR.gen++;
    },
  };
}
