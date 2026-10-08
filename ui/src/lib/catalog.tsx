export interface CatalogSourceCandidate {
  candidate_key: string;
  source: { root: string; path: string };
  native_name_hint: string | null;
  generation: string;
  archive_observed_at: number | null;
}
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
  observation?: string;
  discovering?: boolean;
  note: string;
  harness: string;
  repo: string;
  query?: string;
  metadataSearch?: boolean;
  searchPartial?: boolean;
  searchIndexIncomplete?: boolean;
  more: boolean;
  compatibilityHref: string;
  candidates?: readonly CatalogSourceCandidate[];
  candidatesMore?: boolean;
  candidateUpdating?: boolean;
  candidateNote?: string;
}
export interface CatalogListHost {
  session(item: CatalogListItem): void;
  filter(field: 'harness' | 'repo' | 'q', value: string): void;
  more(): void;
  retry(): void;
  candidate?(candidate: CatalogSourceCandidate): void;
  candidateMore?(): void;
  candidateRetry?(): void;
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
        <p class="sub">
          {screenText(snapshot.sourceLabel)} · {snapshot.observation ?? 'Cached history'}
        </p>
      </div>
      <form
        class="catalog-filters facet-filters"
        onSubmit={(event) => {
          event.preventDefault();
        }}
      >
        {snapshot.metadataSearch && (
          <label class="search" key="q">
            Search session details{' '}
            <input
              aria-label="Search session details"
              value={snapshot.query ?? ''}
              onChange={(event) => host.filter('q', event.currentTarget.value)}
            />
          </label>
        )}
        <label class="search" key="harness">
          Harness{' '}
          <input
            aria-label="Harness"
            value={snapshot.harness}
            onChange={(event) => host.filter('harness', event.currentTarget.value.trim())}
          />
        </label>
        <label class="search" key="repo">
          Repository{' '}
          <input
            aria-label="Repository"
            value={snapshot.repo}
            onChange={(event) => host.filter('repo', event.currentTarget.value.trim())}
          />
        </label>
      </form>
      <p class="catalog-note">
        Sorted by latest recorded activity.{' '}
        {snapshot.metadataSearch
          ? 'Search includes names, IDs, repositories, branches, models and harnesses. Conversation text and history across sources are unavailable.'
          : 'Text search and history across sources are unavailable.'}
      </p>
      {snapshot.searchPartial && (
        <p class="catalog-note" role="status">
          Search checked a bounded part of the index. Load more sessions to continue looking for
          matches.
        </p>
      )}
      {snapshot.searchIndexIncomplete && (
        <p class="catalog-note" role="status">
          Search is incomplete while recorded sessions are being indexed.
        </p>
      )}
      {snapshot.discovering && (
        <p class="catalog-note" role="status">
          More sessions are being discovered. This list is incomplete.
        </p>
      )}
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
      {(snapshot.candidates?.length || snapshot.candidateNote || snapshot.candidateUpdating) && (
        <section aria-label="Archived sources awaiting sessions">
          <h2>Archived sources</h2>
          <p class="catalog-note">
            These recorded sources have not been indexed as sessions yet. Open a source to read it.
          </p>
          {snapshot.candidateNote && (
            <p class="catalog-note" role="status">
              {snapshot.candidateNote}{' '}
              <button class="link" type="button" onClick={() => host.candidateRetry?.()}>
                Retry archived sources
              </button>
            </p>
          )}
          {snapshot.candidateUpdating && (
            <p class="catalog-note" role="status">
              Preparing recorded session…
            </p>
          )}
          {snapshot.candidates?.map((candidate) => (
            <button
              class="nrow"
              type="button"
              key={candidate.candidate_key}
              disabled={snapshot.candidateUpdating}
              onClick={() => host.candidate?.(candidate)}
            >
              <span class="session-row-main srow-main">
                <span class="nm">
                  {screenText(candidate.native_name_hint ?? candidate.source.path)}
                </span>
                <span class="ag">Source hint</span>
              </span>
              <span class="session-row-meta srow-meta for">
                {screenText(candidate.source.root + ' · ' + candidate.source.path)}
                {candidate.archive_observed_at === null
                  ? ' · Archive verification time unknown'
                  : ' · Archive verified ' + new Date(candidate.archive_observed_at).toISOString()}
              </span>
            </button>
          ))}
          {snapshot.candidatesMore && (
            <button
              class="link"
              type="button"
              disabled={snapshot.candidateUpdating}
              onClick={() => host.candidateMore?.()}
            >
              Load more archived sources
            </button>
          )}
        </section>
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
      {!snapshot.updating &&
        !snapshot.items.length &&
        !snapshot.note &&
        !snapshot.discovering &&
        !snapshot.searchIndexIncomplete &&
        !snapshot.searchPartial &&
        !snapshot.candidates?.length &&
        !snapshot.candidateUpdating && (
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
