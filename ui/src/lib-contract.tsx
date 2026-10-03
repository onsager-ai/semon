// Compiled consumer contract; no viewer, fetch or routing dependency.
import { AccountMenu, parseAccount, safePath, type Account, type AccountMenuProps } from './lib';
const account: Account = { name: 'Ada', login: 'ada', initials: 'A', avatar_href: null, workspaces: [], links: [] };
const props: AccountMenuProps = { account, compact: false, wide: false, onWideChange: () => {} };
export const fixture = <AccountMenu account={props.account} compact={props.compact} wide={props.wide} onWideChange={props.onWideChange} />;
export const parsed = parseAccount(account);
export const path = safePath('/account');

// Independent shell consumer: the host selects placement/history and removes its own container.
import { createAccountChrome, type AccountChromeHost } from './lib';
export function mountAccountExample(container: HTMLElement, host: AccountChromeHost) {
  const chrome = createAccountChrome(host);
  const root = chrome.mount(props);
  container.append(root);
  return {
    chrome,
    destroy() { chrome.destroy(); root.remove(); },
  };
}

import { createShellChrome, type ShellHost } from './lib';
export function mountShellExample(container: HTMLElement, host: ShellHost) {
  const chrome = createShellChrome(host);
  chrome.mount(container);
  chrome.update([{ key: 'sessions', label: 'Sessions', href: '/sessions', icon: 'M4 5h16', current: true }], false);
  const title = document.createElement('div'); title.className = 'ttl'; title.textContent = 'Sessions';
  const action = document.createElement('button'); action.textContent = 'Host action';
  chrome.topbar({ titleSlot: title, actions: [action], lead: { label: 'Open navigation', icon: 'M4 7h16M4 12h16M4 17h16' }, account: props });
  chrome.drawerAccount({ ...props, compact: true });
  return { chrome, title, action, destroy() { chrome.destroy(); } };
}

import { createRecentRenderer, type RecentHost, type RecentSnapshot } from './lib';
export function mountRecentExample(container: HTMLElement, host: RecentHost, snapshot: RecentSnapshot) {
  const recent = createRecentRenderer(container, host); recent.update(snapshot); return recent;
}

export { createPanelChrome } from './lib';
export { createFacetChrome, renderSessionsScreen, releaseScreen, createMarkdown, renderSessionMenu, createImageViewer } from './lib';
export { createMeasuredLayout } from './lib';
