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
import { screenText, Glyph, SessionRowView, fitSessionRows } from './screens';
export interface CatalogListSnapshot {
  browser?: {
    facets: HTMLElement;
    groupBy: string;
    working: number;
    waiting: number;
    groups: readonly { title: string; items: readonly CatalogListItem[] }[];
  };
  items: readonly CatalogListItem[];
  groups?: readonly {
    root: CatalogListItem;
    children: readonly CatalogListItem[];
    expanded: boolean;
  }[];
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
  group?(value: string): void;
  session(item: CatalogListItem): void;
  filters(values: { harness: string; repo: string; model: string; q: string }): void;
  searchDraft?(query: string): void;
  clearFilters(): void;
  more(): void;
  children?(key: string, expanded: boolean): void;
  retry(): void;
  reviews?(): void;
  candidate?(candidate: CatalogSourceCandidate): void;
  candidateMore?(): void;
  candidateRetry?(): void;
}
/** Cached metadata reports its scope, freshness and recorded status without live control. */
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
      <div class="ph" data-catalog-observation={snapshot.observation ?? 'Cached history'}>
        <h1>Sessions</h1>
        {snapshot.browser ? (
          <div class="sub">
            <span>
              <b>{snapshot.items.length}</b>sessions loaded
            </span>
            <span>
              <b>{snapshot.browser.working}</b>working
            </span>
            <span>
              <b>{snapshot.browser.waiting}</b>waiting on you
            </span>
          </div>
        ) : (
          <p class="sub">
            {screenText(snapshot.sourceLabel)} · {snapshot.observation ?? 'Cached history'}
          </p>
        )}
      </div>
      {snapshot.browser ? (
        <>
          <div
            class="session-facet-slot"
            ref={(node) => {
              const facet = snapshot.browser!.facets;
              if (node && facet.parentElement !== node) node.append(facet);
            }}
          />
          {snapshot.metadataSearch && (
            <form
              onSubmit={(event) => {
                event.preventDefault();
                const input = event.currentTarget.querySelector<HTMLInputElement>('input')!;
                host.filters({
                  q: input.value.trim(),
                  harness: snapshot.harness,
                  repo: snapshot.repo,
                  model: snapshot.model ?? '',
                });
              }}
            >
              <label class="find">
                <Glyph path="M11 18a7 7 0 1 1 0-14 7 7 0 0 1 0 14zM20 20l-4-4" />
                <input
                  id="sq"
                  name="q"
                  type="search"
                  aria-label="Search session details"
                  placeholder="Search sessions"
                  value={snapshot.query ?? ''}
                  onInput={(event) => host.searchDraft?.(event.currentTarget.value)}
                  onChange={(event) =>
                    host.filters({
                      q: event.currentTarget.value.trim(),
                      harness: snapshot.harness,
                      repo: snapshot.repo,
                      model: snapshot.model ?? '',
                    })
                  }
                />
              </label>
            </form>
          )}
          <div class="groupby" role="group" aria-label="Group by">
            {[
              ['recent', 'Recent'],
              ['project', 'Project'],
              ['machine', 'Machine'],
              ['harness', 'Harness'],
            ].map(([key, label]) => (
              <button
                type="button"
                key={key}
                data-g={key}
                aria-pressed={snapshot.browser!.groupBy === key}
                onClick={() => host.group?.(key)}
              >
                {label}
              </button>
            ))}
          </div>
        </>
      ) : (
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
                onInput={(event) => host.searchDraft?.(event.currentTarget.value)}
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
      )}
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
      {snapshot.browser ? (
        <div class="sess session-groups session-list" aria-label="Recorded sessions">
          {snapshot.browser.groups.map((group) => (
            <div key={group.title} class="session-group">
              {group.title && (
                <div class="sec-h">
                  {screenText(group.title)}
                  <span class="n">{group.items.length}</span>
                </div>
              )}
              <div class="list">
                {group.items.map((item) => (
                  <SessionRowView
                    key={item.key}
                    row={item.presentation!}
                    host={{ session: () => host.session(item) }}
                  />
                ))}
              </div>
            </div>
          ))}
        </div>
      ) : (
        <div class="session-list" aria-label="Recorded sessions">
          {(
            snapshot.groups ??
            snapshot.items.map((root) => ({ root, children: [], expanded: false }))
          ).map((group) => {
            const shown =
              group.expanded || filtered
                ? group.children.length
                : Math.min(5, group.children.length);
            const row = (item: CatalogListItem, child = false) => (
              <SessionRowView
                key={item.key}
                child={child}
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
            );
            return (
              <div class={group.children.length ? 'kids-wrap' : ''} key={group.root.key}>
                {row(group.root)}
                {!!group.children.length && (
                  <div class="kids">
                    {group.children.slice(0, shown).map((item) => row(item, true))}
                    {!filtered && group.children.length > 5 && (
                      <button
                        class="tree-more"
                        type="button"
                        aria-expanded={group.expanded}
                        onClick={() => host.children?.(group.root.key, !group.expanded)}
                      >
                        {group.expanded
                          ? 'Show fewer'
                          : 'Show ' + (group.children.length - shown) + ' more'}
                      </button>
                    )}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
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
