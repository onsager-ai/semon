import { LocalControl } from './localControl';
import type { ControlView } from './control';
import { commitApplicationView } from './application-view';
import { render } from 'preact';

export interface MachineRow {
  id: string;
  name: string;
  state: string;
  stateLabel: string;
  status: string;
  detail: string;
}
export interface MachinesSnapshot {
  up: number;
  working: number;
  rows: readonly MachineRow[];
  admin: { label: string; href: string } | null;
}
export interface MachinesHost {
  machine(id: string): void;
  admin(href: string): void;
  committed(): void;
}
const owned = new WeakSet<HTMLElement>();
const kinds = new WeakMap<HTMLElement, string>();
export const screenKind = (root: HTMLElement) => kinds.get(root);
const cleanups = new WeakMap<HTMLElement, () => void>();
export function claimScreen(root: HTMLElement, kind: string, cleanup: () => void) {
  owned.add(root);
  kinds.set(root, kind);
  cleanups.set(root, cleanup);
}
export function ownsScreen(root: HTMLElement): boolean {
  return owned.has(root);
}
export function releaseScreen(root: HTMLElement) {
  cleanups.get(root)?.();
  cleanups.delete(root);
  kinds.delete(root);
  const state = sessionsState.get(root);
  if (state) {
    cancelAnimationFrame(state.frame);
    window.removeEventListener('resize', state.resize);
    sessionsState.delete(root);
  }
  if (owned.delete(root)) render(null, root);
}
export const screenText = (text: string) =>
  text.replace(/ · /g, '\u2009 · \u2009').replace(/^· /, '·\u2009 ');
export function MachinesView({
  snapshot,
  host,
}: {
  snapshot: MachinesSnapshot;
  host: MachinesHost;
}) {
  return (
    <>
      <div class="ph">
        <h1>Machines</h1>
        <div class="sub">
          <span>
            <b>{snapshot.up + ' of ' + snapshot.rows.length}</b>up
          </span>
          <span>
            <b>{snapshot.working}</b>sessions working
          </span>
        </div>
      </div>
      <div class="list">
        {snapshot.rows.map((row) => (
          <button
            key={row.id}
            class="nrow"
            type="button"
            data-m={row.id}
            onClick={(event) => {
              if (event.currentTarget.isConnected) host.machine(row.id);
            }}
          >
            <span
              class={'dot ' + row.state}
              role="img"
              aria-label={row.stateLabel}
              data-tip={row.stateLabel}
            />
            <span class="nm">{screenText(row.name)}</span>
            <span class="ag">{row.status}</span>
            <span class="for">{screenText(row.detail)}</span>
          </button>
        ))}
        {snapshot.admin && (
          <button
            class="more"
            type="button"
            onClick={(event) => {
              if (event.currentTarget.isConnected) host.admin(snapshot.admin!.href);
            }}
          >
            {screenText(snapshot.admin.label)}
          </button>
        )}
      </div>
    </>
  );
}
export function renderMachinesScreen(
  root: HTMLElement,
  snapshot: MachinesSnapshot,
  host: MachinesHost,
) {
  commitApplicationView(root, <MachinesView snapshot={snapshot} host={host} />);
  owned.add(root);
  kinds.set(root, 'machines');
  host.committed();
}

