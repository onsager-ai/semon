import { sessionPath } from './sessionRoute';
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
    bookmarks?: readonly {
      source_key: string;
      catalog_key: string;
      harness: string;
      href?: string;
    }[];
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
      {!!view.bookmarks?.length && (
        <div class="session-list" aria-label="Matching session origins">
          {view.bookmarks.map((item) => (
            <a
              class="nrow"
              key={JSON.stringify([item.source_key, item.catalog_key])}
              href={item.href ?? sessionPath(item.source_key, item.catalog_key)}
            >
              <span class="nm">{screenText(item.catalog_key)}</span>
              <span>
                {screenText(
                  view.items.find((source) => source.source_key === item.source_key)?.label ??
                    item.source_key,
                )}
              </span>
            </a>
          ))}
        </div>
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
        <a class="link" href={view.machinesHref}>
          Manage machines
        </a>
      </p>
    </>,
    root,
  );
}
