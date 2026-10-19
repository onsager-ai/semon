import { safePath, type ShellNavigation, type ShellDestination } from './lib';
/** Narrow document host port; the viewer remains the sole router and read lifecycle owner. */
export interface ViewerContent {
  element: HTMLElement;
  /** Host-owned heading while this initial content occupies the canonical reader. */
  title?: string;
  destroy(): void;
}
/** Server-authorized content occupies the same router/content slot as the reader. */
export interface ViewerPage extends ViewerContent {
  title: string;
  nav: string;
  /** Enhancements mount after insertion and are released by destroy. */
  mount?(): void;
}
export interface ViewerPages {
  match(url: URL): { title: string; nav: string } | null;
  initial?: ViewerPage;
  /** null requires a fresh authorized document (for example after a workspace change). */
  load(url: URL, signal: AbortSignal): Promise<ViewerPage | null>;
}
export interface ViewerHost {
  pages?: ViewerPages;
  /** false for authenticated hosts without a current workspace; no workspace chrome reads. */
  workspaceMetadata?: boolean;
  machinesPath: string;
  /** Shared destination policy; no host data or routing owner lives in chrome. */
  navigation?: ShellNavigation;
  /** Narrow host destination notifications; no model observation is implied. */
  subscribeNavigation?(changed: () => void): () => void;
  /** Host-owned links use native navigation; shared chrome validates paths. */
  nativeNavigation?: readonly ShellDestination[];
  /** A native administrative page uses shared chrome and native links without a reader/model. */
  nativePage?: {
    title: string;
    nav: string;
    /** An authenticated workspace can publish compact Recent/count metadata without selecting a reader. */
    workspaceChrome?: boolean;
  };
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