import { Inline } from './richtext';
export interface HarnessMark {
  id: string;
  name: string;
  light: string;
  dark: string;
  darkTheme: boolean;
}
export interface LiveRow extends MachineRow {
  harness?: HarnessMark;
  activity?: readonly [string, string, number];
}
export interface InboxRow {
  id: string;
  quiet: boolean;
  icon: string;
  parts: readonly { className: string; text: string; tip?: string }[];
  age: string;
  preview: string;
  answer?: string;
  origin?: { message?: string; text?: string };
  context: string;
  harness?: HarnessMark;
  trace?: string;
}
export interface ActivityHost {
  session(id: string): void;
  inbox(id: string): void;
  trace(id: string): void;
  committed(): void;
}
export interface HomeSnapshot {
  control?: ControlView;
  waiting: number;
  working: number;
  up: number;
  machines: number;
  inbox: readonly InboxRow[];
  live: readonly LiveRow[];
  answered: readonly InboxRow[];
  totalAnswered: number;
  allAnswered: boolean;
}
export interface HomeHost extends ActivityHost {
  answered(): void;
}
export interface MachineSnapshot {
  name: string;
  sessions: readonly LiveRow[];
  totalSessions: number;
  off: readonly LiveRow[];
  moves: readonly InboxRow[];
}
export function Glyph({ path, className = 'icon' }: { path: string; className?: string }) {
  return (
    <svg
      class={className}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="1.8"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      <path d={path} />
    </svg>
  );
}
export function Harness({
  mark,
  size = 14,
  lead = true,
}: {
  mark: HarnessMark;
  size?: 14 | 16;
  lead?: boolean;
}) {
  return (
    <span
      class={'hicon ' + (size === 16 ? 'hi-screen-16' : 'hi-screen') + (lead ? ' hi-lead' : '')}
      data-harness={mark.id}
      aria-hidden="true"
    >
      {mark.light === mark.dark ? (
        <img src={mark.light} alt="" draggable={false} decoding="async" />
      ) : (
        <>
          <img
            class="hi-light"
            loading={mark.darkTheme ? 'lazy' : undefined}
            src={mark.light}
            alt=""
            draggable={false}
            decoding="async"
          />
          <img
            class="hi-dark"
            loading={mark.darkTheme ? undefined : 'lazy'}
            src={mark.dark}
            alt=""
            draggable={false}
            decoding="async"
          />
        </>
      )}
    </span>
  );
}
function Section({ heading, count }: { heading: string; count: number }) {
  return (
    <div class="sec-h">
      {screenText(heading)}
      <span class="n">{count}</span>
    </div>
  );
}
function Live({ row, host }: { row: LiveRow; host: ActivityHost }) {
  return (
    <button
      class="nrow"
      type="button"
      data-id={row.id}
      onClick={(event) => {
        if (event.currentTarget.isConnected) host.session(row.id);
      }}
    >
      <span
        class={'dot ' + row.state}
        role="img"
        aria-label={row.stateLabel}
        data-tip={row.stateLabel}
      />
      <span class="nm">{screenText(row.name)}</span>
      <span class="ag">
        {row.harness && <Harness mark={row.harness} />}
        {screenText(row.status)}
      </span>
      <span class="for">{screenText(row.detail)}</span>
      {row.activity && (
        <span class="act">
          <span class="spin" />
          <span>{screenText(row.activity[0])}</span>
          <code>{row.activity[1]}</code>
          <span class="el">{Math.max(0, row.activity[2]) + 's'}</span>
        </span>
      )}
    </button>
  );
}
function Inbox({ row, host }: { row: InboxRow; host: ActivityHost }) {
  return (
    <div
      class={'ib' + (row.quiet ? ' quiet' : '')}
      tabIndex={0}
      role="link"
      data-h={row.id}
      onClick={(event) => {
        if (event.currentTarget.isConnected && getSelection()?.isCollapsed) host.inbox(row.id);
      }}
      onKeyDown={(event) => {
        if (
          event.currentTarget.isConnected &&
          event.target === event.currentTarget &&
          (event.key === 'Enter' || event.key === ' ')
        ) {
          event.preventDefault();
          host.inbox(row.id);
        }
      }}
    >
      <Glyph path={row.icon} />
      <span class="ln">
        {row.parts.map((part, i) => (
          <span key={i} class={part.className} data-tip={part.tip}>
            {screenText(part.text)}
          </span>
        ))}
      </span>
      <span class="tm">{row.age}</span>
      <span class="q">
        <Inline text={row.preview} />
      </span>
      {row.answer !== undefined && <span class="ans">{screenText(row.answer)}</span>}
      {row.origin && (
        <span class="org">
          {row.origin.message ? (
            <>
              From your message:{' '}
              <span class="oq">{'“' + screenText(row.origin.message) + '”'}</span>
            </>
          ) : (
            screenText(row.origin.text ?? '')
          )}
        </span>
      )}
      <span class="ctx">
        <span>
          {row.harness && <Harness mark={row.harness} />}
          {screenText(row.context)}
        </span>
        {row.trace && (
          <button
            class="tracebtn"
            type="button"
            aria-label="Trace what this turn set off"
            onClick={(event) => {
              event.stopPropagation();
              if (event.currentTarget.isConnected) host.trace(row.trace!);
            }}
          >
            <Glyph path="M6 4v10a4 4 0 0 0 4 4h8M6 10h12M15 7l3 3-3 3M15 15l3 3-3 3" />
            <span>Trace</span>
          </button>
        )}
      </span>
    </div>
  );
}
export function HomeView({ snapshot, host }: { snapshot: HomeSnapshot; host: HomeHost }) {
  return (
    <>
      <div class="ph">
        <h1>Home</h1>
        <div class="sub">
          <span>
            <b>{snapshot.waiting}</b>waiting on you
          </span>
          <span>
            <b>{snapshot.working}</b>working
          </span>
          <span>
            <b>{snapshot.up + ' of ' + snapshot.machines}</b>
            {snapshot.machines === 1 ? 'machine up' : 'machines up'}
          </span>
        </div>
      </div>
      {snapshot.control && <LocalControl view={snapshot.control} />}
      <Section heading="Needs you" count={snapshot.waiting} />
      <div class="list">
        {snapshot.inbox.map((row) => (
          <Inbox key={row.id} row={row} host={host} />
        ))}
        {!snapshot.waiting && <p class="empty">Nothing is waiting on you.</p>}
      </div>
      <Section heading="Working now" count={snapshot.working} />
      <div class="list">
        {snapshot.live.map((row) => (
          <Live key={row.id} row={row} host={host} />
        ))}
        {!snapshot.working && <p class="empty">Nothing is running.</p>}
      </div>
      {snapshot.totalAnswered > 0 && (
        <>
          <Section heading="Answered" count={snapshot.totalAnswered} />
          <div class="list">
            {snapshot.answered.map((row) => (
              <Inbox key={row.id} row={row} host={host} />
            ))}
            {!snapshot.allAnswered && snapshot.totalAnswered > 3 && (
              <button
                class="more"
                type="button"
                onClick={(event) => {
                  if (event.currentTarget.isConnected) host.answered();
                }}
              >
                {'Show all ' + snapshot.totalAnswered + ' answered'}
              </button>
            )}
          </div>
        </>
      )}
    </>
  );
}
export function renderHomeScreen(root: HTMLElement, snapshot: HomeSnapshot, host: HomeHost) {
  commitApplicationView(root, <HomeView snapshot={snapshot} host={host} />);
  owned.add(root);
  kinds.set(root, 'home');
  host.committed();
}
export function MachineView({ snapshot, host }: { snapshot: MachineSnapshot; host: ActivityHost }) {
  return (
    <>
      <div class="ph sr">
        <h1>{screenText(snapshot.name)}</h1>
      </div>
      {snapshot.totalSessions > 0 && (
        <>
          <Section heading="Sessions" count={snapshot.totalSessions} />
          <div class="list">
            {snapshot.sessions.map((row) => (
              <Live key={row.id} row={row} host={host} />
            ))}
          </div>
        </>
      )}
      {snapshot.off.length > 0 && (
        <>
          <Section heading="Moved off" count={snapshot.off.length} />
          <div class="list">
            {snapshot.off.map((row) => (
              <Live key={row.id} row={row} host={host} />
            ))}
          </div>
        </>
      )}
      {snapshot.moves.length > 0 && (
        <>
          <Section heading="Moves" count={snapshot.moves.length} />
          <div class="list">
            {snapshot.moves.map((row) => (
              <Inbox key={row.id} row={row} host={host} />
            ))}
          </div>
        </>
      )}
      {!snapshot.totalSessions && !snapshot.off.length && (
        <p class="empty">No sessions have run here.</p>
      )}
    </>
  );
}
export function renderMachineScreen(
  root: HTMLElement,
  snapshot: MachineSnapshot,
  host: ActivityHost,
) {
  commitApplicationView(root, <MachineView snapshot={snapshot} host={host} />);
  owned.add(root);
  kinds.set(root, 'machine');
  host.committed();
}

