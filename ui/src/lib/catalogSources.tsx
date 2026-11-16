import { render } from 'preact';
import { screenText } from './screens';
export function renderCatalogSources(
  root: HTMLElement,
  view: {
    items: readonly { source_key: string; label: string }[];
    updating: boolean;
    note: string;
    more: boolean;
    machinesHref: string;
    pair?(): void;
  },
  host: { select(key: string): void; more(): void; retry(): void },
) {
  const emptyReceiver = !view.items.length && !view.updating && !view.note && !!view.pair;
  render(
    <>
      <div class="ph">
        <h1>{emptyReceiver ? 'Pair your first machine' : 'Choose a machine'}</h1>
        <p class="sub">
          {emptyReceiver
            ? 'Run one command on each machine whose sessions you want here. Sync starts automatically.'
            : 'Browse recorded history from one authorized source.'}
        </p>
      </div>
      {view.note && (
        <p class="catalog-note" role="status">
          {screenText(view.note)}{' '}
          <button class="link" type="button" onClick={() => host.retry()}>
            Retry
          </button>
        </p>
      )}
      {view.updating && (
        <p class="catalog-note" role="status">
          Loading machines…
        </p>
      )}
      <div class="session-list">
        {view.items.map((item) => (
          <button
            class="nrow"
            type="button"
            key={item.source_key}
            data-source-key={item.source_key}
            onClick={() => host.select(item.source_key)}
          >
            <span class="nm">{screenText(item.label)}</span>
          </button>
        ))}
      </div>
      {!view.items.length && !view.updating && !view.note && (
        <div class="catalog-note">
          {!view.pair && <p>No authorized recorded sources are available.</p>}
          {view.pair && (
            <button type="button" class="btn sh-compact" onClick={view.pair}>
              Pair a machine
            </button>
          )}
        </div>
      )}
      {view.more && (
        <button class="link" type="button" disabled={view.updating} onClick={() => host.more()}>
          Load more machines
        </button>
      )}
      {!emptyReceiver && (
        <p class="catalog-note">
          <a class="link" href={view.machinesHref}>
            Manage machines
          </a>
        </p>
      )}
    </>,
    root,
  );
}
