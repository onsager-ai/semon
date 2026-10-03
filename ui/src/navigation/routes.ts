import type { ViewerRoute } from '../lib/routes';
import type { ViewerContent } from '../viewer-host';
export interface HostFocus { id?: string; row?: string; label?: string }
export type ApplicationRoute = ViewerRoute & { scrollTop?: number; hostFocus?: HostFocus; q?: string; sheet?: number };
export interface NavigationHost { loadMachines?(signal: AbortSignal): Promise<ViewerContent> }
/** Owns route tokens, destination requests and native-content lifetime. */
export class NavigationController {
  route: ApplicationRoute = { v: 'home' };
  rendered: ApplicationRoute | null = null;
  content: ViewerContent | null;
  private nativePending: AbortController | null = null;
  private routePending: AbortController | null = null;
  private disposed = false;
  constructor(private readonly host: NavigationHost, initial: ViewerContent | null = null) { this.content = initial; }
  cancelNative() { this.nativePending?.abort(); this.nativePending = null; }
  cancelRoute() { this.routePending?.abort(); this.routePending = null; }
  loadNative(route: ApplicationRoute, ready: (content: ViewerContent) => void, failed: () => void) {
    if (this.disposed || !this.host.loadMachines) return;
    this.cancelNative(); const request = new AbortController(); this.nativePending = request;
    this.host.loadMachines(request.signal).then(content => {
      if (this.disposed || request.signal.aborted || this.nativePending !== request) { content.destroy(); return; }
      this.nativePending = null; ready(content);
    }, () => { if (!this.disposed && !request.signal.aborted && this.nativePending === request) { this.nativePending = null; failed(); } });
  }
  replaceContent(route: ApplicationRoute, next: ViewerContent | null) {
    if (next) { this.content?.destroy(); this.content = next; }
    else if (route.v !== 'machines' && this.content) { this.content.destroy(); this.content = null; }
  }
  load(route: ApplicationRoute, request: (signal?: AbortSignal) => Promise<unknown> | null, done: () => void, failed: (error: unknown) => void) {
    if (this.disposed) return;
    this.cancelRoute(); const controller = new AbortController(); this.routePending = controller;
    const loaded = request(controller.signal);
    const current = () => !this.disposed && !controller.signal.aborted && this.routePending === controller && this.route === route;
    if (loaded) loaded.then(() => { if (current()) { this.routePending = null; done(); } }, error => { if (current()) { this.routePending = null; failed(error); } });
    else if (current()) { this.routePending = null; done(); }
  }
  historyRoute(value: unknown, fallback: ApplicationRoute): ApplicationRoute {
    // URLs/model validation remain authoritative; history only adds validated restoration metadata.
    if (!value || typeof value !== 'object') return fallback;
    const out = { ...fallback };
    if ('scrollTop' in value && typeof value.scrollTop === 'number' && Number.isFinite(value.scrollTop)) out.scrollTop = value.scrollTop;
    if ('hostFocus' in value && value.hostFocus && typeof value.hostFocus === 'object') {
      const focus = value.hostFocus;
      out.hostFocus = { id: 'id' in focus && typeof focus.id === 'string' ? focus.id : undefined, row: 'row' in focus && typeof focus.row === 'string' ? focus.row : undefined, label: 'label' in focus && typeof focus.label === 'string' ? focus.label : undefined };
    }
    return out;
  }
  destroy() { if (this.disposed) return; this.disposed = true; this.cancelNative(); this.cancelRoute(); this.content?.destroy(); this.content = null; this.rendered = null; }
}
