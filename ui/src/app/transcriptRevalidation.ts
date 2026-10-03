import type { ApplicationRoute } from '../navigation/routes';
interface TranscriptRevalidationHost {
  TXM: Record<string, import("../domain/types").TranscriptMeta>;
  transportOwner: ReturnType<typeof import("./transport").createTransport>;
  reload: (sid: string) => Promise<{ cut: null; reload: boolean; }>;
  tail: ReturnType<typeof import("./liveUpdates").createLiveUpdates>["tail"];
  navigation: import("../navigation/routes").NavigationController;
  refresh: (dirty?: ReadonlySet<string> | null) => void;
}
/** Owns transcriptRevalidation behavior through explicit application ports. */
export function createTranscriptRevalidation(host: TranscriptRevalidationHost) {
  function shrank(a: string | undefined, b: string | undefined) { const [s0, b0] = String(a).split(".").map(Number), [s1, b1] = String(b).split(".").map(Number); return s1 < s0 || b1 < b0; }
  // A transcript drawn from the cache is brought up to date the way a live update does it: when the model's mark for it moved
  // since it was kept, its tail is fetched (or the whole page, if the file shrank), and its child work loads. Nothing is asked
  // for when the mark is the same. The page is drawn again, keeping the reader's place, once something arrived.
  function revalidate(r: Extract<ApplicationRoute, {v: "session"}>) {
    const sid = r.id, m = host.TXM[sid], moved = m && m.to >= m.total && m.tok != null && host.transportOwner.TOK[sid] != null && m.tok !== host.transportOwner.TOK[sid];
    const job = moved ? (shrank(m.tok, host.transportOwner.TOK[sid]) ? host.reload(sid) : host.tail(sid)) : null, work = job;
    if (work) work.then(() => { if (host.navigation.route === r && host.navigation.rendered === r) host.refresh(null); }, () => {});
  }
  // Paging state survives redraws: a click and an observer share one request per session and direction, and a failed
  // page stays manual until Retry succeeds. Observers belong only to the buttons currently drawn.

  return {revalidate, shrank};
}