export interface SessionRow {
  id: string;
  name: string;
  state: string;
  stateLabel: string;
  age: string;
  harness?: HarnessMark;
  model: string;
  modelTip: string;
  delegation?: string;
  fields: readonly {
    className: string;
    text: string;
    priority: number;
    tip?: string;
    icon?: string;
  }[];
}
export interface SessionsSnapshot {
  total: number;
  working: number;
  waiting: number;
  query: string;
  groupBy: string;
  reviews: boolean;
  showReviews: boolean;
  groups: readonly {
    title: string;
    total: number;
    harness?: HarnessMark;
    rows: readonly SessionRow[];
  }[];
  matches: number;
}
export interface SessionsHost {
  session(id: string): void;
  search(value: string): void;
  group(value: string): void;
  reviews(): void;
  committed(): void;
  facets(): HTMLElement;
}
export function fitSessionRows(root: ParentNode) {
  for (const meta of root.querySelectorAll<HTMLElement>('.session-row-meta')) {
    if (!meta.isConnected || !meta.clientWidth) continue;
    const values = [...meta.querySelectorAll<HTMLElement>('[data-drop]')];
    for (const value of values) value.hidden = false;
    for (const value of values.sort((a, b) => Number(b.dataset.drop) - Number(a.dataset.drop))) {
      if (meta.scrollWidth <= meta.clientWidth + 1) break;
      value.hidden = true;
    }
  }
}
function Session({ row, host }: { row: SessionRow; host: SessionsHost }) {
  return (
    <button
      class="nrow"
      type="button"
      data-id={row.id}
      data-session-row="list"
      onClick={(event) => {
        if (event.currentTarget.isConnected) host.session(row.id);
      }}
    >
      <span class="session-row-main srow-main">
        <span
          class={'dot ' + row.state}
          role="img"
          aria-label={row.stateLabel}
          data-tip={row.stateLabel}
        />
        <span class="nm" data-tip={row.name} data-tip-clipped="">
          {screenText(row.name)}
        </span>
        <span class="ag">{row.age}</span>
      </span>
      <span class="session-row-meta srow-meta for">
        {row.harness && (
          <span
            class="hicon hi-screen"
            data-harness={row.harness.id}
            role="img"
            aria-label={row.harness.name}
            data-tip={row.harness.name}
          >
            {row.harness.light === row.harness.dark ? (
              <img src={row.harness.light} alt="" draggable={false} decoding="async" />
            ) : (
              <>
                <img
                  class="hi-light"
                  loading={row.harness.darkTheme ? 'lazy' : undefined}
                  src={row.harness.light}
                  alt=""
                  draggable={false}
                  decoding="async"
                />
                <img
                  class="hi-dark"
                  loading={row.harness.darkTheme ? undefined : 'lazy'}
                  src={row.harness.dark}
                  alt=""
                  draggable={false}
                  decoding="async"
                />
              </>
            )}
          </span>
        )}
        <span class="row-model" data-tip={row.modelTip}>
          {screenText(row.model)}
        </span>
        {row.delegation && (
          <svg
            class="icon row-delegation"
            data-tip="Delegated session"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="1.8"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
          >
            <path d={row.delegation} />
          </svg>
        )}
        {row.fields.map((field) => (
          <span
            key={field.priority}
            class={field.className + (field.icon ? ' row-field' : '')}
            data-drop={field.priority}
            data-tip={field.tip}
          >
            {field.icon ? (
              <>
                <Glyph path={field.icon} />
                <span class="field-value">{screenText(field.text)}</span>
              </>
            ) : (
              screenText(field.text)
            )}
          </span>
        ))}
      </span>
    </button>
  );
}
const sessionsState = new WeakMap<
  HTMLElement,
  { input: HTMLInputElement | null; frame: number; resize: () => void }
