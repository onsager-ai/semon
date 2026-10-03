import { render } from 'preact';
import { createShellChrome, createSelect, createNativeSheet, createPanelChrome, type AccountMenuProps } from '../../src/lib';

const app = document.querySelector<HTMLElement>('.app')!;
const chrome = createShellChrome({
  account: { place() {}, opened() {}, closed() {}, navigate() { return true; }, submit() { return true; } },
  navigate() { return true; }, drawerOpened() {}, drawerClosed() {}, railChanged() {},
});
chrome.mount(app);
chrome.update([{ key: 'gallery', label: 'Components', href: '/', icon: 'M4 5h16M4 12h16M4 19h16', current: true }], false);
const account: AccountMenuProps = { account: {
  name: 'Ada Example', login: '@ada', initials: 'AE', avatar_href: null,
  workspaces: [{ name: 'A workspace with a deliberately long name', role: 'owner', current: true, switch_href: '/workspace' }],
  links: [{ label: 'Account', href: '/account', method: 'get', danger: false }, { label: 'Sign out', href: '/logout', method: 'post', danger: true }],
}, compact: false, wide: false, onWideChange() { chrome.account.updateWide(true); } };
const title = document.createElement('div'); title.className = 'ttl';
const text = document.createElement('span'); text.className = 't'; text.textContent = 'Components'; title.append(text);
chrome.topbar({ titleSlot: title, lead: { label: 'Open menu', icon: 'M4 7h16M4 12h16M4 17h16' }, account });
chrome.drawerAccount({ ...account, compact: true });
render(<>
  <section><h2 class="sec-h">Controls</h2><div class="btn-row">
    <button class="btn primary" type="button">Continue</button><button class="btn" type="button">Secondary</button>
    <button class="btn danger" type="button">Revoke</button><button class="btn" type="button" disabled>Unavailable</button>
  </div></section>
  <section><label class="field">Workspace name<input placeholder="Name" value="An intentionally long workspace name that wraps without sideways scrolling" /><span class="hint">A helper message stays with its field.</span></label>
    <label class="field">Email<input aria-invalid="true" aria-describedby="field-error" value="invalid" /><span id="field-error" class="error">Enter a valid email address.</span></label></section>
  <section><h2 class="sec-h">Status and notices</h2><div class="status"><span class="dot work" />Working</div>
    <div class="notice">A quiet informational message.</div><div class="notice sh-err" role="alert">An actionable error with a clear next step.</div></section>
  <section class="rows"><div class="row"><span class="dot idle" /><div class="row-main"><span class="sh-nm">A long session name that must fit the available space</span><span class="sh-meta">Secondary context in the same row</span></div></div></section>
  <section id="select-root"><h2 class="sec-h">Select</h2></section>
  <section><button id="sheet-trigger" class="btn" type="button" onClick={openSheet}>Open sheet</button> <button id="panel-trigger" class="btn" type="button" onClick={openPanel}>Open panel</button></section>
  <section aria-label="Loading" aria-busy="true" class="skeleton"><div class="sk-turn"><div class="sk-text"><span class="sk-line sk-w1" /><span class="sk-line sk-w3" /></div></div></section>
</>, chrome.slots.content);
const select = createSelect({ label: 'Order', options: [{ value: 'new', label: 'Newest first' }, { value: 'old', label: 'Oldest first' }], value: 'new' });
document.querySelector('#select-root')!.append(select.el);
function openSheet() {
  const trigger = document.querySelector<HTMLButtonElement>('#sheet-trigger')!;
  const sheet = createNativeSheet({ className: 'kids-sheet', label: 'Example sheet', heading: 'Example sheet', caption: 'A shared responsive dialog', closeLabel: 'Close sheet' }, { opened() {}, closed() { trigger.focus(); } });
  render(<p class="empty">No items match. Adjust your filters.</p>, sheet.body); sheet.cleanup(() => render(null, sheet.body)); sheet.show();
}
function openPanel() {
  const trigger = document.querySelector<HTMLButtonElement>('#panel-trigger')!;
  const panel = createPanelChrome({ title: 'Example panel', label: 'Example panel' }, { opened() {}, closed() { trigger.focus(); } });
  const content = document.createElement('p'); content.textContent = 'Details lead with what matters.'; panel.body.append(content); panel.show();
}
document.addEventListener('keydown', event => { if (event.key === 'Escape') { if (chrome.account.open) chrome.account.escape(); else if (!document.querySelector('dialog[open]')) chrome.closeDrawer(); } });
window.addEventListener('pagehide', () => { select.destroy(); chrome.destroy(); }, { once: true });
