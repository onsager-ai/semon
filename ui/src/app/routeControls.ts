import type { ApplicationRoute, NavigationController } from '../navigation/routes';
interface SlotContext {
  destroy?: () => void;
  sync: () => void;
  onChange: () => void;
}
interface ControlSlot {
  route: ApplicationRoute;
  box: HTMLElement;
  ctx: SlotContext;
  el: HTMLElement;
}
/** Independently embedded native controls retain their state within a route. */
export function createRouteControls(navigation: NavigationController) {
  const SLOTS = new Map<string, ControlSlot>();
  function slot(key: string, box: HTMLElement, build: (ctx: SlotContext) => HTMLElement) {
    let s = SLOTS.get(key);
    if (!s || s.route !== navigation.route || s.box !== box || !box.contains(s.el)) {
      s?.ctx.destroy?.();
      const ctx: SlotContext = { sync() {}, onChange() {} };
      s = { route: navigation.route, box, ctx, el: build(ctx) };
      SLOTS.set(key, s);
    }
    return s;
  }
  function retain(route: ApplicationRoute | null) {
    for (const [key, control] of SLOTS)
      if (control.route !== route) {
        control.ctx.destroy?.();
        SLOTS.delete(key);
      }
  }
  return {
    slot,
    retain,
    destroy() {
      retain(null);
    },
  };
}
