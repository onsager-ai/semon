import {
  parseCatalogSourceCandidates,
  parseCatalogSourceCandidateProgress,
  type CatalogSourceCandidate,
} from '../state/catalog-source-candidates';
import { render } from 'preact';
import type { ControlView } from '../lib/control';
import type { ViewerApplication } from './viewer';
import { object } from '../domain/validate';
import type { Session } from '../domain/types';
import type { ViewerHost } from '../viewer-host';
import { parseCatalogCapabilities, type CatalogCapabilities } from '../state/catalog-capabilities';
import { parseCatalogPage, type CatalogPage, type CatalogSession } from '../state/catalog-wire';
import { CatalogSelectionStore, type CatalogSelectionRequest } from '../state/catalog-selection';
import { CatalogTranscriptStore } from '../state/catalog-transcript';
import { createShellChrome } from '../lib/shell';
import { createRecentRenderer } from '../lib/recent';
import { renderCatalogList } from '../lib/catalog';
import {
  renderSessionScreen,
  updateSessionControl,
  updateSessionPager,
  updateSessionRuntime,
  updateSessionJump,
} from '../lib/transcript';
import { releaseScreen } from '../lib/screens';
import { requestJson } from '../lib/model';
import { parseAccount } from '../lib/account';
import { setGeometry } from '../lib';
import { NavigationController } from '../navigation/routes';
import { ViewUpdates } from '../state/viewUpdates';
import { createControlObservation } from './controlObservation';
import { createRuntimeObservation } from './runtimeObservation';
import { createLocalControl } from './localControl';
import { EffectScope } from './effects';
import { catalogTranscriptBlocks } from './catalogTranscriptView';
import type { CatalogTranscriptEntry } from '../state/catalog-transcript-wire';
import { parseCatalogField } from '../state/catalog-field-wire';
import { I } from './registry';
import { parseCatalogSources, type CatalogSourceItem } from '../state/catalog-sources';
import { renderCatalogSources } from '../lib/catalogSources';
interface SelectedView {
  sourceKey: string;
  key: string;
  root: HTMLElement;
  store: CatalogTranscriptStore;
  meta: CatalogSession | null;
  scroll: number;
  loading: boolean;
  note: string;
  ticket?: CatalogSelectionRequest;
  identityLoading: boolean;
  lastControl?: ControlView;
  opening: boolean;
  retry?: number;
  retryDelay: number;
  renderedRevision: number;
  renderedNote: string;
  fields: Map<
    string,
    { generation: string; text?: string; next: number | null; loading: boolean; note: string }
  >;
  following: boolean;
  newCount: number;
  lastTotal: number | null;
  queuedAfter?: number | null;
}
/** The advertised bounded reader has no complete ModelStore or global content poller. */
export function createCatalogViewer(
  capabilities: CatalogCapabilities,
  viewerHost: ViewerHost | null,
): ViewerApplication {
  const scope = new EffectScope(),
    selection = new CatalogSelectionStore(),
    navigation = new NavigationController({}),
    updates = new ViewUpdates();
  let listRetry: number | undefined,
    listRetryDelay = 1000;
  let disposed = false,
    page: CatalogPage | null = null,
    items: CatalogSession[] = [],
    cursor: string | null = null,
    listEpoch = 0,
    updating = false,
    note = '',
    harness = '',
    repo = '',
    query = '',
    active: SelectedView | null = null;
  let candidates: CatalogSourceCandidate[] = [],
    candidateCursor: string | null = null,
    candidateLoaded = false,
    candidateUpdating = false,
    candidateNote = '',
    candidateEpoch = 0,
    candidateTimer: number | undefined;
  const selected = new Map<string, SelectedView>();
  const sourceViews = new Map<
    string,
    {
      page: CatalogPage | null;
      items: CatalogSession[];
      cursor: string | null;
      harness: string;
      repo: string;
      query: string;
      selectedKey: string | null;
    }
  >();
  let sourcesOpen = false,
    sourcesEpoch = 0,
    sourceItems: CatalogSourceItem[] = [],
    sourceCursor: string | null = null,
    sourcesUpdating = false,
    sourcesNote = '',
    sourcesRetryDelay = 1000;
  const cacheKey = (key: string) => JSON.stringify([capabilities.source_key, key]);
  const account = parseAccount(viewerHost?.account);
  let wide = document.querySelector('#page')?.classList.contains('wide-mode') ?? false,
    rail = document.querySelector('.app')?.classList.contains('rail') ?? false,
    chromeTitle: string | null = null,
    chromeSession = false;
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
    navigate(destination) {
      if (destination.key === 'sources') {
        void showSources();
        return true;
      }
      if (destination.key !== 'sessions') return false;
      sourcesOpen = false;
      goList();
      return true;
    },
    drawerOpened() {},
    drawerClosed() {},
    railChanged() {
      rail = !rail;
      chrome();
    },
  });
  shell.mount(document.querySelector<HTMLElement>('.app')!);
  const root = shell.slots.content,
    title = document.createElement('div'),
    jumpActions = document.createElement('div'),
    jumpSlot = document.createElement('span'),
    jumpTarget = document.createElement('div');
  title.className = 'ttl';
  jumpActions.className = 'viewer-bar-actions';
  jumpSlot.className = 'viewer-jump';
  jumpTarget.className = 'jump-wrap';
  jumpTarget.hidden = true;
  jumpSlot.append(jumpTarget);
  jumpActions.append(jumpSlot);
  const recent = createRecentRenderer(shell.slots.recent, {
    open(key) {
      void goSession(key);
    },
    toggle() {},
    all() {},
    fewer() {},
  });
  const control = createLocalControl(scope, () => {
    if (!active) return;
    updateSessionControl(active.root, controlFor(active));
  });
  function controlFor(view: SelectedView): ControlView | undefined {
    const identity = selection.selectedIdentity();
    const current =
      active === view &&
      identity?.catalog_key === view.key &&
      identity.owner_qualification !== 'provisional' &&
      identity.native_id
        ? control.view(identity.native_id)
        : undefined;
    if (current) view.lastControl = current;
    const runtime = runtimeFor(view)?.observation,
      terminal = runtime && runtime.state !== 'active';
    if ((current && !terminal) || !view.lastControl) return current;
    const previous = current ?? view.lastControl;
    return {
      ...previous,
      busy: false,
      uncertain: true,
      canReconnect: false,
      snapshot: {
        ...previous.snapshot,
        connected: false,
        reason: terminal
          ? 'Native controls are unavailable for this environment state.'
          : 'Current connection status is unavailable.',
        capabilities: {
          ...previous.snapshot.capabilities,
          input: false,
          steer: false,
          interrupt: false,
          commandApproval: false,
          fileApproval: false,
          questions: false,
        },
      },
      send: () => Promise.resolve(false),
      interrupt() {},
      answer() {},
      reconnect() {},
    };
  }
  const observation = createControlObservation({
    scope,
    navigation,
    updates,
    viewerHost,
    catalogSelection: selection,
    controlOwner: control,
    modelStore: {
      get sessions(): Record<string, Session> {
        throw new Error('Catalog status cannot access the complete model');
      },
    },
  });
  const runtimeObservation = createRuntimeObservation({
    scope,
    navigation,
    viewerHost,
    catalogRuntimeSelection: selection,
  });
  function runtimeFor(view: SelectedView) {
    const frame = runtimeObservation.view();
    return frame?.source_key === capabilities.source_key && frame.catalog_key === view.key
      ? frame
      : undefined;
  }
  const removeRuntimeListener = runtimeObservation.subscribe(() => {
    if (!active) return;
    updateSessionRuntime(active.root, runtimeFor(active));
    updateSessionControl(active.root, controlFor(active));
  });
  const api = async (path: string): Promise<unknown> => {
    const controller = scope.request(),
      deadline = scope.timeout(() => controller.abort(), 15000);
    try {
      return await requestJson(path, controller.signal);
    } finally {
      scope.clearTimeout(deadline);
      scope.releaseRequest(controller);
    }
  };
  const currentParams = () => new URLSearchParams({ machine: capabilities.source_key });
  const params = () =>
    new URLSearchParams({ machine: capabilities.source_key, scope: 'retained_history' });
  function chrome() {
    const name = sourcesOpen
      ? 'Choose a machine'
      : (active?.meta?.name ?? (active ? 'Session' : 'Sessions'));
    shell.update(
      [
        {
          key: 'sessions',
          label: 'Sessions',
          href: '/sessions',
          icon: I.sessions,
          current: !sourcesOpen,
        },
        {
          key: 'sources',
          label: 'Sources',
          href: '/sessions?choose_source=1',
          icon: I.sessions,
          current: sourcesOpen,
        },
        ...(viewerHost?.nativeNavigation ?? []).filter(
          (destination) => destination.key !== 'sessions',
        ),
      ],
      rail,
    );
    if (chromeTitle !== name || chromeSession !== !!active) {
      chromeTitle = name;
      chromeSession = !!active;
      title.textContent = name;
      shell.topbar({
        titleSlot: title,
        actions: active ? [jumpActions] : [],
        session: !!active,
        lead: { label: 'Open menu', icon: I.menu },
        account: account
          ? {
              account,
              compact: false,
              wide,
              onWideChange() {
                wide = !wide;
                root.classList.toggle('wide-mode', wide);
                shell.account.updateWide(wide);
              },
            }
          : null,
      });
      shell.drawerAccount(account ? { account, compact: true, wide, onWideChange() {} } : null);
    }
    recent.update({
      empty: !items.length,
      items: items.slice(0, 60).map((item) => ({
        id: item.key,
        name: item.name,
        label: item.name,
        state: '',
        stateLabel: 'Cached history',
        age: item.freshness.state,
        model: item.model,
        modelTip: item.model,
        fields: [],
        current: active?.key === item.key ? 'page' : undefined,
        rail: false,
        open: false,
        depth: 0,
        stuck: false,
      })),
    });
  }
  function drawList() {
    if (disposed || active || sourcesOpen) return;
    chrome();
    const focused = document.activeElement;
    const draft =
      focused instanceof HTMLInputElement && root.contains(focused)
        ? {
            node: focused,
            value: focused.value,
            start: focused.selectionStart,
            end: focused.selectionEnd,
          }
        : null;
    renderCatalogList(
      root,
      {
        items,
        sourceLabel: page?.machine_info.label ?? (capabilities.source_key || 'This source'),
        updating,
        candidates,
        candidatesMore: candidateCursor !== null,
        candidateUpdating,
        candidateNote,
        discovering: page?.completeness.state === 'partial',
        observation: {
          cached: 'Cached history',
          updating: 'Discovering history',
          stale: 'History observation is stale',
          incomplete: 'History is incomplete',
          unavailable: 'History observation is unavailable',
        }[page?.freshness ?? 'cached'],
        note,
        harness,
        repo,
        query,
        metadataSearch: capabilities.metadata_search,
        searchPartial: page?.search?.partial ?? false,
        searchIndexIncomplete: page?.search ? !page.search.index_complete : false,
        more: cursor !== null,
        compatibilityHref:
          '/sessions?compat=1' +
          (capabilities.source_key
            ? '&machine=' + encodeURIComponent(capabilities.source_key)
            : ''),
      },
      {
        session(item) {
          void goSession(
            item.key,
            items.find((row) => row.key === item.key),
          );
        },
        candidate(item) {
          void prepareCandidate(item);
        },
        candidateMore() {
          void loadCandidates(true);
        },
        candidateRetry() {
          void loadCandidates(false);
        },
        filter(field, value) {
          cancelCandidate();
          if (field === 'harness') harness = value;
          else if (field === 'q') query = value;
          else repo = value;
          void loadList(false);
        },
        more() {
          void loadList(true);
        },
        retry() {
          void loadList(false);
        },
      },
    );
    if (draft?.node.isConnected && document.activeElement === draft.node) {
      draft.node.value = draft.value;
      if (draft.start !== null && draft.end !== null)
        draft.node.setSelectionRange(draft.start, draft.end);
    }
  }
  async function loadList(append: boolean) {
    if (disposed || (append && updating)) return;
    scope.clearTimeout(listRetry);
    const epoch = ++listEpoch,
      p = params();
    p.set('limit', '60');
    if (harness) p.set('harness', harness);
    if (repo) p.set('repo', repo);
    if (query && capabilities.metadata_search) p.set('q', query);
    if (append && cursor !== null) p.set('cursor', cursor);
    updating = true;
    note = '';
    drawList();
    try {
      const next = parseCatalogPage(await api('/api/sessions?' + p));
      if (disposed || epoch !== listEpoch) return;
      if (
        (query && capabilities.metadata_search && next.search === null) ||
        next.machine !== capabilities.source_key ||
        next.read_scope !== 'retained_history' ||
        (append && page?.generation !== next.generation)
      )
        throw new Error('History changed; refresh this list before loading another page.');
      if (append && next.items.some((item) => items.some((old) => old.key === item.key)))
        throw new Error('History page repeated a session. Refresh this list.');
      listRetryDelay = 1000;
      page = next;
      if (capabilities.source_candidates && !candidateLoaded) void loadCandidates(false);
      cursor = next.next_cursor;
      items = append ? [...items, ...next.items] : next.items;
      updating = false;
      drawList();
      chrome();
      if (
        (next.completeness.state === 'partial' || next.search?.index_complete === false) &&
        items.length <= 60
      ) {
        const refresh = () => {
          if (disposed || epoch !== listEpoch || active || sourcesOpen) return;
          const focused = document.activeElement;
          if (
            focused instanceof HTMLElement &&
            root.contains(focused) &&
            focused.matches('input,textarea,select')
          ) {
            listRetry = scope.timeout(refresh, 1000);
            return;
          }
          void loadList(false);
        };
        listRetry = scope.timeout(refresh, 1000);
      }
    } catch (error) {
      if (disposed || epoch !== listEpoch) return;
      updating = false;
      if (
        append &&
        error &&
        typeof error === 'object' &&
        'status' in error &&
        error.status === 409
      ) {
        void loadList(false);
        return;
      }
      note = readError(error);
      drawList();
      if (retryable(error)) {
        listRetry = scope.timeout(() => void loadList(append), listRetryDelay);
        listRetryDelay = Math.min(8000, listRetryDelay * 2);
      }
    }
  }
  function cancelCandidate() {
    candidateEpoch++;
    scope.clearTimeout(candidateTimer);
    candidateUpdating = false;
  }
  async function loadCandidates(append: boolean) {
    if (!capabilities.source_candidates || disposed || (append && candidateUpdating)) return;
    cancelCandidate();
    const epoch = candidateEpoch,
      sourceKey = capabilities.source_key;
    candidateLoaded = true;
    candidateUpdating = true;
    candidateNote = '';
    drawList();
    const p = new URLSearchParams({ machine: sourceKey, limit: '60' });
    if (append && candidateCursor) p.set('cursor', candidateCursor);
    try {
      const result = parseCatalogSourceCandidates(await api('/api/session-source-candidates?' + p));
      if (disposed || epoch !== candidateEpoch || sourceKey !== capabilities.source_key) return;
      if (
        result.source_key !== sourceKey ||
        (append &&
          result.items.some((item) =>
            candidates.some((old) => old.candidate_key === item.candidate_key),
          ))
      )
        throw new Error('Invalid archive source scope');
      candidates = append ? [...candidates, ...result.items] : result.items;
      candidateCursor = result.next_cursor;
    } catch (error) {
      if (disposed || epoch !== candidateEpoch) return;
      candidateNote = readError(error);
    } finally {
      if (!disposed && epoch === candidateEpoch) {
        candidateUpdating = false;
        drawList();
      }
    }
  }
  async function prepareCandidate(candidate: CatalogSourceCandidate) {
    cancelCandidate();
    const epoch = candidateEpoch,
      sourceKey = capabilities.source_key;
    candidateUpdating = true;
    candidateNote = '';
    drawList();
    let delay = 1000;
    const poll = async () => {
      if (disposed || epoch !== candidateEpoch || sourceKey !== capabilities.source_key) return;
      try {
        const p = new URLSearchParams({
          machine: sourceKey,
          candidate: candidate.candidate_key,
          generation: candidate.generation,
        });
        const result = parseCatalogSourceCandidateProgress(
          await api('/api/session-source-candidate?' + p),
        );
        if (disposed || epoch !== candidateEpoch || sourceKey !== capabilities.source_key) return;
        if (
          result.source_key !== sourceKey ||
          result.candidate_key !== candidate.candidate_key ||
          result.generation !== candidate.generation
        )
          throw new Error('Archive source changed; resynchronize source hints');
        if (result.state === 'ready') {
          const metadata = parseCatalogPage(
            await api(
              '/api/sessions?' +
                new URLSearchParams({
                  machine: sourceKey,
                  scope: 'retained_history',
                  sid: result.catalog_key!,
                }),
            ),
          );
          if (disposed || epoch !== candidateEpoch || sourceKey !== capabilities.source_key) return;
          if (
            metadata.machine !== sourceKey ||
            metadata.read_scope !== 'retained_history' ||
            metadata.items.length !== 1 ||
            metadata.items[0].key !== result.catalog_key
          )
            throw new Error('Parsed archive session identity is unavailable');
          candidateUpdating = false;
          await goSession(result.catalog_key!, metadata.items[0]);
          return;
        }
        candidateNote =
          result.reason ??
          (result.state === 'updating'
            ? 'Recorded source is being indexed. Its session will open automatically.'
            : 'This archived source cannot currently be read.');
        if (result.state === 'unavailable' && !result.retryable) {
          candidateUpdating = false;
          drawList();
          return;
        }
        drawList();
        candidateTimer = scope.timeout(() => void poll(), delay);
        delay = Math.min(delay * 2, 8000);
      } catch (error) {
        if (disposed || epoch !== candidateEpoch) return;
        candidateNote = readError(error);
        candidateUpdating = retryable(error);
        drawList();
        if (candidateUpdating) {
          candidateTimer = scope.timeout(() => void poll(), delay);
          delay = Math.min(delay * 2, 8000);
        }
      }
    };
    await poll();
  }
  function readError(error: unknown): string {
    if (
      !(error && typeof error === 'object' && 'status' in error) &&
      !(error instanceof Error && error.name === 'AbortError')
    )
      return 'The source returned an invalid history view. Retry after checking this source.';
    const status = error && typeof error === 'object' && 'status' in error ? error.status : 0;
    if (status === 401 || status === 403)
      return 'Authorization is required to read this source. Reconnect your account and retry.';
    if (status === 400) return 'The requested filter or range is unsupported by this source.';
    if (status === 404) return 'This recorded session is unavailable from the selected source.';
    if (status === 409) return 'Recorded history changed. Resynchronizing…';
    if (status === 503) return 'The source is updating or temporarily unavailable. Retrying…';
    if (status === 0) return 'Source observation is unavailable. Retrying…';
    return 'History could not be read. Retry after checking your connection.';
  }
  function retryable(error: unknown) {
    if (!(error && typeof error === 'object' && 'status' in error))
      return error instanceof Error && error.name === 'AbortError';
    const status = error && typeof error === 'object' && 'status' in error ? error.status : 0;
    return (
      status === 0 ||
      status === 429 ||
      status === 500 ||
      status === 502 ||
      status === 503 ||
      status === 504
    );
  }
  function preserveScroll() {
    if (active)
      active.scroll = window.matchMedia('(max-width: 760px)').matches
        ? window.scrollY
        : document.querySelector<HTMLElement>('#main')!.scrollTop;
  }
  function goList(push = true) {
    cancelCandidate();
    sourcesOpen = false;
    preserveScroll();
    active = null;
    selection.clear();
    navigation.route = { v: 'sessions' };
    if (push)
      history.pushState(
        null,
        '',
        '/sessions' +
          (capabilities.source_key
            ? '?machine=' + encodeURIComponent(capabilities.source_key)
            : ''),
      );
    root.replaceChildren();
    drawList();
    if (page?.completeness.state === 'partial' && items.length <= 60 && !updating)
      void loadList(false);
    shell.closeDrawer(true);
  }
  function drawSelected(view: SelectedView) {
    if (disposed || active !== view) return;
    const current = view.store.selectedPage(),
      identity = selection.selectedIdentity();
    if (!current) {
      renderSessionScreen(
        view.root,
        {
          id: view.key,
          name: view.meta?.name ?? 'Session',
          control: controlFor(view),
          runtime: runtimeFor(view),
          blocks: [],
          order: [],
          observation: 'Recorded history is updating',
          empty:
            view.note ||
            (capabilities.selected_transcript
              ? 'Loading recorded session…'
              : 'Recorded transcript is updating. The source refresh will appear automatically.'),
        },
        {
          committed() {},
          trace() {},
          session(key) {
            void goSession(key);
          },
          machine() {},
          sender() {},
          toolAll() {},
          script() {},
          image() {},
          background() {},
          jump() {},
          pager() {},
        },
      );
      return;
    }
    if (view.lastTotal !== null && !view.following && current.projection.total > view.lastTotal)
      view.newCount += current.projection.total - view.lastTotal;
    view.lastTotal = current.projection.total;
    if (view.renderedRevision === view.store.revision && view.renderedNote === view.note) {
      updateSessionControl(view.root, controlFor(view));
      updateSessionRuntime(view.root, runtimeFor(view));
      syncJump(view);
      if ((view.store.loadedRanges()[0]?.first ?? 0) > 0)
        updateSessionPager(view.root, {
          sid: view.key,
          where: 'before',
          text: 'Load earlier records',
          disabled: view.loading,
          busy: view.loading,
        });
      return;
    }
    view.renderedRevision = view.store.revision;
    view.renderedNote = view.note;
    const anchor = [...view.root.querySelectorAll<HTMLElement>('[data-entry-key]')].find(
      (node) =>
        node.getBoundingClientRect().bottom >
        (window.matchMedia('(max-width: 760px)').matches
          ? 0
          : document.querySelector<HTMLElement>('#main')!.getBoundingClientRect().top),
    );
    const anchorTop = anchor?.getBoundingClientRect().top;
    renderSessionScreen(
      view.root,
      {
        id: view.key,
        name: view.meta?.name ?? 'Session',
        control: controlFor(view),
        runtime: runtimeFor(view),
        observation:
          view.note ||
          {
            cached: 'Cached history',
            stale: 'Stale history',
            incomplete: 'History is incomplete',
            unavailable: 'Source history is unavailable',
          }[current.freshness.state] + ' · related session context is incomplete',
        blocks: catalogTranscriptBlocks(view.store.entries(), (entry) => fieldView(view, entry)),
        order: view.store.entries().map((item) => item.entry_id),
        before:
          (view.store.loadedRanges()[0]?.first ?? 0) > 0
            ? {
                sid: view.key,
                where: 'before',
                text: 'Load earlier records',
                disabled: view.loading,
                busy: view.loading,
              }
            : undefined,
        empty: 'No native records in this range.',
      },
      {
        committed() {},
        trace() {},
        session(key) {
          void goSession(key);
        },
        machine() {},
        sender() {},
        toolAll() {
          view.note = 'Full result unavailable for this source. The recorded preview is shown.';
          drawSelected(view);
        },
        script() {},
        image() {},
        background() {},
        jump() {
          jumpLatest(view);
        },
        pager() {
          void loadSelected(view, Math.max(0, view.store.loadedRanges()[0].first - 60));
        },
      },
    );
    const main = document.querySelector<HTMLElement>('#main')!;
    if (view.following) {
      view.opening = false;
      scrollEnd();
    } else if (anchor?.isConnected && anchorTop !== undefined) {
      const shift = anchor.getBoundingClientRect().top - anchorTop;
      if (window.matchMedia('(max-width: 760px)').matches) window.scrollBy(0, shift);
      else main.scrollTop += shift;
    } else if (view.opening) {
      view.opening = false;
      if (window.matchMedia('(max-width: 760px)').matches)
        window.scrollTo(0, document.documentElement.scrollHeight);
      else main.scrollTop = main.scrollHeight;
    }
    syncJump(view);
  }
  function scrollEnd() {
    if (window.matchMedia('(max-width: 760px)').matches)
      window.scrollTo(0, document.documentElement.scrollHeight);
    else {
      const main = document.querySelector<HTMLElement>('#main')!;
      main.scrollTop = main.scrollHeight;
    }
  }
  function syncJump(view: SelectedView) {
    if (active !== view || disposed) return;
    updateSessionJump(view.root, !view.following, view.newCount, false, jumpTarget);
  }
  function jumpLatest(view: SelectedView) {
    if (active !== view || disposed) return;
    view.following = true;
    view.newCount = 0;
    syncJump(view);
    scrollEnd();
    void loadSelected(view).finally(() => {
      if (active === view && !disposed) scrollEnd();
    });
  }
  function fieldView(view: SelectedView, entry: CatalogTranscriptEntry) {
    if (!entry.clipped || !entry.field) return null;
    const progress = view.fields.get(entry.entry_id),
      generation = view.store.selectedPage()!.projection.generation,
      changed = progress && progress.generation !== generation;
    return {
      text: progress?.text,
      note: changed
        ? 'Source projection changed. Previously loaded text is retained. '
        : progress?.note ||
          (progress?.next === null
            ? 'Complete recorded text loaded.'
            : progress?.text === undefined
              ? 'Recorded text preview. '
              : 'Part of the recorded text is loaded. '),
      action: capabilities.selected_entry
        ? {
            label: progress?.loading
              ? 'Loading text…'
              : changed
                ? 'Reload recorded text'
                : progress?.text === undefined
                  ? 'Load recorded text'
                  : 'Load more text',
            busy: !!progress?.loading,
            run() {
              void loadField(view, entry);
            },
          }
        : undefined,
      ...(progress?.next === null && !changed ? { action: undefined } : {}),
    };
  }
  async function loadField(view: SelectedView, entry: CatalogTranscriptEntry) {
    if (
      disposed ||
      view.sourceKey !== capabilities.source_key ||
      !capabilities.selected_entry ||
      !entry.field
    )
      return;
    const generation = view.store.selectedPage()?.projection.generation;
    if (!generation) return;
    let progress = view.fields.get(entry.entry_id);
    if (progress?.loading) return;
    if (!progress || progress.generation !== generation) {
      progress = { generation, next: 0, loading: false, note: '' };
      view.fields.set(entry.entry_id, progress);
    }
    if (progress.next === null) return;
    const chunk = progress.next,
      p = params(),
      pending = progress;
    p.set('sid', view.key);
    p.set('after', String(entry.slot));
    p.set('limit', '1');
    p.set('generation', generation);
    p.set('field_chunk', String(chunk));
    pending.loading = true;
    pending.note = '';
    view.renderedRevision = -1;
    drawSelected(view);
    try {
      const value = await api('/api/session-entry?' + p);
      if (disposed || view.fields.get(entry.entry_id) !== pending) return;
      if (view.store.selectedPage()?.projection.generation !== generation) {
        pending.note = 'Source projection changed. Reload recorded text. ';
        return;
      }
      const field = parseCatalogField(value, {
        source_key: capabilities.source_key,
        catalog_key: view.key,
        generation,
        entry,
        chunk,
      });
      pending.text = (chunk === 0 ? '' : (pending.text ?? '')) + field.text;
      pending.next = field.next;
    } catch (error) {
      if (disposed) return;
      pending.note =
        error && typeof error === 'object' && 'status' in error && error.status === 422
          ? 'Complete text is unavailable for this native format. '
          : readError(error) + ' ';
      if (error && typeof error === 'object' && 'status' in error && error.status === 409)
        void loadSelected(view);
    } finally {
      pending.loading = false;
      if (!disposed) {
        view.renderedRevision = -1;
        drawSelected(view);
      }
    }
  }
  async function resynchronize(view: SelectedView) {
    const candidate = new CatalogTranscriptStore();
    candidate.select(
      { source_key: capabilities.source_key, catalog_key: view.key },
      'retained_history',
    );
    const read = async (after: number | null, limit: number) => {
      const ticket = candidate.request(after, limit),
        p = params();
      p.set('sid', view.key);
      p.set('limit', String(limit));
      if (after !== null) p.set('after', String(after));
      if (ticket.generation !== null) p.set('generation', ticket.generation);
      candidate.accept(ticket, await api('/api/session-transcript?' + p));
    };
    try {
      await read(null, 60);
      const total = candidate.selectedPage()!.projection.total;
      for (const range of view.store.loadedRanges())
        for (let after = range.first; after < Math.min(range.end, total); after += 100)
          await read(after, Math.min(100, Math.min(range.end, total) - after));
      if (disposed) {
        candidate.destroy();
        return;
      }
      view.store.destroy();
      view.store = candidate;
      view.renderedRevision = -1;
      view.meta = candidate.selectedPage()!.session;
      view.note = '';
      view.retryDelay = 1000;
      if (active === view) {
        void loadIdentity(view);
      }
    } catch (error) {
      candidate.destroy();
      throw error;
    }
  }
  async function loadSelected(view: SelectedView, after: number | null = null) {
    if (view.loading && !disposed && capabilities.selected_transcript) {
      if (view.queuedAfter === undefined || after !== null) view.queuedAfter = after;
      return;
    }
    if (
      disposed ||
      view.sourceKey !== capabilities.source_key ||
      !capabilities.selected_transcript
    ) {
      drawSelected(view);
      return;
    }
    if (after !== null) view.following = false;
    scope.clearTimeout(view.retry);
    const request = view.store.request(after),
      p = params();
    p.set('sid', view.key);
    p.set('limit', String(request.limit));
    if (after !== null) p.set('after', String(after));
    if (request.generation !== null) p.set('generation', request.generation);
    view.loading = true;
    view.root.setAttribute('aria-busy', 'true');
    view.note = '';
    drawSelected(view);
    try {
      const value = await api('/api/session-transcript?' + p);
      if (disposed) return;
      if (view.store.accept(request, value)) {
        view.retryDelay = 1000;
        const current = view.store.selectedPage()!;
        const previousName = view.meta?.name;
        view.meta = current.session;
        if (active === view) {
          void loadIdentity(view);
          if (previousName !== view.meta.name) chrome();
        }
      }
    } catch (error) {
      if (disposed) return;
      view.store.reject(request);
      if (error && typeof error === 'object' && 'status' in error && error.status === 409) {
        view.note = 'Recorded history changed. Resynchronizing…';
        try {
          await resynchronize(view);
          return;
        } catch (failure) {
          error = failure;
        }
      }
      view.note = readError(error);
      if (
        retryable(error) ||
        (error && typeof error === 'object' && 'status' in error && error.status === 409)
      ) {
        view.retry = scope.timeout(() => void loadSelected(view, after), view.retryDelay);
        view.retryDelay = Math.min(8000, view.retryDelay * 2);
      }
    } finally {
      view.loading = false;
      if (!disposed) view.root.setAttribute('aria-busy', 'false');
      drawSelected(view);
      const queued = view.queuedAfter;
      view.queuedAfter = undefined;
      if (!disposed && active === view && queued !== undefined) void loadSelected(view, queued);
    }
  }
  async function loadIdentity(view: SelectedView) {
    if (
      disposed ||
      view.sourceKey !== capabilities.source_key ||
      view.identityLoading ||
      !view.ticket ||
      !capabilities.selected_identity
    )
      return;
    const ticket = view.ticket,
      p = currentParams();
    p.set('sid', view.key);
    view.identityLoading = true;
    let invalidated = false;
    try {
      const reply = object(await api('/api/session-identity?' + p));
      if (reply.api !== 1) throw new Error('Unsupported source identity response');
      if (active === view && selection.accept(ticket, reply.identity)) {
        const identity = selection.selectedIdentity();
        if (identity?.owner_qualification === 'provisional')
          view.note =
            'Session identity is provisional while history is being discovered. Native controls are unavailable.';
        else if (
          identity?.harness === 'codex' &&
          identity.native_selection &&
          identity.native_selection.state !== 'cached'
        )
          view.note =
            'Native Codex session selection is not currently observed. Controls are unavailable; retained history remains readable.';
        else if (
          view.note.startsWith('Session identity is provisional') ||
          view.note.startsWith('Native Codex session selection')
        )
          view.note = '';
        drawSelected(view);
      }
    } catch (error) {
      if (disposed || active !== view || ticket !== view.ticket) return;
      if (
        (error &&
          typeof error === 'object' &&
          'status' in error &&
          [401, 403, 404].includes(Number(error.status))) ||
        (error instanceof Error &&
          error.message === 'Historical identity cannot supply current catalog authority')
      ) {
        invalidated = true;
        view.ticket = selection.begin({
          source_key: capabilities.source_key,
          catalog_key: view.key,
        });
      }
      view.note =
        error && typeof error === 'object' && 'status' in error && error.status === 404
          ? 'Current native session selection is unavailable. Retained history remains readable.'
          : readError(error);
      drawSelected(view);
    } finally {
      view.identityLoading = false;
      if (!disposed && active === view && !invalidated && view.ticket !== ticket)
        scope.timeout(() => void loadIdentity(view), 1000);
    }
  }
  async function goSession(key: string, meta?: CatalogSession, push = true) {
    if (disposed) return;
    cancelCandidate();
    preserveScroll();
    sourcesOpen = false;
    let view = selected.get(cacheKey(key));
    if (!view) {
      const store = new CatalogTranscriptStore();
      store.select({ source_key: capabilities.source_key, catalog_key: key }, 'retained_history');
      view = {
        sourceKey: capabilities.source_key,
        key,
        root: document.createElement('div'),
        store,
        meta: meta ?? null,
        scroll: 0,
        loading: false,
        note: '',
        opening: true,
        identityLoading: false,
        retryDelay: 1000,
        renderedRevision: -1,
        renderedNote: '',
        fields: new Map(),
        following: true,
        newCount: 0,
        lastTotal: null,
      };
      selected.set(cacheKey(key), view);
    }
    active = view;
    view.ticket = selection.begin({ source_key: capabilities.source_key, catalog_key: key });
    navigation.route = { v: 'session', id: key };
    render(null, root);
    root.replaceChildren(view.root);
    chrome();
    shell.closeDrawer(true);
    if (push)
      history.pushState(
        null,
        '',
        '/s/' +
          encodeURIComponent(view.meta?.harness ?? 'native') +
          '/' +
          encodeURIComponent(key) +
          (capabilities.source_key
            ? '?machine=' + encodeURIComponent(capabilities.source_key)
            : ''),
      );
    drawSelected(view);
    if (view.store.selectedPage()) {
      void loadIdentity(view);
      if (window.matchMedia('(max-width: 760px)').matches) window.scrollTo(0, view.scroll);
      else document.querySelector<HTMLElement>('#main')!.scrollTop = view.scroll;
    } else {
      void loadIdentity(view);
      if (capabilities.selected_transcript) await loadSelected(view);
    }
  }
  async function recheckCapabilities() {
    const observedSource = capabilities.source_key,
      observedEpoch = sourcesEpoch;
    try {
      const next = parseCatalogCapabilities(
        await api('/api/session-capabilities?' + currentParams()),
      );
      if (disposed || observedEpoch !== sourcesEpoch || observedSource !== capabilities.source_key)
        return;
      if (next.source_key !== capabilities.source_key)
        throw new Error('Source selection changed. Choose the source again.');
      const ready = !capabilities.selected_transcript && next.selected_transcript;
      Object.assign(capabilities, next);
      if (next.source_candidates && !candidateLoaded && !active && !sourcesOpen)
        void loadCandidates(false);
      if (active) void loadIdentity(active);
      if (ready && active && !active.store.selectedPage()) void loadSelected(active);
    } catch (error) {
      if (disposed || observedEpoch !== sourcesEpoch || observedSource !== capabilities.source_key)
        return;
      if (active && !active.store.selectedPage()) {
        active.note = readError(error);
        drawSelected(active);
      }
    } finally {
      if (!disposed) scope.timeout(() => void recheckCapabilities(), 8000);
    }
  }
  function drawSources() {
    if (disposed || !sourcesOpen) return;
    chrome();
    renderCatalogSources(
      root,
      {
        items: sourceItems,
        updating: sourcesUpdating,
        note: sourcesNote,
        more: sourceCursor !== null,
        machinesHref: viewerHost?.machinesPath ?? '/machines?compat=1',
      },
      {
        select(key) {
          if (sourceItems.some((item) => item.source_key === key)) void switchSource(key);
        },
        more() {
          void loadSources(true);
        },
        retry() {
          void loadSources(false);
        },
      },
    );
  }
  async function loadSources(append: boolean) {
    if (disposed || !sourcesOpen || sourcesUpdating) return;
    const epoch = ++sourcesEpoch,
      p = new URLSearchParams({ limit: '60' });
    if (append && sourceCursor !== null) p.set('cursor', sourceCursor);
    sourcesUpdating = true;
    sourcesNote = '';
    drawSources();
    try {
      const next = parseCatalogSources(await api('/api/session-sources?' + p));
      if (disposed || !sourcesOpen || epoch !== sourcesEpoch) return;
      if (
        append &&
        next.items.some((item) => sourceItems.some((old) => old.source_key === item.source_key))
      )
        throw new Error('Source page repeated a machine');
      sourceItems = append ? [...sourceItems, ...next.items] : next.items;
      sourceCursor = next.next_cursor;
      sourcesRetryDelay = 1000;
    } catch (error) {
      if (disposed || !sourcesOpen || epoch !== sourcesEpoch) return;
      sourcesNote = readError(error);
      if (retryable(error)) {
        scope.timeout(() => {
          if (sourcesOpen && epoch === sourcesEpoch) void loadSources(append);
        }, sourcesRetryDelay);
        sourcesRetryDelay = Math.min(8000, sourcesRetryDelay * 2);
      }
    } finally {
      if (!disposed && epoch === sourcesEpoch) {
        sourcesUpdating = false;
        drawSources();
      }
    }
  }
  function saveSource() {
    preserveScroll();
    sourceViews.set(capabilities.source_key, {
      page,
      items,
      cursor,
      harness,
      repo,
      query,
      selectedKey: active?.key ?? null,
    });
  }
  async function showSources() {
    cancelCandidate();
    if (disposed) return;
    if (!sourcesOpen) saveSource();
    ++listEpoch;
    scope.clearTimeout(listRetry);
    active = null;
    selection.clear();
    navigation.route = { v: 'sessions' };
    sourcesOpen = true;
    root.replaceChildren();
    shell.closeDrawer(true);
    drawSources();
    if (!sourceItems.length) await loadSources(false);
  }
  async function switchSource(key: string, push = true) {
    if (disposed) return;
    const epoch = ++sourcesEpoch;
    sourcesUpdating = true;
    sourcesNote = '';
    drawSources();
    try {
      const next = parseCatalogCapabilities(
        await api('/api/session-capabilities?' + new URLSearchParams({ machine: key })),
      );
      if (disposed || epoch !== sourcesEpoch) return;
      if (next.source_key !== key)
        throw new Error('Source capability identity does not match this selection');
      if (!sourcesOpen) saveSource();
      ++listEpoch;
      scope.clearTimeout(listRetry);
      selection.clear();
      Object.assign(capabilities, next);
      const retained = sourceViews.get(key);
      page = retained?.page ?? null;
      items = retained?.items ?? [];
      cursor = retained?.cursor ?? null;
      cancelCandidate();
      candidates = [];
      candidateCursor = null;
      candidateLoaded = false;
      candidateNote = '';
      harness = retained?.harness ?? '';
      repo = retained?.repo ?? '';
      query = retained?.query ?? '';
      updating = false;
      note = '';
      sourcesOpen = false;
      active = null;
      if (push && retained?.selectedKey) await goSession(retained.selectedKey, undefined, false);
      else goList(false);
      const url = retained?.selectedKey
        ? '/s/' +
          encodeURIComponent(
            selected.get(cacheKey(retained.selectedKey))?.meta?.harness ?? 'native',
          ) +
          '/' +
          encodeURIComponent(retained.selectedKey)
        : '/sessions';
      if (push) history.pushState(null, '', url + '?' + new URLSearchParams({ machine: key }));
      else fromLocation();
      void loadList(false);
    } catch (error) {
      if (disposed || epoch !== sourcesEpoch) return;
      sourcesNote = readError(error);
    } finally {
      if (!disposed && epoch === sourcesEpoch) {
        sourcesUpdating = false;
        drawSources();
      }
    }
  }
  function fromLocation() {
    if (new URLSearchParams(location.search).get('choose_source') === '1') {
      void showSources();
      return;
    }
    const machine = new URLSearchParams(location.search).get('machine');
    if (machine && machine !== capabilities.source_key) {
      void showSources();
      void switchSource(machine, false);
      return;
    }
    const parts = location.pathname.split('/').filter(Boolean);
    if (parts[0] === 's' && parts[2]) {
      try {
        void goSession(decodeURIComponent(parts[2]), undefined, false);
      } catch {
        goList(false);
      }
    } else goList(false);
  }
  scope.listen(window, 'popstate', fromLocation);
  const readingScroll = () => {
    if (!active) return;
    const main = document.querySelector<HTMLElement>('#main')!,
      gap = window.matchMedia('(max-width: 760px)').matches
        ? document.documentElement.scrollHeight - window.scrollY - window.innerHeight
        : main.scrollHeight - main.scrollTop - main.clientHeight;
    active.following = gap <= 80;
    if (active.following) active.newCount = 0;
    syncJump(active);
  };
  scope.listen(window, 'scroll', readingScroll, { passive: true });
  scope.listen(document.querySelector<HTMLElement>('#main')!, 'scroll', readingScroll, {
    passive: true,
  });
  scope.listen(document, 'keydown', (event) => {
    if (event.key === 'Escape') shell.closeDrawer();
  });
  fromLocation();
  void loadList(false);
  scope.timeout(() => void recheckCapabilities(), 8000);
  scope.interval(() => {
    if (
      active &&
      capabilities.selected_transcript &&
      active.store.selectedPage() &&
      document.visibilityState === 'visible'
    )
      void loadSelected(active);
  }, 3000);
  return {
    destroy() {
      if (disposed) return;
      disposed = true;
      ++listEpoch;
      observation.destroy();
      removeRuntimeListener();
      runtimeObservation.destroy();
      scope.destroy();
      selection.destroy();
      navigation.destroy();
      control.destroy();
      updates.destroy();
      for (const view of selected.values()) {
        view.store.destroy();
        releaseScreen(view.root);
        render(null, view.root);
      }
      recent.destroy();
      render(null, root);
      shell.destroy();
      selected.clear();
    },
  };
}
