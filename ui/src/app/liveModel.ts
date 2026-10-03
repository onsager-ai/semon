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
  const liveController = createLiveController({
    async poll() {
      const response = await host.transportOwner.api(
        '/api/model?delta=1&since=' +
          host.transportOwner.enc(LIVE.late ? '' : (LIVE.version ?? '')),
        undefined,
        true,
      );
      if (!response) return;
      let model;
      try {
        model = host.liveUpdates.applyModelDelta(response);
      } catch {
        await host.transportOwner
          .api('/api/model?delta=1')
          .then((...args: Parameters<typeof host.liveUpdates.update>) =>
            host.liveUpdates.update(...args),
          );
        return;
      }
      await host.liveUpdates.update(model);
    },
    failed(error) {
      return !!host.viewerHost?.modelFailed?.(error instanceof ApiError ? error.status : 0);
    },
    ended,
  });
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
  const schedule = (ms: number) => liveController.schedule(ms);
  // An embedding page can cancel `semon:ended` to draw its own note in place of this one.
  function ended(status: number) {
    if (host.disposed) return;
    liveController.stop();
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
  // A 403 or a dropped connection fails the update (and backs off); anything else skips that one transcript.
  const soft = <T>(p: Promise<T>) =>
    p.catch((e: unknown) => {
      if (e instanceof ApiError && (e.status === 403 || e.status === 0)) throw e;
    });
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
