import { Component } from 'preact';
import type { Account } from './account';

export interface AccountMenuProps {
  account: Account;
  compact: boolean;
  wide: boolean;
  onWideChange: () => void;
}
export class AccountAvatar extends Component<{ account: Account }, { failed: boolean }> {
  state = { failed: false };
  render() {
    const { account } = this.props;
    return <span class="account-avatar">{account.initials}{account.avatar_href && !this.state.failed &&
      <img alt="" src={account.avatar_href} onError={() => this.setState({ failed: true })} />}</span>;
  }
}
function Workspace({ workspace }: { workspace: Account['workspaces'][number] }) {
  return <form class="account-menu-form account-workspace-form" method="post" action={workspace.switch_href}>
    <button class="account-menu-row" type="submit" role="menuitem" aria-current={workspace.current ? 'page' : undefined}>
      <span class="account-row-main"><span class="account-workspace-name">{workspace.name}</span><span class="account-role">{workspace.role}</span></span>
      {workspace.current && <span class="account-check">✓</span>}
    </button>
  </form>;
}
function AccountLink({ link }: { link: Account['links'][number] }) {
  const cls = 'account-menu-row' + (link.danger ? ' danger' : '');
  return link.method === 'post'
    ? <form class="account-menu-form account-link-form" method="post" action={link.href}><button class={cls} type="submit" role="menuitem">{link.label}</button></form>
    : <a class={cls} role="menuitem" href={link.href}>{link.label}</a>;
}
/** Owns only the contents of .account-popover; no fetch, routing or page state. */
export function AccountMenu({ account, compact, wide, onWideChange }: AccountMenuProps) {
  return <>
    <div class="account-identity"><AccountAvatar account={account} /><span class="account-identity-text"><span class="account-name">{account.name}</span><span class="account-login-value">{account.login}</span></span></div>
    <section class="account-section"><div class="account-heading">Workspaces</div>
      {account.workspaces.map((workspace, i) => <Workspace key={`${workspace.switch_href}:${i}`} workspace={workspace} />)}
    </section>
    {!compact && <section class="account-section account-display"><div class="account-heading">Display</div>
      <button class="account-menu-row account-switch-row" type="button" role="menuitemcheckbox" aria-checked={wide} data-pref="wide" onClick={event => { event.stopPropagation(); onWideChange(); }}>
        <span class="account-row-main">Wide reading mode</span><span class="switch" aria-hidden="true"><span class="switch-knob" /></span>
      </button>
    </section>}
    {[false, true].map(danger => {
      const links = account.links.filter(link => link.danger === danger);
      return links.length ? <section key={String(danger)} class={'account-section account-links' + (danger ? ' account-danger' : '')}>
        {links.map((link, i) => <AccountLink key={`${link.method}:${link.href}:${i}`} link={link} />)}
      </section> : null;
    })}
  </>;
}
