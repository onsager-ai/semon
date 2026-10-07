import { LocalControl } from './localControl';
import type { ControlView } from './control';
import type { RuntimeObservationView } from './runtimeObservation';
import { commitApplicationView } from './application-view';
import { Component, Fragment, render } from 'preact';
import type { ComponentChildren } from 'preact';
import { claimScreen, releaseScreen, Glyph, Harness, screenText } from './screens';
import type { HarnessMark } from './screens';
import { Inline, Markdown } from './richtext';
import { Sentence } from './sentence';
import type { SentencePart, SentenceHost } from './sentence';
import type { Attachment } from './attachments';
import { ToolStepContents } from './tool-step';
import type { ToolStepSnapshot } from './tool-step';
interface Identity {
  key?: string;
  entryKey?: string;
}
export interface MessageView extends Identity {
  kind: 'message';
  flavor: 'user' | 'assistant' | 'incoming';
  text: string;
  images?: readonly Attachment[];
  handoff?: string;
}
export interface ThoughtView extends Identity {
  kind: 'thought';
  mode: 'readable' | 'pending' | 'masked';
  label?: string;
  text?: string;
}
export interface LabelView extends Identity {
  kind: 'label';
  className: string;
  text: string;
  action?: { label: string; busy: boolean; run(): void };
}
export interface ToolView extends Identity {
  kind: 'tool';
  step: ToolStepSnapshot;
}
export interface BackgroundView extends Identity {
  kind: 'background';
  failed: boolean;
  label: string;
  call: string;
  loaded: boolean;
}
export interface GroupView extends Identity {
  kind: 'group';
  entries: readonly (ToolView | BackgroundView)[];
  lone: boolean;
  summary: readonly { text: string; className?: string }[];
  background: boolean;
  failed: number;
  running?: string;
  clockTick?: boolean;
  stack: string;
  chevron: string;
}
export interface ChildView extends Identity {
  kind: 'child';
  handoff: string;
  id: string;
  turn?: string;
  name: string;
  state: string;
  stateLabel: string;
  meta: string;
  mark?: HarnessMark;
  brief: string;
  result?: string;
  failed: boolean;
  activity?: readonly [string, string];
  trace?: string;
  traceLabel?: string;
  chevron: string;
}
export interface EventView extends Identity {
  kind: 'event';
  handoff: string;
  className: string;
  icon: string;
  parts: readonly SentencePart[];
  time: string;
  brief: string;
  result?: string;
  resultLabel: string;
  answers?: readonly string[];
  waiting: boolean;
  stateLabel: string;
}
export type EntryView =
  | MessageView
  | ThoughtView
  | LabelView
  | ToolView
  | BackgroundView
  | GroupView
  | ChildView
  | EventView;
export interface TurnView {
  id: string;
  label?: string;
  header?: { parts: readonly SentencePart[]; mark?: HarnessMark; time: string };
  entries: readonly EntryView[];
  error?: string;
  trace?: string;
  traceIcon: string;
  stateLabel: string;
  bare?: boolean;
}
export type TranscriptBlock =
  | { kind: 'turn'; turn: TurnView }
  | { kind: 'loose'; key: string; entries: readonly EntryView[] };
