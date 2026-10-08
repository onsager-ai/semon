import { render } from 'preact';
import type { ViewerApplication } from './viewer';
import type { ViewerHost } from '../viewer-host';
import { createShellChrome } from '../lib/shell';
import { renderCatalogSources } from '../lib/catalogSources';
import { parseCatalogSources, type CatalogSourceItem } from '../state/catalog-sources';
import { requestJson } from '../lib/model';
import { EffectScope } from './effects';
import { I } from './registry';
import { parseAccount } from '../lib/account';
import { setGeometry } from '../lib';
/** Initial source discovery reads only authorized metadata, never a workspace model or native pane. */
export function createCatalogSources(
  host: ViewerHost | null,
  select: (key: string) => void,
): ViewerApplication {
  const scope = new EffectScope();
  let disposed = false,
    epoch = 0,
    items: CatalogSourceItem[] = [],
    cursor: string | null = null,
    updating = false,
    note = '',
    retry: number | undefined,
    delay = 1000;
  const account = parseAccount(host?.account);
  const navigation = [
    { key: 'sessions', label: 'Sessions', href: '/sessions', icon: I.sessions, current: true },
    {
      key: 'machines',
      label: 'Machines',
      href: host?.machinesPath ?? '/machines?compat=1',
      icon: I.machines,
      current: false,
    },
    ...(host?.nativeNavigation ?? []).filter(
      (destination) => !['sessions', 'machines'].includes(destination.key),
    ),
  ];
  let wide = document.querySelector('#page')?.classList.contains('wide-mode') ?? false,
    rail = document.querySelector('.app')?.classList.contains('rail') ?? false;
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
      shell.update(navigation, rail);
    },
  });
  shell.mount(document.querySelector<HTMLElement>('.app')!);
  shell.update(navigation, rail);
  const title = document.createElement('div');
  title.className = 'ttl';
  title.textContent = 'Choose a machine';
  function wideChange() {
    wide = !wide;
    document.querySelector('#page')?.classList.toggle('wide-mode', wide);
    shell.account.updateWide(wide);
  }
  shell.topbar({
    titleSlot: title,
    lead: { label: 'Open menu', icon: I.menu },
    account: account ? { account, compact: false, wide, onWideChange: wideChange } : null,
  });
  shell.drawerAccount(account ? { account, compact: true, wide, onWideChange: wideChange } : null);
  const root = shell.slots.content;
  function draw() {
    if (disposed) return;
    renderCatalogSources(
      root,
      {
        items,
        updating,
        note,
        more: cursor !== null,
        machinesHref: host?.machinesPath ?? '/machines?compat=1',
      },
      {
        select(key) {
          if (!disposed && items.some((item) => item.source_key === key)) select(key);
        },
        more() {
          void load(true);
        },
        retry() {
          void load(false);
        },
      },
    );
  }
  async function load(append: boolean) {
    if (disposed || (append && updating)) return;
    scope.clearTimeout(retry);
    const ticket = ++epoch,
      request = scope.request(),
      deadline = scope.timeout(() => request.abort(), 15000),
      params = new URLSearchParams({ limit: '60' });
    if (append && cursor !== null) params.set('cursor', cursor);
    updating = true;
    note = '';
    draw();
    try {
      const page = parseCatalogSources(
        await requestJson('/api/session-sources?' + params, request.signal),
      );
      if (disposed || ticket !== epoch) return;
      if (
        append &&
        page.items.some((item) => items.some((old) => old.source_key === item.source_key))
      )
        throw Error('Source inventory repeated a machine');
      items = append ? [...items, ...page.items] : page.items;
      cursor = page.next_cursor;
      delay = 1000;
    } catch (error) {
      if (disposed || ticket !== epoch) return;
      const status =
        error && typeof error === 'object' && 'status' in error ? Number(error.status) : 0;
      const transient =
        (error && typeof error === 'object' && 'status' in error) ||
        (error instanceof Error && error.name === 'AbortError');
      note = !transient
        ? 'The source returned an invalid inventory. Check this source and retry.'
        : status === 401 || status === 403
          ? 'Authorization is required to browse these machines.'
          : status === 404
            ? 'Bounded source discovery is unavailable. Choose a machine from its management page.'
            : status === 400
              ? 'The source inventory cursor is unavailable. Refresh this list.'
              : 'Source inventory is unavailable. Retrying…';
      if (append && status === 400) {
        void load(false);
        return;
      }
      if (transient && (status === 0 || [429, 500, 502, 503, 504].includes(status))) {
        retry = scope.timeout(() => void load(append), delay);
        delay = Math.min(8000, delay * 2);
      }
    } finally {
      scope.clearTimeout(deadline);
      scope.releaseRequest(request);
      if (!disposed && ticket === epoch) {
        updating = false;
        draw();
      }
    }
  }
  scope.listen(document, 'keydown', (event) => {
    if (event.key === 'Escape') shell.closeDrawer();
  });
  scope.listen(window, 'focus', () => void load(false));
  void load(false);
  return {
    destroy() {
      if (disposed) return;
      disposed = true;
      ++epoch;
      scope.destroy();
      render(null, root);
      shell.destroy();
    },
  };
}
