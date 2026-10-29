import { getViewerHost, type ViewerHost } from '../viewer-host';
import { createCatalogViewer } from './catalogViewer';
export interface ViewerApplication {
  destroy(): void;
}
let mounted: ViewerApplication | null = null;
/** The document has one reader, shell, router and effect owner. */
export function mountViewerApplication(
  host: ViewerHost | null = getViewerHost(),
): ViewerApplication {
  mounted?.destroy();
  const url = new URL(location.href);
  const before = url.href;
  // The standalone HTTP boundary already authenticated this bootstrap credential.
  // Consume it before any shared route parsing or canonical history update.
  url.searchParams.delete('t');
  if (url.href !== before) {
    history.replaceState(history.state, '', url.pathname + url.search + url.hash);
  }
  const owner = createCatalogViewer(null, host);
  const application = {
    destroy() {
      owner.destroy();
      if (mounted === application) mounted = null;
    },
  };
  mounted = application;
  return application;
}