export interface PagerView {
  sid: string;
  where: 'before' | 'after';
  text: string;
  disabled: boolean;
  busy: boolean;
}
export interface FooterView {
  state: string;
  stateLabel: string;
  text: string;
  items: readonly { kind: string; text: string; tip: string }[];
  parent?: { id: string; turn?: string; name: string };
}
export interface SessionSnapshot {
  control?: ControlView;
  runtime?: RuntimeObservationView;
  id: string;
  name: string;
  observation?: string;
  blocks: readonly TranscriptBlock[];
  started?: { lead: string; machine: string };
  before?: PagerView;
  after?: PagerView;
  empty?: string;
  footer?: FooterView;
  dirty?: ReadonlySet<string>;
  order: readonly string[];
}
export interface TranscriptHost extends SentenceHost {
  committed(): void;
  trace(id: string): void;
  toolAll(key: string, label: string): void;
  script(key: string): void;
  image(url: string, label: string, trigger: HTMLButtonElement): void;
  background(call: string, trigger: HTMLElement): void;
  pager(button: HTMLButtonElement): void;
  jump(): void;
}
interface SessionOwner {
  disposed: boolean;
  snapshot: SessionSnapshot;
  host: TranscriptHost;
  blocks: readonly TranscriptBlock[];
  open: Set<string>;
  mounted: Set<string>;
  failedImages: Set<string>;
  revisions: Map<string, number>;
  clipped: Set<string>;
  clamps: Map<string, HTMLElement>;
  observer: ResizeObserver;
  frame: number;
  jump: HTMLElement | null;
  jumpVisible: boolean;
  jumpCount: number;
  jumpBusy: boolean;
  paint(): void;
  controls: (() => void) | null;
  runtime: (() => void) | null;
  measure(): void;
  change(key: string): void;
}
const owners = new WeakMap<HTMLElement, SessionOwner>();
/** The existing page tree owns this leaf; status never walks transcript blocks. */
class SessionControls extends Component<{ owner: SessionOwner }> {
  componentDidMount() {
    this.props.owner.controls = () => this.forceUpdate();
  }
  componentWillUnmount() {
    this.props.owner.controls = null;
  }
  render() {
    const view = this.props.owner.snapshot.control;
    return view ? (
      <LocalControl view={view} runtimeObserved={!!this.props.owner.snapshot.runtime} />
    ) : null;
  }
}
/** Durable status stays observable even when no current native control identity exists. */
class SessionRuntime extends Component<{ owner: SessionOwner }> {
  componentDidMount() {
    this.props.owner.runtime = () => this.forceUpdate();
  }
  componentWillUnmount() {
    this.props.owner.runtime = null;
  }
  render() {
    const value = this.props.owner.snapshot.runtime;
    if (!value) return null;
    const observation = value.observation;
    return (
      <section class="local-control" aria-label="Environment status">
        <p role="status">
          {observation
            ? {
                active: 'Environment is active.',
                disconnected: 'Environment is disconnected.',
                failed: 'Environment needs attention.',
                ended: 'This session has ended.',
                unavailable: 'Runtime state is unavailable.',
              }[observation.state]
            : 'Runtime state is unavailable.'}
        </p>
        {observation?.phase && <p>Phase: {screenText(observation.phase)}</p>}
        {observation?.presence && <p>Compute presence: {screenText(observation.presence)}</p>}
        {(value.delivery !== 'current' || observation?.freshness !== 'current') && (
          <p role="status">
            {value.delivery === 'updating' || observation?.freshness === 'updating'
              ? 'Checking the environment…'
              : value.delivery === 'stale' || observation?.freshness === 'stale'
                ? 'Last environment observation is stale.'
                : 'Current environment observation is unavailable.'}
          </p>
        )}
        {value.reason && <p>{screenText(value.reason)}</p>}
        {observation?.observationError && <p>{screenText(observation.observationError)}</p>}
      </section>
    );
  }
}

