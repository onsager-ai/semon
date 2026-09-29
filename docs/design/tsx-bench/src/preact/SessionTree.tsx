// The sidebar's session tree in Preact: the whole tree re-renders from the model on each update, keyed rows keep their
// DOM nodes, and a row whose signature is unchanged skips its diff (shouldComponentUpdate), so a poll that changed one
// session touches one row. The order rule's snapshot lives in a ref, outside render's inputs.
import { Component } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
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
  sig: string;
  onToggle: (id: string) => void;
  onOpen: (id: string) => void;
}

class Row extends Component<RowProps> {
  override shouldComponentUpdate(next: RowProps): boolean {
    return next.sig !== this.props.sig;
  }
  override render() {
    const { s, now, kids, all, open, current, onToggle, onOpen } = this.props, parts = childParts(all);
    return (
      <div class="treeitem" role="treeitem" aria-label={s.name} aria-expanded={kids.length ? open : undefined} data-id={s.id} tabIndex={0}>
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
            <button class="tree-toggle" type="button" aria-expanded={open} aria-label={(open ? "Collapse " : "Expand ") + s.name} onClick={() => onToggle(s.id)}>
              <Chevron />
            </button>
          ) : null}
        </div>
        {kids.length && open ? (
          <div class="tree-group" role="group" aria-label={"Sessions spawned by " + s.name}>
            {kids.map((k) => (
              <Row key={k.id} s={k} now={now} kids={[]} all={[]} open={false} current={false} sig={k.id + ":" + k.state + ":" + ago(now, k.last)} onToggle={onToggle} onOpen={onOpen} />
            ))}
          </div>
        ) : null}
      </div>
    );
  }
}

const sig = (s: Session, now: number, kids: Session[], all: Session[], open: boolean, current: boolean): string =>
  [s.name, s.state, ago(now, s.last), open, current, all.length, childParts(all).join(","), ...kids.map((k) => k.id + ":" + k.state + ":" + ago(now, k.last))].join("|");

export function SessionTree({ model, current, onOpen }: { model: Model; current?: string; onOpen: (id: string) => void }) {
  const order = useRef<string[] | null>(null), touched = useRef(false), topInView = useRef(true), head = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState<ReadonlySet<string>>(() => new Set());
  const [, resort] = useState(0);
  useEffect(() => {
    if (!head.current) return;
    const io = new IntersectionObserver(([e]) => (topInView.current = !!e?.isIntersecting));
    io.observe(head.current);
    return () => io.disconnect();
  }, []);
  const children = childrenOf(model.sessions), tops = model.sessions.filter((s) => !s.parent);
  const kept = keepOrder(order.current, tops, topInView.current && !touched.current);
  order.current = kept.order;
  const byId = new Map(tops.map((s) => [s.id, s]));
  const toggle = (id: string) =>
    setOpen((was) => {
      const next = new Set(was);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  return (
    <nav class="lanes" onPointerOver={() => (touched.current = true)} onPointerLeave={() => (touched.current = false)} onFocusIn={() => (touched.current = true)} onFocusOut={() => (touched.current = false)}>
      <div class="ghead" ref={head}>
        <span>Recent</span>
        <button
          class="updated"
          type="button"
          hidden={!kept.held}
          onClick={() => {
            order.current = null;
            resort((n) => n + 1);
          }}
        >
          <span class="n">{kept.held} updated</span>
        </button>
      </div>
      <div class="tree" role="tree" aria-label="Sessions">
        {kept.order.slice(0, 8).map((id) => {
          const s = byId.get(id);
          if (!s) return null;
          const kids = children.get(id) ?? [], all = descendants(id, children), isOpen = open.has(id) || all.some((x) => x.state === "wait");
          return <Row key={id} s={s} now={model.now} kids={kids} all={all} open={isOpen} current={current === id} sig={sig(s, model.now, kids, all, isOpen, current === id)} onToggle={toggle} onOpen={onOpen} />;
        })}
      </div>
    </nav>
  );
}
