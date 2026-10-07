import type { EffectScope } from './effects';
import type { NavigationController } from '../navigation/routes';
import type { ViewerHost } from '../viewer-host';
import { safePath } from '../lib';
import { parseRuntimeObservation, type RuntimeObservation } from '../lib/runtimeObservation';
export interface RuntimeSelectionScope {
  readonly source_key: string;
  readonly catalog_key: string;
}
export interface RuntimeObservationFrame extends RuntimeSelectionScope {
  readonly revision: string | null;
  readonly observation: RuntimeObservation | null;
  readonly reason: string | null;
  /** Client delivery freshness never substitutes for durable observation freshness. */
  readonly delivery: 'updating' | 'current' | 'stale' | 'unavailable';
}
interface RuntimeObservationHost {
  scope: EffectScope;
  navigation: Pick<NavigationController, 'route' | 'subscribeRoute'>;
  viewerHost: Pick<ViewerHost, 'catalogRuntimeStream'> | null;
  catalogRuntimeSelection?: {
    selectedScope(): Readonly<RuntimeSelectionScope> | null;
    subscribe(listener: () => void): () => void;
  };
}
/** A selected readonly phase stream has no model or native control authority. */
export function createRuntimeObservation(host: RuntimeObservationHost) {
  let disposed = false;
  let stream: EventSource | null = null;
  let cleanups: Array<() => void> = [];
  let ticket = 0;
  let key = '';
  let frame: RuntimeObservationFrame | null = null;
  let timer: number | undefined;
  let deadline: number | undefined;
  let delay = 1000;
  const listeners = new Set<() => void>();
  const selected = (): Readonly<RuntimeSelectionScope> | null => {
    const selected = host.catalogRuntimeSelection?.selectedScope();
    const route = host.navigation.route;
    return route.v === 'session' && selected?.catalog_key === route.id ? selected : null;
  };
  const selectionKey = (scope: RuntimeSelectionScope | null) =>
    scope ? JSON.stringify([scope.source_key, scope.catalog_key]) : '';
  function publish(value: RuntimeObservationFrame | null) {
    frame = value ? Object.freeze(value) : null;
    for (const listener of listeners) listener();
  }
  function cancel() {
    ++ticket;
    for (const cleanup of cleanups) cleanup();
    cleanups = [];
    stream?.close();
    stream = null;
    host.scope.clearTimeout(timer);
    timer = undefined;
    host.scope.clearTimeout(deadline);
    deadline = undefined;
  }
  function disconnected(reason: string, permanent = false) {
    cancel();
    if (frame)
      publish({
        ...frame,
        delivery: permanent ? 'unavailable' : frame.observation ? 'stale' : 'unavailable',
        reason,
      });
    if (!permanent) {
      const generation = ticket;
      timer = host.scope.timeout(() => {
        if (!disposed && generation === ticket) connect();
      }, delay);
      delay = Math.min(15000, delay * 2);
    }
  }
  function connect() {
    if (disposed) return;
    cancel();
    const scope = selected();
    const path = host.viewerHost?.catalogRuntimeStream;
    key = selectionKey(scope);
    if (!scope || !path || !safePath(path)) {
      publish(null);
      return;
    }
    if (!frame || selectionKey(frame) !== key)
      publish({
        ...scope,
        revision: null,
        observation: null,
        reason: 'Checking environment status…',
        delivery: 'updating',
      });
    else publish({ ...frame, delivery: 'updating' });
    const generation = ticket;
    const query = new URLSearchParams({
      source_key: scope.source_key,
      catalog_key: scope.catalog_key,
    });
    let current: EventSource;
    try {
      current = new EventSource(path + (path.includes('?') ? '&' : '?') + query);
    } catch {
      disconnected('Environment status is unavailable. Retrying.');
      return;
    }
    stream = current;
    const valid = () =>
      !disposed && ticket === generation && stream === current && key === selectionKey(selected());
    const recover = () => {
      if (valid()) disconnected('Environment status updates were interrupted. Retrying.');
    };
    const heartbeat = () => {
      if (!valid()) return;
      host.scope.clearTimeout(deadline);
      deadline = host.scope.timeout(recover, 30000);
    };
    function listen(type: string, listener: EventListener) {
      current.addEventListener(type, listener);
      cleanups.push(() => current.removeEventListener(type, listener));
    }
    heartbeat();
    listen('heartbeat', heartbeat);
    listen('error', recover);
    listen('unavailable', recover);
    listen('ended', () => {
      if (valid())
        disconnected(
          'Access changed. Sign in or select an authorized workspace to inspect status.',
          true,
        );
    });
    listen('runtime', (event) => {
      if (!valid() || !(event instanceof MessageEvent)) return;
      try {
        if (String(event.data).length > 16384) throw new Error('Runtime status too large');
        const value: unknown = JSON.parse(String(event.data));
        if (!value || typeof value !== 'object' || Array.isArray(value))
          throw new Error('Invalid runtime envelope');
        const row = value as Record<string, unknown>;
        if (
          row.source_key !== scope.source_key ||
          row.catalog_key !== scope.catalog_key ||
          typeof row.revision !== 'string' ||
          !/^[0-9a-f]{64}$/.test(row.revision) ||
          !(
            row.reason === null ||
            row.reason === undefined ||
            (typeof row.reason === 'string' && row.reason.length <= 512)
          ) ||
          row.runtime === undefined
        )
          throw new Error('Runtime selection changed');
        const observation = row.runtime === null ? null : parseRuntimeObservation(row.runtime);
        heartbeat();
        delay = 1000;
        if (frame?.revision === row.revision && frame.delivery === 'current') return;
        publish({
          ...scope,
          revision: row.revision,
          observation,
          reason: typeof row.reason === 'string' ? row.reason : null,
          delivery: 'current',
        });
      } catch {
        recover();
      }
    });
  }
  const reselect = () => {
    const next = selectionKey(selected());
    if (next === key) return;
    cancel();
    key = next;
    delay = 1000;
    publish(null);
    connect();
  };
  const unsubscribe = host.catalogRuntimeSelection?.subscribe(reselect) ?? (() => {});
  const unnavigate = host.navigation.subscribeRoute(reselect);
  host.scope.listen(document, 'semon:refresh' as keyof DocumentEventMap, () => {
    delay = 1000;
    connect();
  });
  connect();
  return {
    view: () => frame,
    subscribe(listener: () => void) {
      if (disposed) return () => {};
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    destroy() {
      if (disposed) return;
      disposed = true;
      cancel();
      unsubscribe();
      unnavigate();
      listeners.clear();
      frame = null;
    },
  };
}
