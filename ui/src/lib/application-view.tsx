import { render, type VNode } from 'preact';
import type { ComponentChildren } from 'preact';
/** A single declarative viewer page tree; native slots and measured subroots are separate owners. */
export function ApplicationView({ children }: { children: ComponentChildren }) {
  return <>{children}</>;
}
export function commitApplicationView(root: HTMLElement, view: VNode) {
  render(<ApplicationView>{view}</ApplicationView>, root);
}
