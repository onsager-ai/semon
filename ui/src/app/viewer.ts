import { getViewerHost, type ViewerHost } from '../viewer-host';
import { ViewerComposition } from './composition';
export interface ViewerApplication {
  destroy(): void;
}
let mounted: ViewerApplication | null = null;
/** One document owner; feature services own state, behavior and effects. */
export function mountViewerApplication(
  host: ViewerHost | null = getViewerHost(),
): ViewerApplication {
  mounted?.destroy();
  const composition = new ViewerComposition(host, (owner) => {
    if (mounted === owner) mounted = null;
  });
  mounted = composition.application;
  return composition.application;
}
