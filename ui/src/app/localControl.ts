import type { EffectScope } from './effects';
import type { JsonObject } from '../lib/model';
import { parseControl, unavailableControl } from '../lib/control';
import type { ControlSnapshot, ControlView } from '../lib/control';
export type { ControlView } from '../lib/control';
function record(value: unknown): value is JsonObject {
  return !!value && typeof value === 'object' && !Array.isArray(value);
}
/** Live control owner. Uses accepted model transactions and the existing effect scope; no poller. */
export function createLocalControl(scope: EffectScope, refresh: () => void) {
  const drafts = new Map<string, string>();
  let current: ControlSnapshot | null = null;
  let busy = false;
  let note = '';
  let uncertain = false;
  let revision = 0;
  let selection = 0;
  async function write(op: string, extra: JsonObject = {}): Promise<boolean> {
    if (!current || busy || (uncertain && op !== 'reconnect')) return false;
    if (
      op === 'send' &&
      (!current.connected ||
        !(current.activeTurn ? current.capabilities.steer : current.capabilities.input))
    )
      return false;
    if (op === 'reconnect' && current.runtime?.reconnectable === false) return false;
    if (op !== 'reconnect' && !current.connected) return false;
    if (op === 'interrupt' && !current.capabilities.interrupt) return false;
    if (op === 'answer') {
      const request = current.requests.find(
        (request) => request.id === extra.request && request.hash === extra.hash,
      );
      if (!request || request.state.state !== 'open') return false;
      const supported =
        request.kind === 'question'
          ? current.capabilities.questions
          : request.payload.method === 'item/fileChange/requestApproval'
            ? current.capabilities.fileApproval
            : current.capabilities.commandApproval;
      if (!supported) return false;
    }

    if (
      op === 'send' &&
      (!current.connected ||
        !(current.activeTurn ? current.capabilities.steer : current.capabilities.input))
    )
      return false;
    const target = current;
    const at = ++revision;
    busy = true;
    note = 'Sending…';
    refresh();
    const controller = scope.request();
    try {
      const command = {
        id: crypto.randomUUID().replaceAll('-', ''),
        op,
        thread: target.thread,
        generation: target.generation,
        activeTurn: target.activeTurn,
        expires: Date.now() + 25_000,
        ...extra,
      };
      const response = await fetch('/api/control', {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(command),
        signal: controller.signal,
      });
      const receipt: unknown = await response.json();
      if (at !== revision) return false;
      if (op === 'reconnect' && response.ok) uncertain = false;
      if (!response.ok || (record(receipt) && receipt.error)) {
        note =
          record(receipt) && typeof receipt.error === 'string'
            ? receipt.error
            : 'Control refused. Inspect the session before sending again.';
        uncertain = uncertain || note.includes('unknown');
        return false;
      } else {
        if (op === 'send') {
          const snapshot = record(receipt) && record(receipt.snapshot) ? receipt.snapshot : null;
          const action = snapshot && record(snapshot.actions) ? snapshot.actions[command.id] : null;
          if (!record(action) || action.delivery !== 'accepted') {
            uncertain = true;
            note = 'Delivery unconfirmed. Inspect the session before sending again.';
            return false;
          }
        }
        note =
          op === 'send'
            ? ''
            : op === 'answer'
              ? 'Response sent. Codex resolution means answered or cleared; the winning client is unknown.'
              : 'Native request accepted. Inspect the session for its outcome.';
        return true;
      }
    } catch {
      if (at === revision) {
        uncertain = true;
        note = 'Delivery unknown. Inspect Codex and reconnect; this request will not be resent.';
      }
      return false;
    } finally {
      scope.releaseRequest(controller);
      if (at === revision) {
        busy = false;
        refresh();
      }
    }
  }
  function view(sid?: string, draftKey = sid ?? current?.thread ?? ''): ControlView | undefined {
    if (!current || (sid && sid !== current.thread)) return undefined;
    const target = current;
    const selected = selection;
    const send = (op: string, extra: JsonObject = {}) =>
      selection === selected &&
      current?.thread === target.thread &&
      current.generation === target.generation
        ? write(op, extra)
        : Promise.resolve(false);
    return {
      get draft() {
        return drafts.get(draftKey) ?? '';
      },
      setDraft(text: string) {
        drafts.set(draftKey, text);
      },
      snapshot: current,
      busy,
      uncertain,
      note,
      send: (text) => send('send', { text }),
      interrupt: () => void send('interrupt'),
      answer: (request, answer) =>
        void send('answer', { request: request.id, hash: request.hash, answer }),
      reconnect: () => void send('reconnect'),
    };
  }
  function apply(value: ControlSnapshot | null) {
    if (current?.thread !== value?.thread) {
      ++selection;
      ++revision;
      busy = false;
      uncertain = false;
      note = '';
    }
    if (value?.runtime?.state === 'ended' && current?.runtime?.state !== 'ended') {
      ++revision;
      busy = false;
      note = '';
    }
    current = value;
  }
  return {
    readOnly(sid: string, draftKey = sid): ControlView {
      return {
        ...unavailableControl(sid),
        get draft() {
          return drafts.get(draftKey) ?? '';
        },
        setDraft(text) {
          drafts.set(draftKey, text);
        },
      };
    },
    prepare: parseControl,
    adopt: apply,
    unavailable() {
      if (!current) return;
      current = {
        ...current,
        connected: false,
        capabilities: {
          ...current.capabilities,
          input: false,
          steer: false,
          interrupt: false,
          commandApproval: false,
          fileApproval: false,
          questions: false,
        },
        runtime: current.runtime ? { ...current.runtime, freshness: 'stale' } : undefined,
        reason:
          current.runtime?.state === 'ended' || current.runtime?.state === 'failed'
            ? current.reason
            : 'Current connection status is unavailable. Reconnecting…',
      };
      refresh();
    },
    observe(value: ControlSnapshot | null) {
      apply(value);
      refresh();
    },
    view,
    destroy() {
      ++selection;
      ++revision;
      current = null;
    },
  };
}
