// Compiled consumer contract; no viewer, fetch or routing dependency.
import { AccountMenu, parseAccount, safePath, type Account, type AccountMenuProps } from './lib';
const account: Account = { name: 'Ada', login: 'ada', initials: 'A', avatar_href: null, workspaces: [], links: [] };
const props: AccountMenuProps = { account, compact: false, wide: false, onWideChange: () => {} };
export const fixture = <AccountMenu account={props.account} compact={props.compact} wide={props.wide} onWideChange={props.onWideChange} />;
export const parsed = parseAccount(account);
export const path = safePath('/account');
