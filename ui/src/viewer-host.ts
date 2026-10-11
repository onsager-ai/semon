import { safePath, type ShellNavigation, type ShellDestination } from './lib';
/** Narrow document host port; the viewer remains the sole router and read lifecycle owner. */
export interface ViewerContent {
  element: HTMLElement;
  destroy(): void;
}
export interface ViewerHost {
  machinesPath: string;
  /** Shared destination policy; no host data or routing owner lives in chrome. */
  navigation?: ShellNavigation;
  /** Narrow host destination notifications; no model observation is implied. */
  subscribeNavigation?(changed: () => void): () => void;
  /** Host-owned links use native navigation; shared chrome validates paths. */
  nativeNavigation?: readonly ShellDestination[];
  /** A native administrative page uses shared chrome and native links without a reader/model. */
  nativePage?: { title: string; nav: string };
  /** Advertise source inventory so an unselected reader can choose without a failing probe. */
  catalogSources?: boolean;
  account?: unknown;
  /** Exact catalog/source status; identity comes from the accepted Viewer selection owner. */
  catalogControlStream?: string;
  /** Readonly selected phase/status by source/catalog intent, independent of native identity. */
  catalogRuntimeStream?: string;
  initialMachines?: ViewerContent;
  /** Optional native onboarding content inside canonical Home; no reader or router. */
  initialHome?: ViewerContent;
}
let host: ViewerHost | null = null;
export function configureViewerHost(value: ViewerHost) {
  if (!safePath(value.machinesPath)) throw new Error('Invalid Machines path');
  if (host) throw new Error('Viewer host already configured');
  host = value;
}
export function getViewerHost() {
  return host;
}
