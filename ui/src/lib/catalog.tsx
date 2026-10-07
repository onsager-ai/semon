import { render } from 'preact';
export interface CatalogListItem {
  key: string;
  name: string;
  harness: string;
  repo: string | null;
  branch: string | null;
  model: string;
  freshness: { state: string };
}
import { screenText } from './screens';
export interface CatalogListSnapshot {
  items: readonly CatalogListItem[];
  sourceLabel: string;
  updating: boolean;
  note: string;
  harness: string;
  repo: string;
  more: boolean;
  compatibilityHref: string;
}
export interface CatalogListHost {
  session(item: CatalogListItem): void;
  filter(field: 'harness' | 'repo', value: string): void;
  more(): void;
  retry(): void;
}
/** Cached metadata reports its own scope/freshness and carries no runtime totals or state. */
export function renderCatalogList(
  root: HTMLElement,
  snapshot: CatalogListSnapshot,
  host: CatalogListHost,
) {
  render(
    <>
      <div class="ph">
        <h1>Sessions</h1>
        <p class="sub">{screenText(snapshot.sourceLabel)} · Cached history</p>
      </div>
      <form
        class="catalog-filters facet-filters"
        onSubmit={(event) => {
          event.preventDefault();
        }}
      >
        <label class="search">
          Harness{' '}
          <input
            aria-label="Harness"
            value={snapshot.harness}
            onChange={(event) => host.filter('harness', event.currentTarget.value.trim())}
          />
        </label>
        <label class="search">
          Repository{' '}
          <input
            aria-label="Repository"
            value={snapshot.repo}
            onChange={(event) => host.filter('repo', event.currentTarget.value.trim())}
          />
        </label>
      </form>
      <p class="catalog-note">
        Sorted by latest recorded activity. Text search and history across sources are unavailable.
      </p>
      {snapshot.note && (
        <p class="catalog-note" role="status">
          {snapshot.note}{' '}
          <button type="button" class="link" onClick={() => host.retry()}>
            Retry
          </button>
        </p>
      )}
      {snapshot.updating && (
        <p class="catalog-note" role="status">
          Updating history…
        </p>
      )}
      <div class="session-list">
        {snapshot.items.map((item) => (
          <button
            key={item.key}
            class="nrow"
            type="button"
            data-session-row="list"
            data-id={item.key}
            onClick={() => host.session(item)}
          >
            <span class="session-row-main srow-main">
              <span class="nm">{screenText(item.name)}</span>
              <span class="ag">{item.freshness.state}</span>
            </span>
            <span class="session-row-meta srow-meta for">
              {screenText(
                [item.harness, item.repo, item.branch, item.model].filter(Boolean).join(' · '),
              )}
            </span>
          </button>
        ))}
      </div>
      <p class="catalog-note">
        <a class="link" href={snapshot.compatibilityHref}>
          Open compatibility view (loads workspace history)
        </a>
      </p>
      {!snapshot.updating && !snapshot.items.length && !snapshot.note && (
        <p class="catalog-note">No recorded sessions match these filters.</p>
      )}
      {snapshot.more && (
        <button class="link" type="button" disabled={snapshot.updating} onClick={() => host.more()}>
          Load more sessions
        </button>
      )}
    </>,
    root,
  );
}
