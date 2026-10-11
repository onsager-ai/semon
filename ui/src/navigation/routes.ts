import type { ViewerRoute } from '../lib/routes';
/** Route observation shared by the canonical reader and scoped controls. */
export class NavigationController {
  constructor(private readonly observe?: (route: ViewerRoute) => void) {}
  private currentRoute: ViewerRoute = { v: 'home' };
  private readonly routeListeners = new Set<() => void>();
  private disposed = false;
  get route() {
    return this.currentRoute;
  }
  set route(value: ViewerRoute) {
    if (this.disposed) return;
    this.currentRoute = value;
    this.observe?.(value);
    for (const listener of this.routeListeners) listener();
  }
  subscribeRoute(listener: () => void) {
    if (!this.disposed) this.routeListeners.add(listener);
    return () => this.routeListeners.delete(listener);
  }
  destroy() {
    this.disposed = true;
    this.routeListeners.clear();
  }
}