const identity = (entry: Identity) => entry.entryKey ?? entry.key ?? '';
const TRACE = 'M6 4v10a4 4 0 0 0 4 4h8M6 10h12M15 7l3 3-3 3M15 15l3 3-3 3';
function State({ state, label, text }: { state: string; label: string; text: string }) {
  return (
    <span class={'state ' + state}>
      {state === 'work' ? (
        <span class="spin" />
      ) : (
        <span class={'dot ' + state} role="img" aria-label={label} data-tip={label} />
      )}
      <span>{screenText(text)}</span>
    </span>
  );
}
function Answer({ values }: { values: readonly string[] }) {
  return (
    <>
      {!values.length ? (
        <>
          <b>Answered</b>
          {' · reply not in these logs'}
        </>
      ) : (
        <>
          <b>{values.length === 1 ? 'You answered: ' : 'You answered:'}</b>
          {values.length === 1 ? (
            values[0]
          ) : (
            <ol>
              {values.map((value, i) => (
                <li key={i}>{screenText(value)}</li>
              ))}
            </ol>
          )}
        </>
      )}
    </>
  );
}
function Entry({ entry, owner }: { entry: EntryView; owner: SessionOwner }): ComponentChildren {
  const key = identity(entry),
    open = owner.open.has(key),
    active = (action: () => void) => {
      if (!owner.disposed) action();
    };
  switch (entry.kind) {
    case 'message':
      return (
        <div
          class={entry.flavor === 'incoming' ? 'bubble in' : 'msg ' + entry.flavor}
          data-e={entry.key}
          data-entry-key={entry.entryKey}
          data-h={entry.handoff}
        >
          {entry.images?.length ? (
            <div class="attach-row">
              {entry.images.map((image, i) =>
                image.unavailable || owner.failedImages.has(key + ':' + image.url) ? (
                  <span key={i} class="attach-na">
                    Image not available
                  </span>
                ) : (
                  <button
                    key={i}
                    class="attach"
                    type="button"
                    aria-haspopup="dialog"
                    onClick={(event) => {
                      if (event.currentTarget.isConnected)
                        active(() => owner.host.image(image.url, image.label, event.currentTarget));
                    }}
                  >
                    <img
                      class={'attach-img' + (image.width ? '' : ' unsized')}
                      alt={image.label}
                      loading="lazy"
                      decoding="async"
                      width={image.width}
                      height={image.height}
                      src={image.url}
                      onError={() =>
                        active(() => {
                          owner.failedImages.add(key + ':' + image.url);
                          owner.change(key);
                        })
                      }
                    />
                  </button>
                ),
              )}
            </div>
          ) : null}
          {(!entry.images?.length || entry.text) && <Markdown text={entry.text} />}
        </div>
      );
    case 'thought':
      return entry.mode === 'pending' ? (
        <div class="think-pending" data-e={entry.key} data-entry-key={entry.entryKey}>
          <span class="spin" />
          <span>Thinking…</span>
        </div>
      ) : (
        <div
          class={'thought' + (entry.mode === 'masked' ? ' masked' : '')}
          data-e={entry.key}
          data-entry-key={entry.entryKey}
        >
          <div class="think-label">{screenText(entry.label ?? '')}</div>
          {entry.mode === 'readable' && <Markdown text={entry.text ?? ''} className="think-text" />}
        </div>
      );
    case 'label':
      return (
        <div class={entry.className} data-e={entry.key} data-entry-key={entry.entryKey}>
          {screenText(entry.text)}
          {entry.action && (
            <button
              type="button"
              class="link"
              disabled={entry.action.busy}
              onClick={(event) => {
                if (event.currentTarget.isConnected) active(() => entry.action?.run());
              }}
            >
              {screenText(entry.action.label)}
            </button>
          )}
        </div>
      );
    case 'tool':
      return (
        <div
          class={entry.step.className}
          data-e={entry.key}
          data-entry-key={entry.entryKey}
          data-tid={entry.step.tid}
          data-live={entry.step.sid}
          data-since={entry.step.since}
        >
          <ToolStepContents
            snapshot={entry.step}
            expanded={open}
            mounted={owner.mounted.has(key)}
            toggle={() =>
              active(() => {
                owner.mounted.add(key);
                if (open) owner.open.delete(key);
                else owner.open.add(key);
                owner.change(key);
              })
            }
            host={{
              all(label) {
                owner.host.toolAll(key, label);
              },
              script() {
                owner.host.script(key);
              },
            }}
          />
        </div>
      );
    case 'background': {
      const content = (
        <>
          <Glyph path="M4 17l5-5-5-5M12 19h8" className="" />
          <span class="bg-label">{screenText(entry.label)}</span>
        </>
      );
      return (
        <div
          class={'step bgend' + (entry.failed ? ' err' : '')}
          data-e={entry.key}
          data-entry-key={entry.entryKey}
        >
          {entry.loaded ? (
            <button
              class="bg-line"
              type="button"
              onClick={(event) => {
                if (event.currentTarget.isConnected)
                  owner.host.background(entry.call, event.currentTarget);
              }}
            >
              {content}
            </button>
          ) : (
            <div class="bg-line">{content}</div>
          )}
        </div>
      );
    }
    case 'group': {
      const steps = (
        <div
          class={'steps' + (entry.lone ? ' lone' : '')}
          hidden={entry.summary.length > 0 && !open}
        >
          {entry.entries.map((item, i) => (
            <Entry key={identity(item) || i} entry={item} owner={owner} />
          ))}
        </div>
      );
      return !entry.summary.length ? (
        steps
      ) : (
        <div class="tgroup" data-e={entry.key} data-entry-key={entry.entryKey}>
          <button
            class="tsum"
            type="button"
            aria-expanded={open}
            onClick={(event) => {
              if (!event.currentTarget.isConnected) return;
              if (open) owner.open.delete(key);
              else owner.open.add(key);
              owner.change(key);
            }}
          >
            {entry.running ? <span class="spin" /> : <Glyph path={entry.stack} className="" />}
            <span class={'tt' + (entry.background ? ' bgsum' : '')}>
              {entry.summary.map((part, i) =>
                part.className ? (
                  <span key={i} class={part.className}>
                    {screenText(part.text)}
                  </span>
                ) : (
                  part.text
                ),
              )}
            </span>
            {entry.failed > 0 && (
              <span class="tf">{screenText('· ' + entry.failed + ' failed')}</span>
            )}
            {entry.running && (
              <span class="tl tick">
                {entry.clockTick
                  ? '· running ' + entry.running
                  : screenText('· running ' + entry.running)}
              </span>
            )}
            <Glyph path={entry.chevron} className="chev" />
          </button>
          {steps}
        </div>
      );
    }
    case 'child':
      return (
        <div
          class="child-card"
          data-e={entry.key}
          data-entry-key={entry.entryKey}
          data-h={entry.handoff}
        >
          <span class="cc-head">
            <button
              class="cc-name who-link"
              type="button"
              aria-label={'Open ' + entry.name}
              onClick={(event) => {
                if (event.currentTarget.isConnected) owner.host.session(entry.id, entry.turn);
              }}
            >
              {screenText(entry.name)}
            </button>
            <State state={entry.state} label={entry.stateLabel} text={entry.stateLabel} />
            <Glyph path={entry.chevron} className="chev" />
          </span>
          <span class="cc-meta">
            {entry.mark && <Harness mark={entry.mark} />}
            {screenText(entry.meta)}
          </span>
          <span class="cc-brief">
            <Inline text={entry.brief} />
          </span>
          {entry.result ? (
            <span class={'cc-result' + (entry.failed ? ' err' : '')}>
              <span class="rl">{entry.failed ? 'Result: ' : 'Returned: '}</span>
              <Inline text={entry.result} />
            </span>
          ) : (
            entry.activity && (
              <span class="cc-now">
                <span class="spin" />
                <span>{screenText(entry.activity[0])}</span>
                <code>{entry.activity[1]}</code>
              </span>
            )
          )}
          {entry.trace && (
            <button
              class="link cc-run"
              type="button"
              aria-label={entry.traceLabel}
              onClick={(event) => {
                if (event.currentTarget.isConnected) owner.host.trace(entry.trace!);
              }}
            >
              Run view
            </button>
          )}
        </div>
      );
    case 'event':
      return (
        <div
          class={entry.className}
          data-e={entry.key}
          data-entry-key={entry.entryKey}
          data-h={entry.handoff}
        >
          <div class="ev-head">
            <Glyph path={entry.icon} className="" />
            <span class="ln">
              <Sentence parts={entry.parts} host={owner.host} />
            </span>
            <span class="tm">{entry.time}</span>
          </div>
          <div
            class={'ev-text' + (open ? ' open' : owner.clipped.has(key) ? ' clipped' : '')}
            ref={(node) => {
              if (node) owner.clamps.set(key, node);
            }}
          >
            <Inline text={entry.brief} />
          </div>
          <button
            class="link ev-more"
            type="button"
            hidden={!owner.clipped.has(key) && !open}
            aria-expanded={open}
            onClick={(event) => {
              event.stopPropagation();
              if (!event.currentTarget.isConnected) return;
              if (open) owner.open.delete(key);
              else owner.open.add(key);
              owner.change(key);
            }}
          >
            {open ? 'Show less' : 'Show more'}
          </button>
          {entry.result && (
            <div class="ev-result">
              <span class="rl">{entry.resultLabel}</span>
              <Inline text={entry.result} />
            </div>
          )}
          {entry.answers && (
            <div class={'ev-result answer' + (entry.answers.length ? '' : ' none')}>
              <Answer values={entry.answers} />
            </div>
          )}
          {entry.waiting && <State state="wait" label={entry.stateLabel} text="Waiting on you" />}
        </div>
      );
  }
}
class Turn extends Component<{ view: TurnView; owner: SessionOwner; revision: number }> {
  shouldComponentUpdate(next: Readonly<{ view: TurnView; owner: SessionOwner; revision: number }>) {
    return next.view !== this.props.view || next.revision !== this.props.revision;
  }
  render() {
    const { view, owner } = this.props;
    return (
      <section class="turn" data-turn={view.id} aria-label={view.label}>
        {view.header && (
          <div class="turn-h">
            {view.header.mark && <Harness mark={view.header.mark} size={16} lead={false} />}
            <Sentence parts={view.header.parts} host={owner.host} />
            <span class="tm">{view.header.time}</span>
          </div>
        )}
        {!view.bare && (
          <div class="tx">
            {view.entries.map((entry, i) => (
              <Entry key={identity(entry) || i} entry={entry} owner={owner} />
            ))}
          </div>
        )}
        {(view.error || view.trace) && (
          <div class="turn-end">
            {view.error && <State state="err" label={view.stateLabel} text={view.error} />}
            {view.trace && (
              <button
                class="link"
                type="button"
                aria-label="Trace what this turn set off"
                onClick={(event) => {
                  if (event.currentTarget.isConnected) owner.host.trace(view.trace!);
                }}
              >
                <Glyph path={view.traceIcon} className="" />
                <span>Trace</span>
              </button>
            )}
          </div>
        )}
      </section>
    );
  }
}
function mergeBlocks(
  previous: readonly TranscriptBlock[],
  snapshot: SessionSnapshot,
): readonly TranscriptBlock[] {
  if (!snapshot.dirty) return snapshot.blocks;
  const blocks = [...previous],
    changed = new Map(
      snapshot.blocks
        .filter(
          (block): block is Extract<TranscriptBlock, { kind: 'turn' }> => block.kind === 'turn',
        )
        .map((block) => [block.turn.id, block]),
    );
  for (const id of snapshot.order) {
    if (!snapshot.dirty.has(id)) continue;
    const at = blocks.findIndex((block) => block.kind === 'turn' && block.turn.id === id),
      next = changed.get(id);
    if (at >= 0) {
      if (next) blocks[at] = next;
      else blocks.splice(at, 1);
    } else if (next) {
      const later = snapshot.order.slice(snapshot.order.indexOf(id) + 1),
        index = blocks.findIndex((block) => block.kind === 'turn' && later.includes(block.turn.id));
      if (index >= 0) blocks.splice(index, 0, next);
      else blocks.push(next);
    }
  }
  return blocks.filter((block) => block.kind !== 'turn' || snapshot.order.includes(block.turn.id));
}
function paintJump(owner: SessionOwner) {
  if (!owner.jump || owner.disposed) return;
  owner.jump.hidden = !owner.jumpVisible;
  render(
    <button
      class="jump"
      type="button"
      id="jump-bottom"
      aria-label={
        owner.jumpCount
          ? 'Jump to bottom; ' + owner.jumpCount + ' new entries'
          : 'Jump to bottom of transcript'
      }
      disabled={owner.jumpBusy}
      onClick={(event) => {
        if (event.currentTarget.isConnected) owner.host.jump();
      }}
    >
      {owner.jumpCount > 0 && <span class="new-count">{owner.jumpCount + ' new'}</span>}
      <Glyph path="M12 4v15M5 12l7 7 7-7" className="" />
    </button>,
    owner.jump,
  );
}
export function updateSessionJump(
  root: HTMLElement,
  visible: boolean,
  count: number,
  busy = false,
) {
  const owner = owners.get(root);
  if (!owner) return;
  owner.jumpVisible = visible;
  owner.jumpCount = count;
  owner.jumpBusy = busy;
  paintJump(owner);
}
export function renderSessionScreen(
  root: HTMLElement,
  snapshot: SessionSnapshot,
  host: TranscriptHost,
) {
  let owner = owners.get(root);
  if (owner && owner.snapshot.id !== snapshot.id) {
    releaseScreen(root);
    owner = undefined;
  }
  if (!owner) {
    const state: SessionOwner = {
      snapshot,
      host,
      disposed: false,
      blocks: snapshot.blocks,
      open: new Set(),
      mounted: new Set(),
      failedImages: new Set(),
      revisions: new Map(),
      clipped: new Set(),
      clamps: new Map(),
      frame: 0,
      jump: null,
      jumpVisible: false,
      jumpCount: 0,
      jumpBusy: false,
      paint() {},
      controls: null,
      runtime: null,
      measure() {},
      change() {},
      observer: new ResizeObserver(() => {}),
    };
    owner = state;
    owners.set(root, state);
    state.measure = () => {
      if (state.disposed) return;
      let changed = false;
      for (const [key, node] of state.clamps) {
        if (!node.isConnected || state.open.has(key) || !node.clientHeight) continue;
        const clipped = node.scrollHeight > node.clientHeight + 1;
        if (state.clipped.has(key) !== clipped) {
          if (clipped) state.clipped.add(key);
          else state.clipped.delete(key);
          changed = true;
          for (const block of state.blocks)
            if (
              block.kind === 'turn' &&
              block.turn.entries.some((entry) => identity(entry) === key)
            )
              state.revisions.set(block.turn.id, (state.revisions.get(block.turn.id) ?? 0) + 1);
        }
      }
      if (changed) state.paint();
    };
    state.observer.disconnect();
    state.observer = new ResizeObserver(state.measure);
    claimScreen(root, 'session', () => {
      state.disposed = true;
      state.observer.disconnect();
      cancelAnimationFrame(state.frame);
      if (state.jump) render(null, state.jump);
      owners.delete(root);
    });
  }
  const state = owner;
  state.host = host;
  state.blocks = mergeBlocks(state.blocks, snapshot);
  state.snapshot = snapshot;
  state.change = (key) => {
    for (const block of state.blocks)
      if (
        block.kind === 'turn' &&
        block.turn.entries.some(
          (entry) =>
            identity(entry) === key ||
            (entry.kind === 'group' && entry.entries.some((item) => identity(item) === key)),
        )
      )
        state.revisions.set(block.turn.id, (state.revisions.get(block.turn.id) ?? 0) + 1);
    state.paint();
  };
  function pager(view: PagerView) {
    return (
      <div key={view.where} class="list">
        <button
          class="more"
          type="button"
          data-load-earlier={view.where === 'before' ? '' : undefined}
          data-pager-sid={view.sid}
          data-pager-where={view.where}
          disabled={view.disabled}
          aria-busy={view.busy || undefined}
          onClick={(event) => {
            if (event.currentTarget.isConnected) state.host.pager(event.currentTarget);
          }}
        >
          {view.busy && <span class="spin" aria-hidden="true" />}
          <span class="pager-label" aria-live="polite">
            {screenText(view.text)}
          </span>
        </button>
      </div>
    );
  }
  state.paint = () => {
    if (state.disposed) return;
    const held =
        document.activeElement instanceof HTMLElement &&
        document.activeElement.closest('.session-foot')
          ? document.activeElement
          : null,
      heldKind = held?.dataset.foot;
    const view = state.snapshot;
    commitApplicationView(
      root,
      <>
        <div class="ph sr">
          <h1>{screenText(view.name)}</h1>
        </div>
        <section class="transcript" aria-label="Transcript">
          {view.observation && (
            <p class="empty" role="status" data-transcript-observation="">
              {screenText(view.observation)}
            </p>
          )}
          <div class="turns">
            {view.before
              ? pager(view.before)
              : view.started && (
                  <div class="divider started">
                    <span class="dv-text">
                      <span class="dv-lead">{view.started.lead}</span>
                      <span class="dv-machine" data-tip={view.started.machine} data-tip-clipped="">
                        {screenText(view.started.machine)}
                      </span>
                    </span>
                  </div>
                )}
            {state.blocks.map((block) =>
              block.kind === 'turn' ? (
                <Turn
                  key={block.turn.id}
                  view={block.turn}
                  owner={state}
                  revision={state.revisions.get(block.turn.id) ?? 0}
                />
              ) : (
                <Fragment key={block.key}>
                  {block.entries.map((entry, i) => (
                    <Entry key={identity(entry) || block.key + i} entry={entry} owner={state} />
                  ))}
                </Fragment>
              ),
            )}
            {view.after && pager(view.after)}
            {view.empty && <p class="empty">{screenText(view.empty)}</p>}
          </div>
          <div
            class="jump-wrap"
            ref={(node) => {
              state.jump = node;
              paintJump(state);
            }}
          />
        </section>
        <SessionRuntime owner={state} />
        <SessionControls owner={state} />
        {view.footer && (
          <div class="session-foot">
            <span class={'stat ' + view.footer.state}>
              {view.footer.state === 'work' ? (
                <span class="spin" />
              ) : (
                <span
                  class={'dot ' + view.footer.state}
                  role="img"
                  aria-label={view.footer.stateLabel}
                />
              )}
              <span>
                {screenText(view.footer.text)}
                {view.footer.items.map((item) => (
                  <Fragment key={item.kind}>
                    {screenText(' · ')}
                    <span
                      key={item.kind}
                      class="cr-item"
                      data-foot={item.kind}
                      data-tip={item.tip || undefined}
                      tabIndex={item.tip ? 0 : undefined}
                    >
                      {screenText(item.text)}
                    </span>
                  </Fragment>
                ))}
              </span>
            </span>
            {view.footer.parent && (
              <button
                type="button"
                data-foot="open"
                onClick={(event) => {
                  if (event.currentTarget.isConnected)
                    state.host.session(view.footer!.parent!.id, view.footer!.parent!.turn);
                }}
              >
                {screenText('Open in ' + view.footer.parent.name)}
              </button>
            )}
          </div>
        )}
      </>,
    );
    if (heldKind && held && !held.isConnected)
      (
        root.querySelector<HTMLElement>('[data-foot="' + CSS.escape(heldKind) + '"]') ??
        root.querySelector<HTMLElement>('[data-foot="time"]')
      )?.focus({ preventScroll: true });
    for (const [key, node] of state.clamps) {
      if (node.isConnected) state.observer.observe(node);
      else {
        state.observer.unobserve(node);
        state.clamps.delete(key);
      }
    }
  };
  state.paint();
  host.committed();
}

