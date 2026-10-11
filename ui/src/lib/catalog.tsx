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
  presentation?: import('./screens').SessionRow;
}
import { screenText, SessionRowView, fitSessionRows } from './screens';
export interface CatalogListSnapshot {
  items: readonly CatalogListItem[];
  sourceLabel: string;
  updating: boolean;
  observation?: string;
  discovering?: boolean;
  note: string;
  harness: string;
  repo: string;
  model?: string;
  query?: string;
  metadataSearch?: boolean;
  searchPartial?: boolean;
  searchIndexIncomplete?: boolean;
  more: boolean;
  reviews?: boolean;
  hasReviews?: boolean;
  candidates?: readonly CatalogSourceCandidate[];
  candidatesMore?: boolean;
  candidateUpdating?: boolean;
  candidateNote?: string;
}
export interface CatalogListHost {
  session(item: CatalogListItem): void;
  filters(values: { harness: string; repo: string; model: string; q: string }): void;
  clearFilters(): void;
  more(): void;
  retry(): void;
  reviews?(): void;
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
  const filtered = !!(snapshot.query || snapshot.harness || snapshot.repo || snapshot.model);
  function commit(form: HTMLFormElement) {
    const values = new FormData(form);
    host.filters({
      q: String(values.get('q') ?? '').trim(),
      model: String(values.get('model') ?? '').trim(),
      harness: String(values.get('harness') ?? '').trim(),
      repo: values.get('no_repo') === 'on' ? '__none__' : String(values.get('repo') ?? '').trim(),
    });
  }
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
          commit(event.currentTarget);
        }}
      >
        {snapshot.metadataSearch && (
          <label class="search catalog-query" key="q">
            <span>Search</span>
            <input
              name="q"
              aria-label="Search session details"
              id="sq"
              placeholder="Session details…"
              value={snapshot.query ?? ''}
              onChange={(event) => {
                if (event.currentTarget.form) commit(event.currentTarget.form);
              }}
            />
          </label>
        )}
        <label class="search" key="harness">
          <span>Harness</span>
          <input
            name="harness"
            aria-label="Harness"
            value={snapshot.harness}
            onChange={(event) => {
              if (event.currentTarget.form) commit(event.currentTarget.form);
            }}
          />
        </label>
        <label class="search" key="repo">
          <span>Repository</span>
          <input
            name="repo"
            aria-label="Repository"
            value={snapshot.repo === '__none__' ? '' : snapshot.repo}
            onChange={(event) => {
              if (event.currentTarget.form) commit(event.currentTarget.form);
            }}
          />
        </label>
        <label class="search" key="no_repo">
          <input
            name="no_repo"
            type="checkbox"
            checked={snapshot.repo === '__none__'}
            onChange={(event) => {
              if (event.currentTarget.form) commit(event.currentTarget.form);
            }}
          />
          <span>No repository</span>
        </label>
        <label class="search" key="model">
          <span>Model</span>
          <input
            name="model"
            aria-label="Model"
            value={snapshot.model ?? ''}
            onChange={(event) => {
              if (event.currentTarget.form) commit(event.currentTarget.form);
            }}
          />
        </label>
        <button class="link catalog-apply" type="submit">
          Apply filters
        </button>
      </form>
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
      {snapshot.hasReviews && (
        <button
          class="link"
          type="button"
          data-show-approval-reviews=""
          aria-pressed={snapshot.reviews ?? false}
          onClick={() => host.reviews?.()}
        >
          Show approval reviews
        </button>
      )}
      <div class="catalog-result-header">
        <span>
          {snapshot.items.length}{' '}
          {snapshot.items.length === 1 ? 'session loaded' : 'sessions loaded'}
          <span class="catalog-sort"> · Latest recorded activity</span>
        </span>
        {filtered && (
          <button class="link" type="button" onClick={() => host.clearFilters()}>
            Clear filters
          </button>
        )}
        <details class="catalog-scope">
          <summary>Search scope</summary>
          <p class="catalog-note">
            {snapshot.metadataSearch
              ? 'Search includes names, IDs, repositories, branches, models and harnesses. Conversation text and history across sources are unavailable.'
              : 'Text search and history across sources are unavailable.'}
          </p>
        </details>
      </div>
      <div class="session-list" aria-label="Recorded sessions">
        {snapshot.items.map((item) => (
          <SessionRowView
            key={item.key}
            row={
              item.presentation ?? {
                id: item.key,
                name: item.name,
                state: 'off',
                stateLabel: 'Recorded history',
                age: item.freshness.state,
                model: item.model,
                modelTip: item.model,
                fields: [],
              }
            }
            host={{ session: () => host.session(item) }}
          />
        ))}
      </div>
      {!snapshot.updating &&
        !snapshot.items.length &&
        !snapshot.note &&
        !snapshot.discovering &&
        !snapshot.searchIndexIncomplete &&
        !snapshot.searchPartial &&
        !snapshot.candidates?.length &&
        !snapshot.candidateUpdating && (
          <p class="catalog-note">
            {filtered ? (
              'No recorded sessions match these filters.'
            ) : (
              <>
                No recorded sessions in this source yet.{' '}
                <button class="link" type="button" onClick={() => host.retry()}>
                  Refresh history
                </button>
              </>
            )}
          </p>
        )}
      {snapshot.more && (
        <button class="link" type="button" disabled={snapshot.updating} onClick={() => host.more()}>
          Load more sessions
        </button>
      )}
    </>,
    root,
  );
  fitSessionRows(root);
}
