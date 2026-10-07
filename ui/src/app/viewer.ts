import { getViewerHost, type ViewerHost } from '../viewer-host';
import { ViewerComposition } from './composition';
import { createCatalogViewer } from './catalogViewer';
import { createCatalogSources } from './catalogSources';
import { parseCatalogCapabilities } from '../state/catalog-capabilities';
import { requestJson } from '../lib/model';
import { EffectScope } from './effects';
export interface ViewerApplication {
  destroy(): void;
}
let mounted: ViewerApplication | null = null;
/** One document owner; feature services own state, behavior and effects. */
export function mountViewerApplication(
  host: ViewerHost | null = getViewerHost(),
): ViewerApplication {
  mounted?.destroy();
  const scope = new EffectScope();
  let disposed = false,
    owner: ViewerApplication | null = null,
    retryDelay = 1000,
    choosing = false;
  const application: ViewerApplication = {
    destroy() {
      if (disposed) return;
      disposed = true;
      scope.destroy();
      owner?.destroy();
      if (mounted === application) mounted = null;
    },
  };
  mounted = application;
  function legacy() {
    if (disposed) return;
    owner = new ViewerComposition(host, () => {}).application;
  }
  function chooseSources() {
    owner = createCatalogSources(host, (key) => {
      if (disposed) return;
      owner?.destroy();
      owner = null;
      const url = new URL(location.href);
      url.searchParams.set('machine', key);
      history.replaceState(null, '', url.pathname + url.search + url.hash);
      void chooseReader();
    });
  }
  async function chooseReader() {
    if (disposed || owner || choosing) return;
    choosing = true;
    const request = scope.request(),
      deadline = scope.timeout(() => request.abort(), 15000);
    try {
      const params = new URLSearchParams();
      const source = new URLSearchParams(location.search).get('machine');
      if (source !== null) params.set('machine', source);
      const value = await requestJson(
        '/api/session-capabilities' + (params.size ? '?' + params : ''),
        request.signal,
      );
      const capabilities = parseCatalogCapabilities(value);
      if (source !== null && capabilities.source_key !== source)
        throw new Error('Source capability identity does not match this selection');
      if (!disposed) owner = createCatalogViewer(capabilities, host);
    } catch (error) {
      if (disposed) return;
      const status = error && typeof error === 'object' && 'status' in error ? error.status : 0;
      // Older servers explicitly lack this endpoint. Every recognized catalog
      // contract remains independent of global reads, including failed observations.
      if (status === 404) {
        legacy();
        return;
      }
      if (status === 400 && !new URLSearchParams(location.search).has('machine')) {
        chooseSources();
        return;
      }
      const root = document.querySelector<HTMLElement>('#page');
      if (root) {
        const notice = document.createElement('p');
        notice.className = 'catalog-note';
        notice.setAttribute('role', 'status');
        notice.textContent =
          status === 401 || status === 403
            ? 'Authorization is required to read this source.'
            : status === 400
              ? 'Choose a machine to browse its recorded history.'
              : 'Session history is unavailable. Reconnecting…';
        const machines = document.createElement('a');
        machines.className = 'link';
        machines.href = host?.machinesPath ?? '/machines?compat=1';
        machines.textContent = 'Choose a machine';
        const compatibility = document.createElement('a');
        compatibility.className = 'link';
        compatibility.href = '/sessions?compat=1';
        compatibility.textContent = 'Open compatibility view (loads workspace history)';
        const actions = document.createElement('p');
        actions.className = 'catalog-note';
        actions.append(machines, document.createTextNode(' · '), compatibility);
        root.replaceChildren(notice, actions);
      }
      if (status !== 400 && status !== 401 && status !== 403) {
        scope.timeout(() => void chooseReader(), retryDelay);
        retryDelay = Math.min(8000, retryDelay * 2);
      }
    } finally {
      choosing = false;
      scope.clearTimeout(deadline);
      scope.releaseRequest(request);
    }
  }
  // Native embedding pages retain their explicitly configured compatibility owner.
  if (
    new URLSearchParams(location.search).get('compat') === '1' ||
    host?.nativePage ||
    (host?.loadMachines && host.machinesPath === location.pathname) ||
    document.querySelector<HTMLElement>('.app')?.dataset.viewer === 'sidebar'
  )
    legacy();
  else if (host?.catalogSources && !new URLSearchParams(location.search).has('machine'))
    chooseSources();
  else {
    void chooseReader();
    scope.listen(window, 'focus', () => void chooseReader());
  }
  return application;
}
