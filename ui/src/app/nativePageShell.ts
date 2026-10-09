import type { ViewerHost } from '../viewer-host';
import type { ViewerApplication } from './viewer';
import { createShellChrome } from '../lib/shell';
import { parseAccount } from '../lib/account';
import {
  projectShellNavigation,
  readShellPreference,
  writeShellPreference,
} from '../lib/navigation';
import { setGeometry } from '../lib';
import { EffectScope } from './effects';
import { I } from './registry';

/** Native content stays host-owned. Chrome never needs a complete history model. */
export function createNativePageShell(host: ViewerHost): ViewerApplication {
  const scope = new EffectScope(),
    account = parseAccount(host.account);
  const app = document.querySelector<HTMLElement>('.app')!;
  let rail = readShellPreference('rail'),
    wide = readShellPreference('wide');
  let destinations = projectShellNavigation(
    host.navigation ?? { leading: host.nativeNavigation },
    host.nativePage?.nav ?? 'machines',
  );
  const shell = createShellChrome({
    account: {
      place(widget, trigger) {
        const at = trigger.getBoundingClientRect();
        setGeometry(widget, 'accountLeft', at.left);
        setGeometry(widget, 'accountWidth', at.width);
        setGeometry(widget, 'accountBottom', Math.max(0, innerHeight - at.top + 6));
      },
      opened() {},
      closed() {},
      navigate() {
        return false;
      },
      submit() {
        return false;
      },
    },
    navigate() {
      return false;
    },
    drawerOpened() {},
    drawerClosed() {},
    railChanged() {
      rail = !rail;
      writeShellPreference('rail', rail);
      shell.update(destinations, rail);
    },
  });
  const refresh = () => {
    destinations = projectShellNavigation(
      host.navigation ?? { leading: host.nativeNavigation },
      host.nativePage?.nav ?? 'machines',
    );
    shell.update(destinations, rail);
  };
  const removeNavigation = host.subscribeNavigation?.(refresh);
  const title = app.querySelector<HTMLElement>('#topbar .ttl') ?? document.createElement('div');
  title.className = 'ttl';
  if (!title.querySelector('.l1')) {
    const line = document.createElement('div');
    line.className = 'l1';
    line.append(...title.childNodes);
    title.append(line);
  }
  shell.mount(app);
  shell.update(destinations, rail);
  const accountProps = account
    ? {
        account,
        compact: false,
        wide,
        onWideChange() {
          wide = !wide;
          writeShellPreference('wide', wide);
          shell.account.updateWide(wide);
        },
      }
    : null;
  shell.topbar({
    titleSlot: title,
    lead: { label: 'Open menu', icon: I.menu },
    account: accountProps,
  });
  shell.drawerAccount(accountProps ? { ...accountProps, compact: true } : null);
  scope.listen(document, 'keydown', (event) => {
    if (event.key === 'Escape') {
      if (shell.account.open) shell.account.escape();
      else shell.closeDrawer();
    }
  });
  return {
    destroy() {
      removeNavigation?.();
      scope.destroy();
      shell.destroy();
    },
  };
}
