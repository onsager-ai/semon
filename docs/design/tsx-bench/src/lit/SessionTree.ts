// The sidebar's session tree in lit-html: tagged templates instead of JSX (lit has no TSX). Each update renders the
// whole template again; lit compares each binding's value with the last and writes only the ones that differ, and
// repeat() keys the rows by id so held rows keep their nodes.
import { html, nothing, render, svg } from "lit-html";
import { ref, createRef } from "lit-html/directives/ref.js";
import { repeat } from "lit-html/directives/repeat.js";
import { ago, childParts, childrenOf, descendants, STATE_LABEL, type Model, type Session, type State } from "../shared/model";
import { keepOrder } from "../shared/order";

const CHEV = "M9 6l6 6-6 6";

const dot = (state: State) => html`<span class=${"dot " + state} role="img" aria-label=${STATE_LABEL[state]} data-tip=${STATE_LABEL[state]}></span>`;
const chevron = svg`<svg viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.8"><path d=${CHEV}></path></svg>`;

interface RowArgs {
  s: Session;
  now: number;
  kids: Session[];
  all: Session[];
  open: boolean;
  current: boolean;
  onToggle: (id: string) => void;
  onOpen: (id: string) => void;
}

function row(a: RowArgs): ReturnType<typeof html> {
  const { s, now, kids, all, open, current } = a, parts = childParts(all);
  return html`<div class="treeitem" role="treeitem" aria-label=${s.name} aria-expanded=${kids.length ? String(open) : nothing} data-id=${s.id} tabindex="0">
    <div class=${"tree-row" + (kids.length ? " has-toggle" : "")}>
      <button class="srow" type="button" data-id=${s.id} aria-current=${current ? "page" : nothing} aria-label=${[s.name, STATE_LABEL[s.state], s.host, ...parts].join(", ")} @click=${() => a.onOpen(s.id)}>
        <span class="srow-main">
          ${dot(s.state)}
          <span class="nm" data-tip=${s.name} data-tip-clipped="">${s.name}</span>
          ${all.length ? html`<span class=${"tree-summary" + (all.some((x) => x.state === "wait") ? " wait" : "")} data-tip=${parts.join(" · ")}>${all.length}</span>` : nothing}
          <span class="ag">${ago(now, s.last)}</span>
        </span>
      </button>
      ${kids.length
        ? html`<button class="tree-toggle" type="button" aria-expanded=${String(open)} aria-label=${(open ? "Collapse " : "Expand ") + s.name} @click=${() => a.onToggle(s.id)}>${chevron}</button>`
        : nothing}
    </div>
    ${kids.length && open
      ? html`<div class="tree-group" role="group" aria-label=${"Sessions spawned by " + s.name}>
          ${repeat(kids, (k) => k.id, (k) => row({ ...a, s: k, kids: [], all: [], open: false, current: false }))}
        </div>`
      : nothing}
  </div>`;
}

export interface Tree {
  update(model: Model, current?: string): void;
}

export function mountSessionTree(box: HTMLElement, onOpen: (id: string) => void): Tree {
  let order: string[] | null = null, touched = false, topInView = true, last: Model | null = null, current: string | undefined;
  const open = new Set<string>(), head = createRef<HTMLDivElement>();
  let io: IntersectionObserver | null = null;
  const watchHead = (n: Element | undefined) => {
    if (!n || io) return;
    io = new IntersectionObserver(([e]) => (topInView = !!e?.isIntersecting));
    io.observe(n);
  };
  const toggle = (id: string) => {
    if (!open.delete(id)) open.add(id);
    draw();
  };
  function draw(): void {
    if (!last) return;
    const now = last.now, children = childrenOf(last.sessions), tops = last.sessions.filter((s) => !s.parent);
    const kept = keepOrder(order, tops, topInView && !touched);
    order = kept.order;
    const byId = new Map(tops.map((s) => [s.id, s]));
    const shown = kept.order.slice(0, 8).flatMap((id) => {
      const s = byId.get(id);
      return s ? [s] : [];
    });
    render(
      html`<nav class="lanes" @pointerover=${() => (touched = true)} @pointerleave=${() => (touched = false)} @focusin=${() => (touched = true)} @focusout=${() => (touched = false)}>
        <div class="ghead" ${ref(head)} ${ref(watchHead)}>
          <span>Recent</span>
          <button class="updated" type="button" ?hidden=${!kept.held} @click=${() => ((order = null), draw())}><span class="n">${kept.held} updated</span></button>
        </div>
        <div class="tree" role="tree" aria-label="Sessions">
          ${repeat(
            shown,
            (s) => s.id,
            (s) => {
              const kids = children.get(s.id) ?? [], all = descendants(s.id, children), isOpen = open.has(s.id) || all.some((x) => x.state === "wait");
              return row({ s, now, kids, all, open: isOpen, current: current === s.id, onToggle: toggle, onOpen });
            },
          )}
        </div>
      </nav>`,
      box,
    );
  }
  return {
    update(model, cur) {
      last = model;
      current = cur;
      draw();
    },
  };
}
