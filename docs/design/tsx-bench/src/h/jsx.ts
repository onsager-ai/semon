// A JSX factory with no runtime to speak of: `<div class="x">text</div>` calls h(), which builds real DOM nodes with
// createElement, setAttribute and text nodes. Nothing is diffed; a live update rebuilds what changed and `patchList`
// swaps it in by key, as the viewer's `morph` and `morphTurns` do today. Text only ever goes in as text nodes, and there
// is no `style` or `title` attribute: the CSP has no 'unsafe-inline' for styles, and tips are data-tip.

export type Child = Node | string | number | boolean | null | undefined | readonly Child[];

type Attrs = {
  class?: string;
  id?: string;
  role?: string;
  type?: string;
  hidden?: boolean;
  tabIndex?: number;
  ref?: (node: Element) => void;
  children?: Child;
} & { [data: `data-${string}`]: string | number | undefined } & { [aria: `aria-${string}`]: string | number | boolean | undefined } & {
  [event: `on${string}`]: ((event: Event) => void) | undefined;
};

export type Component<P> = (props: P) => Element;

const SVG = "http://www.w3.org/2000/svg";
const SVG_TAGS = new Set(["svg", "path", "circle"]);

function append(parent: Node, child: Child): void {
  if (child == null || child === false || child === true) return;
  if (Array.isArray(child)) for (const c of child as readonly Child[]) append(parent, c);
  else if (child instanceof Node) parent.appendChild(child);
  else parent.appendChild(document.createTextNode(String(child)));
}

export function h(tag: string | Component<any>, props: Record<string, unknown> | null, ...children: Child[]): Element {
  if (typeof tag === "function") return tag({ ...props, children: children.length === 1 ? children[0] : children });
  const node = SVG_TAGS.has(tag) ? document.createElementNS(SVG, tag) : document.createElement(tag);
  for (const [key, value] of Object.entries(props ?? {})) {
    if (value == null || value === false) continue;
    if (key === "ref") (value as (n: Element) => void)(node);
    else if (key.startsWith("on") && typeof value === "function") node.addEventListener(key.slice(2).toLowerCase(), value as EventListener);
    else if (key === "hidden" && node instanceof HTMLElement) node.hidden = true;
    else if (key === "tabIndex" && node instanceof HTMLElement) node.tabIndex = Number(value);
    else if (key === "style" || key === "title") throw new Error("h: no " + key + " attribute");
    else node.setAttribute(key, value === true ? "" : String(value));
  }
  for (const c of children) append(node, c);
  return node;
}

// TypeScript finds the JSX types for `jsxFactory: h` in h.JSX.
export namespace h {
  export namespace JSX {
    export type Element = globalThis.Element;
    export interface IntrinsicElements {
      [tag: string]: Attrs & Record<string, unknown>;
    }
    export interface ElementChildrenAttribute {
      children: {};
    }
  }
}

/**
 * Puts `items` in `box` in order. A node already there with the same key and signature is kept as it is (with whatever
 * the reader opened in it); anything else is built. Nodes not in `items` go. Like the viewer's `placer` and `morph`.
 */
export function patchList(box: Element, items: ReadonlyArray<{ key: string; sig: string; build: () => Element }>): number {
  const old = new Map<string, Element>();
  for (const n of Array.from(box.children)) {
    const key = n.getAttribute("data-key");
    if (key != null) old.set(key, n);
  }
  let cur: ChildNode | null = box.firstChild, built = 0;
  for (const item of items) {
    let n = old.get(item.key);
    if (!n || n.getAttribute("data-sig") !== item.sig) {
      n = item.build();
      n.setAttribute("data-key", item.key);
      n.setAttribute("data-sig", item.sig);
      built++;
    }
    if (n === cur) cur = cur.nextSibling;
    else box.insertBefore(n, cur);
  }
  while (cur) {
    const next: ChildNode | null = cur.nextSibling;
    cur.remove();
    cur = next;
  }
  return built;
}
