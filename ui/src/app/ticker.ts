import { updateSessionClock } from "../lib";
interface TickerHost {
  visible: () => boolean;
  transportOwner: ReturnType<typeof import("./transport").createTransport>;
  tick: () => void;
  navigation: import("../navigation/routes").NavigationController;
  $: <T extends HTMLElement = HTMLElement>(s: string, r?: ParentNode) => T;
  NOW: number;
  SESS: Record<string, import("../domain/types").Session>;
  renderHome: (page: HTMLElement) => void;
}
/** Owns ticker behavior through explicit application ports. */
export function createTicker(host: TickerHost) {
  const running = (ms: number) => { const x = Math.max(0, Math.floor(ms / 1000)); return x < 60 ? x + "s" : Math.floor(x / 60) + "m " + (x % 60) + "s"; };
  function ticker() {
    if (!host.visible() || Date.now() === host.transportOwner.fetchedAt) return;
    host.tick();
    if (host.navigation.route.v === "session" && host.navigation.rendered === host.navigation.route) updateSessionClock(host.$("#page"), host.NOW, Object.fromEntries(Object.values(host.SESS).filter((s) => s.activity?.[3] != null).map((s) => [s.id, s.activity![3]!])));
    else if (host.navigation.route.v === "home" && host.navigation.rendered === host.navigation.route) host.renderHome(host.$("#page"));
  }


  return {ticker, running};
}