/** Update only native controls; retain parsed blocks, keyed turns and input ownership. */
export function updateSessionControl(root: HTMLElement, control: ControlView | undefined) {
  const owner = owners.get(root);
  if (!owner || owner.disposed) return;
  owner.snapshot = { ...owner.snapshot, control };
  owner.controls?.();
}
export function updateSessionRuntime(
  root: HTMLElement,
  runtime: RuntimeObservationView | undefined,
) {
  const owner = owners.get(root);
  if (!owner || owner.disposed) return;
  const hadRuntime = !!owner.snapshot.runtime;
  owner.snapshot = { ...owner.snapshot, runtime };
  owner.runtime?.();
  if (hadRuntime !== !!runtime) owner.controls?.();
}

export function updateSessionPager(root: HTMLElement, view: PagerView) {
  const owner = owners.get(root);
  if (!owner || owner.snapshot.id !== view.sid) return;
  owner.snapshot = { ...owner.snapshot, [view.where]: view };
  owner.paint();
}

export function updateSessionClock(
  root: HTMLElement,
  now: number,
  starts: Readonly<Record<string, number>>,
) {
  const owner = owners.get(root);
  if (!owner || owner.disposed) return;
  const elapsed = (since: number) => {
    const n = Math.max(0, Math.floor((now - since) / 1000));
    return n < 60 ? n + 's' : Math.floor(n / 60) + 'm ' + (n % 60) + 's';
  };
  function update(entry: EntryView): EntryView {
    if (entry.kind === 'tool' && entry.step.running && entry.step.sid) {
      const since = entry.step.since ?? starts[entry.step.sid];
      if (!Number.isFinite(since)) return entry;
      const text = elapsed(since!),
        step = entry.step;
      const status = step.background ? step.status : text,
        backgroundStatus = step.background ? 'running ' + text : step.backgroundStatus;
      return status === step.status && backgroundStatus === step.backgroundStatus
        ? entry
        : { ...entry, step: { ...step, status, backgroundStatus } };
    }
    if (entry.kind === 'group') {
      const entries = entry.entries.map((item) => update(item) as ToolView | BackgroundView),
        since = entries
          .filter(
            (item): item is ToolView =>
              item.kind === 'tool' && item.step.running && !!item.step.sid,
          )
          .map((item) => item.step.since ?? starts[item.step.sid!])
          .filter((value): value is number => value !== undefined && Number.isFinite(value));
      const running = since.length ? elapsed(Math.min(...since)) : entry.running;
      return running === entry.running && entries.every((item, i) => item === entry.entries[i])
        ? entry
        : { ...entry, entries, running, clockTick: true };
    }
    return entry;
  }
  let changed = false;
  owner.blocks = owner.blocks.map((block) => {
    const previous = block.kind === 'turn' ? block.turn.entries : block.entries,
      entries = previous.map(update);
    if (entries.every((entry, i) => entry === previous[i])) return block;
    changed = true;
    return block.kind === 'turn'
      ? { ...block, turn: { ...block.turn, entries } }
      : { ...block, entries };
  });
  if (changed) owner.paint();
}

export function measureSessionScreen(root: HTMLElement) {
  owners.get(root)?.measure();
}
