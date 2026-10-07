import type { EffectScope } from './effects';
import type { NavigationController } from '../navigation/routes';
import type { ViewerModelStore } from '../state/model';
import type { ViewUpdates } from '../state/viewUpdates';
import type { ViewerHost } from '../viewer-host';
import type { createLocalControl } from './localControl';
import type { CatalogSessionIdentity } from '../domain/catalog';
interface ControlObservationHost {
  scope: EffectScope;
  navigation: NavigationController;
  modelStore: Pick<ViewerModelStore, 'sessions'>;
  updates: ViewUpdates;
  viewerHost: ViewerHost | null;
  catalogSelection?: {
    selectedIdentity(): CatalogSessionIdentity | null;
    subscribe(listener: () => void): () => void;
  };
  controlOwner: Pick<ReturnType<typeof createLocalControl>, 'prepare' | 'observe' | 'unavailable'>;
}
/** Navigation-owned native status; never adopts or rebuilds a content model. */
export function createControlObservation(host: ControlObservationHost) {
  if (!host.viewerHost?.controlStream && !host.viewerHost?.catalogControlStream)
    return { destroy() {} };
  let stream: EventSource | null = null;
  let identity = '';
  let timer: number | undefined;
  let deadline: number | undefined;
  let delay = 1000;
  let disposed = false;
  function selected(): {
    path: string | undefined;
    params: Record<string, string>;
    thread: string;
  } {
    const route = host.navigation.route;
    if (host.catalogSelection) {
      const selected = host.catalogSelection.selectedIdentity();
      const accepted =
        route.v === 'session' && selected?.catalog_key === route.id ? selected : null;
      return {
        path: accepted?.native_id ? host.viewerHost?.catalogControlStream : undefined,
        params: accepted
          ? {
              source_key: accepted.source_key,
              catalog_key: accepted.catalog_key,
              native_id: accepted.native_id ?? '',
              harness: accepted.harness,
            }
          : {},
        thread: accepted?.native_id ?? '',
      };
    }
    const session = route.v === 'session' ? host.modelStore.sessions[route.id] : undefined;
    const thread = session && route.v === 'session' ? route.id : '';
    return {
      path: host.viewerHost?.controlStream,
      params: { selected: thread, machine: session?.machine ?? '' },
      thread,
    };
  }
  function cancel() {
    stream?.close();
    stream = null;
    host.scope.clearTimeout(timer);
    host.scope.clearTimeout(deadline);
  }
  function connect() {
    if (disposed) return;
    cancel();
    const scope = selected();
    identity = JSON.stringify(scope);
    const path = scope.path;
    if (!path || typeof EventSource === 'undefined') return;
    if (!path.startsWith('/') || path.startsWith('//') || /[\\\s]/.test(path))
      throw new Error('Invalid control stream');
    const current = new EventSource(
      path + (path.includes('?') ? '&' : '?') + new URLSearchParams(scope.params),
    );
    stream = current;
    let revision = '';
    const valid = () => !disposed && stream === current && identity === JSON.stringify(selected());
    const recover = () => {
      if (!valid()) return;
      cancel();
      host.controlOwner.unavailable();
      timer = host.scope.timeout(connect, delay);
      delay = Math.min(15000, delay * 2);
    };
    const heartbeat = () => {
      if (!valid()) return;
      host.scope.clearTimeout(deadline);
      deadline = host.scope.timeout(recover, 30000);
    };
    heartbeat();
    current.addEventListener('heartbeat', heartbeat);
    current.addEventListener('error', recover);
    current.addEventListener('unavailable', recover);
    current.addEventListener('ended', () => {
      if (!valid()) return;
      cancel();
      host.controlOwner.observe(null);
    });
    current.addEventListener('control', (event) => {
      if (!valid() || !(event instanceof MessageEvent)) return;
      try {
        if (String(event.data).length > 300000) throw new Error('Control snapshot too large');
        const value: unknown = JSON.parse(String(event.data));
        if (
          !value ||
          typeof value !== 'object' ||
          !('revision' in value) ||
          typeof value.revision !== 'string' ||
          value.revision.length < 1 ||
          value.revision.length > 128 ||
          !Object.entries(scope.params).every(
            ([key, expected]) =>
              key in value && (value as Record<string, unknown>)[key] === expected,
          ) ||
          !('control' in value)
        )
          throw new Error('Control identity changed');
        const control = host.controlOwner.prepare(value.control);
        if (control && control.thread !== scope.thread) throw new Error('Control thread changed');
        heartbeat();
        delay = 1000;
        if (revision === value.revision) return;
        revision = value.revision;
        if (!host.catalogSelection) host.viewerHost?.modelNavigation?.(value);
        if (control || !scope.thread) host.controlOwner.observe(control);
        else host.controlOwner.unavailable();
      } catch {
        recover();
      }
    });
  }
  function changed() {
    if (JSON.stringify(selected()) === identity) return;
    host.controlOwner.observe(null);
    connect();
  }
  const route = host.navigation.subscribeRoute(changed);
  const model = host.updates.subscribe(changed);
  const catalog = host.catalogSelection?.subscribe(changed);
  connect();
  return {
    destroy() {
      disposed = true;
      route();
      model();
      catalog?.();
      cancel();
    },
  };
}
