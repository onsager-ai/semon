import { safePath, type Account, type ShellDestination } from './lib';
/** Narrow document host port; the viewer remains the sole router/model poller. */
export interface ViewerContent {
  element: HTMLElement;
  destroy(): void;
}
export interface ViewerHost {
  machinesPath: string;
  /** Host-owned links use native navigation; shared chrome validates paths. */
  nativeNavigation?: readonly ShellDestination[];
  /** A native administrative page uses shared chrome/Recent, with native destination links. */
  nativePage?: { title: string; nav: string };
  account?: unknown;
  modelAccount?(account: Account | null): void;
  /** Host SSE models enter the existing validated model transaction. */
  modelStream?: string;
  /** Bind hosted model/control reads to the selected native session and model machine. */
  selectedModel?: boolean;
  /** Separate bounded status snapshots, independent of content model revisions. */
  controlStream?: string;
  modelNavigation?(model: unknown): void;
  /** Return true when the host has taken native recovery for a model failure. */
  modelFailed?(status: number): boolean;
  initialMachines?: ViewerContent;
  loadMachines(signal: AbortSignal): Promise<ViewerContent>;
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
