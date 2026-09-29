// The sidebar's session tree in Solid: the model goes into a store with reconcile(), so an update writes only the fields
// that changed, and each binding that reads one (a dot's class, an age) updates that text or attribute alone. No
// component re-runs. <For> is keyed by id, so rows keep their nodes when the held order moves them. The order rule
// reads the list's free state untracked: only a model change re-sorts, never a pointer leaving the list.
import { createMemo, createSignal, For, onCleanup, onMount, Show, untrack, type Accessor } from "solid-js";
import { ago, childParts, childrenOf, descendants, STATE_LABEL, type Model, type Session, type State } from "../shared/model";
import { keepOrder } from "../shared/order";

const CHEV = "M9 6l6 6-6 6";

const Dot = (p: { state: State }) => <span class={"dot " + p.state} role="img" aria-label={STATE_LABEL[p.state]} data-tip={STATE_LABEL[p.state]} />;

const Chevron = () => (
  <svg viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.8">
    <path d={CHEV} />
  </svg>
);

interface RowProps {
  s: Session;
  now: Accessor<number>;
  kids: Accessor<Session[]>;
  all: Accessor<Session[]>;
  open: Accessor<boolean>;
  current: Accessor<boolean>;
  onToggle: (id: string) => void;
  onOpen: (id: string) => void;
}

function Row(p: RowProps) {
  const parts = createMemo(() => childParts(p.all()));
  return (
    <div class="treeitem" role="treeitem" aria-label={p.s.name} aria-expanded={p.kids().length ? p.open() : undefined} data-id={p.s.id} tabIndex={0}>
      <div class={"tree-row" + (p.kids().length ? " has-toggle" : "")}>
        <button class="srow" type="button" data-id={p.s.id} aria-current={p.current() ? "page" : undefined} aria-label={[p.s.name, STATE_LABEL[p.s.state], p.s.host, ...parts()].join(", ")} onClick={() => p.onOpen(p.s.id)}>
          <span class="srow-main">
            <Dot state={p.s.state} />
            <span class="nm" data-tip={p.s.name} data-tip-clipped="">
              {p.s.name}
            </span>
            <Show when={p.all().length}>
              <span class={"tree-summary" + (p.all().some((x) => x.state === "wait") ? " wait" : "")} data-tip={parts().join(" · ")}>
                {p.all().length}
              </span>
            </Show>
            <span class="ag">{ago(p.now(), p.s.last)}</span>
          </span>
        </button>
        <Show when={p.kids().length}>
          <button class="tree-toggle" type="button" aria-expanded={p.open()} aria-label={(p.open() ? "Collapse " : "Expand ") + p.s.name} onClick={() => p.onToggle(p.s.id)}>
            <Chevron />
          </button>
        </Show>
      </div>
      <Show when={p.kids().length && p.open()}>
        <div class="tree-group" role="group" aria-label={"Sessions spawned by " + p.s.name}>
          <For each={p.kids()}>{(k) => <Row s={k} now={p.now} kids={() => []} all={() => []} open={() => false} current={() => false} onToggle={p.onToggle} onOpen={p.onOpen} />}</For>
        </div>
      </Show>
    </div>
  );
}

export function SessionTree(p: { model: Accessor<Model>; byId: (id: string) => Session | undefined; current: Accessor<string | undefined>; onOpen: (id: string) => void }) {
  let head!: HTMLDivElement;
  let touched = false, topInView = true, resetting = false;
  const [open, setOpen] = createSignal<ReadonlySet<string>>(new Set());
  const [resorted, resort] = createSignal(0);
  onMount(() => {
    const io = new IntersectionObserver(([e]) => (topInView = !!e?.isIntersecting));
    io.observe(head);
    onCleanup(() => io.disconnect());
  });
  const children = createMemo(() => childrenOf(p.model().sessions));
  const kept = createMemo<{ order: string[]; held: number } | undefined>((prev) => {
    resorted();
    const tops = p.model().sessions.filter((s) => !s.parent);
    return keepOrder(untrack(() => (prev && !resetting ? prev.order : null)), tops, untrack(() => topInView && !touched));
  });
  const toggle = (id: string) =>
    setOpen((was) => {
      const next = new Set(was);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  return (
    <nav class="lanes" onPointerOver={() => (touched = true)} onPointerLeave={() => (touched = false)} onFocusIn={() => (touched = true)} onFocusOut={() => (touched = false)}>
      <div class="ghead" ref={head}>
        <span>Recent</span>
        <button
          class="updated"
          type="button"
          hidden={!kept()?.held}
          onClick={() => {
            resetting = true;
            resort((n) => n + 1);
            resetting = false;
          }}
        >
          <span class="n">{kept()?.held} updated</span>
        </button>
      </div>
      <div class="tree" role="tree" aria-label="Sessions">
        <For each={kept()?.order.slice(0, 8) ?? []}>
          {(id) => {
            const s = p.byId(id);
            if (!s) return null;
            const kids = () => children().get(id) ?? [], all = () => descendants(id, children());
            const isOpen = () => open().has(id) || all().some((x) => x.state === "wait");
            return <Row s={s} now={() => p.model().now} kids={kids} all={all} open={isOpen} current={() => p.current() === id} onToggle={toggle} onOpen={p.onOpen} />;
          }}
        </For>
      </div>
    </nav>
  );
}
