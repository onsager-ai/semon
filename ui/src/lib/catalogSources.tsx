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
  },
  host: { select(key: string): void; more(): void; retry(): void },
) {
  render(
    <>
      <div class="ph">
        <h1>Choose a machine</h1>
        <p class="sub">Browse recorded history from one authorized source.</p>
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
        <p class="catalog-note">No authorized recorded sources are available.</p>
      )}
      {view.more && (
        <button class="link" type="button" disabled={view.updating} onClick={() => host.more()}>
          Load more machines
        </button>
      )}
      <p class="catalog-note">
        This source list contains names and keys only. History across sources and text search are
        unavailable.
      </p>
      <p class="catalog-note">
        <a class="link" href={view.machinesHref}>
          Manage machines
        </a>
        {' · '}
        <a class="link" href="/sessions?compat=1">
          Open compatibility view (loads workspace history)
        </a>
      </p>
    </>,
    root,
  );
}