>();
export function renderSessionsScreen(
  root: HTMLElement,
  snapshot: SessionsSnapshot,
  host: SessionsHost,
  focusSearch = false,
) {
  let state = sessionsState.get(root);
  if (!state) {
    state = { input: null, frame: 0, resize: () => fitSessionRows(root) };
    sessionsState.set(root, state);
    window.addEventListener('resize', state.resize, { passive: true });
  }
  const current = state;
  const fieldValue =
    current.input?.value.trim() === snapshot.query ? current.input.value : snapshot.query;
  const facet = host.facets();
  commitApplicationView(
    root,
    <>
      <div class="ph">
        <h1>Sessions</h1>
        <div class="sub">
          <span>
            <b>{snapshot.total}</b>
            {snapshot.total === 1 ? 'session' : 'sessions'}
          </span>
          <span>
            <b>{snapshot.working}</b>working
          </span>
          <span>
            <b>{snapshot.waiting}</b>waiting on you
          </span>
        </div>
      </div>
      <div
        class="session-facet-slot"
        ref={(node) => {
          if (node && facet.parentElement !== node) node.append(facet);
        }}
      />
      <label class="find">
        <Glyph path="M11 18a7 7 0 1 1 0-14 7 7 0 0 1 0 14zM20 20l-4-4" />
        <input
          id="sq"
          type="search"
          placeholder="Search sessions"
          aria-label="Search sessions"
          value={fieldValue}
          ref={(node) => {
            current.input = node;
          }}
          onInput={(event) => {
            if (event.currentTarget.isConnected) host.search(event.currentTarget.value);
          }}
        />
      </label>
      <div class="groupby" role="group" aria-label="Group by">
        {[
          ['recent', 'Recent'],
          ['project', 'Project'],
          ['machine', 'Machine'],
          ['harness', 'Harness'],
        ].map(([key, label]) => (
          <button
            key={key}
            type="button"
            data-g={key}
            aria-pressed={snapshot.groupBy === key}
            onClick={(event) => {
              if (event.currentTarget.isConnected) host.group(key);
            }}
          >
            {label}
          </button>
        ))}
      </div>
      {snapshot.showReviews && (
        <div class="groupby" role="group" aria-label="Session visibility">
          <button
            type="button"
            data-show-approval-reviews=""
            aria-pressed={snapshot.reviews}
            onClick={(event) => {
              if (event.currentTarget.isConnected) host.reviews();
            }}
          >
            Show approval reviews
          </button>
        </div>
      )}
      <div class="sess session-groups">
        {snapshot.groups.map((group) => (
          <div key={group.title} class="session-group">
            {group.title && (
              <div class="sec-h">
                {screenText(group.title)}
                <span class="n">{group.total}</span>
                {group.harness && <Harness mark={group.harness} lead={false} />}
              </div>
            )}
            <div class="list">
              {group.rows.map((row) => (
                <Session key={row.id} row={row} host={host} />
              ))}
            </div>
          </div>
        ))}
        {!snapshot.matches && (
          <p class="empty">{'No sessions match “' + screenText(snapshot.query) + '”.'}</p>
        )}
      </div>
    </>,
  );
  owned.add(root);
  kinds.set(root, 'sessions');
  host.committed();
  cancelAnimationFrame(current.frame);
  current.frame = requestAnimationFrame(current.resize);
  if (focusSearch) current.input?.focus({ preventScroll: true });
}
