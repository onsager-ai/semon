import { safePath, type Account } from './lib';
/** Narrow document host port; the viewer remains the sole router/model poller. */
export interface ViewerContent {
  element: HTMLElement;
  destroy(): void;
}
export interface ViewerHost {
  machinesPath: string;
  /** A native administrative page uses shared chrome/Recent, with native destination links. */
  nativePage?: { title: string; nav: string };
  account?: unknown;
  modelAccount?(account: Account | null): void;
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
