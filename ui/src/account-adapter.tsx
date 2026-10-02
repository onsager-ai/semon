import { render } from 'preact';
import { AccountMenu, parseAccount, type AccountMenuProps } from './lib';

const roots = new Map<HTMLElement, AccountMenuProps>();
export { parseAccount };
export function mountAccountPopover(props: AccountMenuProps): HTMLElement {
  const root = document.createElement('div');
  root.className = 'menu account-popover';
  root.setAttribute('role', 'menu');
  root.setAttribute('aria-label', 'Account');
  roots.set(root, props);
  // Explicit render commits synchronously: callers may insert and focus immediately.
  render(<AccountMenu account={props.account} compact={props.compact} wide={props.wide} onWideChange={props.onWideChange} />, root);
  return root;
}
export function updateAccountWide(wide: boolean) {
  for (const [root, props] of roots) {
    const next = { ...props, wide };
    roots.set(root, next);
    render(<AccountMenu account={next.account} compact={next.compact} wide={next.wide} onWideChange={next.onWideChange} />, root);
  }
}
export function destroyAccountPopover(root: HTMLElement) {
  if (!roots.has(root)) return;
  render(null, root);
  roots.delete(root);
}
