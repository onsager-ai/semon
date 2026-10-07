import type { ViewerModelStore } from '../state/model';
import type { NavigationController } from '../navigation/routes';
import type { ViewerHost } from '../viewer-host';
import type { createTransport } from './transport';
import type { createLiveUpdates } from './liveUpdates';
import type { createDomain } from '../domain/calculations';
import type { Handoff, Turn } from '../domain/types';
import type { ModelWire } from '../lib';
import { createLiveController, createStatusNote } from '../lib';
import { ApiError } from '../lib/model';
interface LiveModelHost {
  viewerHost: ViewerHost | null;
  disposed: boolean;
  $: <T extends HTMLElement = HTMLElement>(s: string, r?: ParentNode) => T;
  sidebarOnly: boolean;
  navigation: NavigationController;

  domain: Pick<ReturnType<typeof createDomain>, 'countOf'>;

  modelStore: Pick<ViewerModelStore, 'turns' | 'handoffs' | 'sessions'>;

  liveUpdates: Pick<ReturnType<typeof createLiveUpdates>, 'applyModelDelta' | 'update'>;

  transportOwner: Pick<ReturnType<typeof createTransport>, 'api' | 'enc'>;
}
/** Owns liveModel behavior through explicit application ports. */
export function createLiveModel(host: LiveModelHost) {
  let streamSequence = 0;
  let activeRead: AbortController | null = null;
  const liveController = createLiveController({
    async poll() {
      // Recovery and SSE delivery share the same update transaction queue.
      const recovering = liveController.recovering;
      const work = delivery.then(async () => {
        if (destroyed || host.disposed) return;
        const sequence = streamSequence;
        const request = new AbortController();
        activeRead = request;
        const response = await host.transportOwner.api(
          recovering
            ? '/api/model?delta=1'
            : '/api/model?delta=1&since=' +
                host.transportOwner.enc(LIVE.late ? '' : (LIVE.version ?? '')),
          request.signal,
          !recovering,
        );
        if (!response || destroyed || host.disposed || request.signal.aborted) return;
        let model;
        try {
          model = host.liveUpdates.applyModelDelta(response);
        } catch {
          model = await host.transportOwner.api('/api/model?delta=1', request.signal);
          if (destroyed || host.disposed || request.signal.aborted) return;
        }
        await host.liveUpdates.update(model, recovering);
        if (activeRead === request) activeRead = null;
        if (recovering && !destroyed && !host.disposed) {
          if (sequence === streamSequence) {
            liveController.synchronized();
            clearStreamNote();
          } else LIVE.again = true;
        }
      });
      delivery = work.catch(() => {});
      try {
        await work;
      } catch (error) {
        showStreamNote();
        liveController.recover();
        throw error;
      }
    },
    failed,
    ended,
  });
  function failed(error: unknown) {
    const handled = !!host.viewerHost?.modelFailed?.(error instanceof ApiError ? error.status : 0);
    if (handled) {
      liveController.stop();
      destroyed = true;
      stream?.close();
      stream = null;
      clearStreamNote();
    }
    return handled;
  }
  const LIVE = liveController.state;
  // The turn index as last drawn, to tell which turns an update changed.
  const remember = (_m: ModelWire) => {
    LIVE.turns = new Map(
      Object.values(host.modelStore.turns)
        .flat()
        .map((x) => [x.id, turnKey(x)]),
    );
  };
  // What an update can change in a turn record or a handoff, cheaply (not the text, which a record never rewrites).
  const turnKey = (x: Turn) =>
    [
      x.start?.id,
      x.end?.st,
      x.end?.why,
      x.end?.h,
      x.sent.map((h) => h.id).join(','),
      x.last ? 1 : 0,
    ].join('|');
  const handKey = (h: Handoff) =>
    [
      h.status,
      h.to,
      h.done,
      h.result?.length,
      h.kind === 'toyou' ? h.answer?.length : undefined,
      h.kind === 'toyou' ? h.answers?.length : undefined,
      h.declined ? 1 : 0,
    ].join('|');
  // rendered route is owned by navigation; // the route the page shows
  const visible = () => document.visibilityState === 'visible';
  let stream: EventSource | null = null;
  let destroyed = false;
  let streamNote: HTMLElement | null = null;
  function clearStreamNote() {
    streamNote?.remove();
    streamNote = null;
  }
  function showStreamNote() {
    if (destroyed || host.disposed || streamNote) return;
    streamNote = createStatusNote('Reconnecting… Your conversation is retained.', 'livenote');
    document.body.append(streamNote);
  }
  let delivery = Promise.resolve();
  const schedule = (ms: number) => {
    const path = host.viewerHost?.modelStream;
    if (!path || typeof EventSource === 'undefined') {
      liveController.schedule(ms);
      return;
    }
    if (stream || destroyed) return;
    if (!path.startsWith('/') || path.startsWith('//') || /[\\\s]/.test(path))
      throw new Error('Invalid live model stream');
    stream = new EventSource(path);
    liveController.useStream();
    stream.addEventListener('open', () => {
      if (streamNote) liveController.recover();
    });
    const recover = () => {
      showStreamNote();
      liveController.recover();
    };
    stream.addEventListener('error', recover);
    stream.addEventListener('unavailable', recover);
    stream.addEventListener('model', (event) => {
      if (!(event instanceof MessageEvent) || destroyed || host.disposed) return;
      streamSequence++;
      if (liveController.recovering) return;
      // Serialize deliveries through the existing model/transcript owner; a
      // stream never introduces a second cache, router or rendering pipeline.
      delivery = delivery
        .then(async () => {
          if (destroyed || host.disposed || liveController.recovering) return;
          await host.liveUpdates.update(JSON.parse(String(event.data)));
          clearStreamNote();
        })
        .catch((error: unknown) => {
          if (failed(error)) return;
          if (error instanceof ApiError && error.status === 403) ended(403);
          else recover();
        });
    });
    stream.addEventListener('ended', () => {
      activeRead?.abort();
      destroyed = true;
      liveController.stop();
      stream?.close();
      stream = null;
      clearStreamNote();
      host.viewerHost?.modelFailed?.(403);
    });
  };
  // An embedding page can cancel `semon:ended` to draw its own note in place of this one.
  function ended(status: number) {
    if (host.disposed) return;
    activeRead?.abort();
    liveController.stop();
    destroyed = true;
    stream?.close();
    stream = null;
    clearStreamNote();
    if (host.$('.livenote, .livenote-side')) return;
    if (
      !window.dispatchEvent(
        new CustomEvent('semon:ended', { cancelable: true, detail: { status } }),
      )
    )
      return;
    const n = createStatusNote(
      host.sidebarOnly
        ? 'Sessions stopped updating: reload the page'
        : 'Session ended: reload with the printed URL',
      host.sidebarOnly ? 'ghead livenote-side' : 'livenote',
    );
    if (host.sidebarOnly) host.$('#lanes').after(n);
    else document.body.append(n); // on an embedding page, under the list that stopped
  }
  // Required reads participate in synchronization; a failure must reach the
  // single recovery owner rather than falsely advancing freshness.
  const soft = <T>(p: Promise<T>) => p;
  // The transcript on screen: a session page's own. Any other loaded transcript is dropped from TX (the last few opened are kept in
  // TXCACHE, and brought up to date when opened again). A child run's card is drawn from the model, so its transcript is not loaded.
  const viewed = () => {
    const v = new Set<string>();
    if (host.navigation.route.v === 'session')
      v.add('id' in host.navigation.route ? host.navigation.route.id : '');
    return v;
  };
  // What a child card shows, so an update knows which cards changed: the run's name, state, kind, model, steps and current call.
  const cardKeys = () =>
    new Map(
      host.modelStore.handoffs
        .filter((h) => h.kind === 'spawn' && host.modelStore.sessions[h.to])
        .map((h) => {
          const c = host.modelStore.sessions[h.to];
          return [
            h.id,
            [
              c.name,
              c.state,
              c.kind,
              c.model,
              host.domain.countOf(c, 'calls'),
              c.activity?.join('|'),
              h.status,
              h.result,
            ].join('\u0001'),
          ];
        }),
    );

  return {
    destroy() {
      activeRead?.abort();
      activeRead = null;
      destroyed = true;
      clearStreamNote();
      stream?.close();
      stream = null;
      liveController.destroy();
    },
    liveController,
    LIVE,
    remember,
    schedule,
    visible,
    ended,
    handKey,
    cardKeys,
    viewed,
    soft,
  };
}
