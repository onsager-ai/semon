// The sidebar's session tree with the hand-written factory: rows are built by JSX, and a live update rebuilds only the
// rows whose signature changed and moves nothing the order rule holds (patchList). The mount keeps its own state in the
// closure, as viewer.js does today, but typed and in one module.
import { h, patchList } from "./jsx";
import { ago, childParts, childrenOf, descendants, STATE_LABEL, type Model, type Session, type State } from "../shared/model";
import { keepOrder } from "../shared/order";

const CHEV = "M9 6l6 6-6 6";

const Dot = ({ state }: { state: State }) => <span class={"dot " + state} role="img" aria-label={STATE_LABEL[state]} data-tip={STATE_LABEL[state]} />;

const Chevron = () => (
  <svg viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.8">
    <path d={CHEV} />
  </svg>
);

interface RowProps {
  s: Session;
  now: number;
  kids: Session[];
  all: Session[];
  open: boolean;
  current: boolean;
  onToggle: (id: string) => void;
  onOpen: (id: string) => void;
}

function Row({ s, now, kids, all, open, current, onToggle, onOpen }: RowProps) {
  const parts = childParts(all);
  return (
    <div class="treeitem" role="treeitem" aria-label={s.name} aria-expanded={kids.length ? String(open) : undefined} data-id={s.id} tabIndex={0}>
      <div class={"tree-row" + (kids.length ? " has-toggle" : "")}>
        <button class="srow" type="button" data-id={s.id} aria-current={current ? "page" : undefined} aria-label={[s.name, STATE_LABEL[s.state], s.host, ...parts].join(", ")} onClick={() => onOpen(s.id)}>
          <span class="srow-main">
            <Dot state={s.state} />
            <span class="nm" data-tip={s.name} data-tip-clipped="">
              {s.name}
            </span>
            {all.length ? (
              <span class={"tree-summary" + (all.some((x) => x.state === "wait") ? " wait" : "")} data-tip={parts.join(" · ")}>
                {all.length}
              </span>
            ) : null}
            <span class="ag">{ago(now, s.last)}</span>
          </span>
        </button>
        {kids.length ? (
          <button class="tree-toggle" type="button" aria-expanded={String(open)} aria-label={(open ? "Collapse " : "Expand ") + s.name} onClick={() => onToggle(s.id)}>
            <Chevron />
          </button>
        ) : null}
      </div>
      {kids.length && open ? (
        <div class="tree-group" role="group" aria-label={"Sessions spawned by " + s.name}>
          {kids.map((k) => (
            <Row s={k} now={now} kids={[]} all={[]} open={false} current={false} onToggle={onToggle} onOpen={onOpen} />
          ))}
        </div>
      ) : null}
    </div>
  );
}

// What a row shows: when this is unchanged the row's nodes stay, with focus and anything opened in them.
const sig = (s: Session, now: number, kids: Session[], all: Session[], open: boolean, current: boolean): string =>
  [s.name, s.state, ago(now, s.last), open, current, all.length, childParts(all).join(","), ...kids.map((k) => k.id + ":" + k.state + ":" + ago(now, k.last))].join("|");

export interface Tree {
  el: HTMLElement;
  update(model: Model, current?: string): number;
}

export function mountSessionTree(onOpen: (id: string) => void): Tree {
  let order: string[] | null = null, touched = false, topInView = true, last: Model | null = null, current: string | undefined;
  const open = new Set<string>();
  const list = (<div class="tree" role="tree" aria-label="Sessions" />) as HTMLElement;
  const chip = (
    <button class="updated" type="button" hidden onClick={() => resort()}>
      <span class="n" />
    </button>
  ) as HTMLButtonElement;
  const head = (
    <div class="ghead">
      <span>Recent</span>
      {chip}
    </div>
  ) as HTMLElement;
  const el = (
    <nav class="lanes" onPointerOver={() => (touched = true)} onPointerLeave={() => (touched = false)} onFocusIn={() => (touched = true)} onFocusOut={() => (touched = false)}>
      {head}
      {list}
    </nav>
  ) as HTMLElement;
  new IntersectionObserver(([e]) => (topInView = !!e?.isIntersecting)).observe(head);

  const toggle = (id: string) => {
    if (open.has(id)) open.delete(id);
    else open.add(id);
    draw();
  };
  const resort = () => {
    order = null;
    draw();
  };
  function draw(): number {
    if (!last) return 0;
    const now = last.now, children = childrenOf(last.sessions), tops = last.sessions.filter((s) => !s.parent);
    const kept = keepOrder(order, tops, topInView && !touched);
    order = kept.order;
    chip.hidden = !kept.held;
    chip.querySelector(".n")!.textContent = kept.held + " updated";
    const byId = new Map(tops.map((s) => [s.id, s]));
    return patchList(
      list,
      kept.order.slice(0, 8).flatMap((id) => {
        const s = byId.get(id);
        if (!s) return [];
        const kids = children.get(id) ?? [], all = descendants(id, children), isOpen = open.has(id) || all.some((x) => x.state === "wait");
        return [{ key: id, sig: sig(s, now, kids, all, isOpen, current === id), build: () => <Row s={s} now={now} kids={kids} all={all} open={isOpen} current={current === id} onToggle={toggle} onOpen={onOpen} /> }];
      }),
    );
  }
  return {
    el,
    update(model, cur) {
      last = model;
      current = cur;
      return draw();
    },
  };
}
