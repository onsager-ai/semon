import type { EffectScope } from './effects';
import type { JsonObject } from '../lib/model';
import { parseControl } from '../lib/control';
import type { ControlSnapshot, ControlView } from '../lib/control';
export type { ControlView } from '../lib/control';
function record(value: unknown): value is JsonObject {
  return !!value && typeof value === 'object' && !Array.isArray(value);
}
/** Live control owner. Uses accepted model transactions and the existing effect scope; no poller. */
export function createLocalControl(scope: EffectScope, refresh: () => void) {
  let current: ControlSnapshot | null = null;
  let busy = false;
  let note = '';
  let uncertain = false;
  let revision = 0;
  async function write(op: string, extra: JsonObject = {}) {
    if (!current || busy || (uncertain && op !== 'reconnect')) return;
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
      if (at !== revision) return;
      if (op === 'reconnect' && response.ok) uncertain = false;
      if (!response.ok || (record(receipt) && receipt.error)) {
        note =
          record(receipt) && typeof receipt.error === 'string'
            ? receipt.error
            : 'Control refused. Inspect the session before sending again.';
        uncertain = uncertain || note.includes('unknown');
      } else {
        note =
          op === 'answer'
            ? 'Response sent. Codex resolution means answered or cleared; the winning client is unknown.'
            : 'Native request accepted. Inspect the session for its outcome.';
      }
    } catch {
      if (at === revision) {
        uncertain = true;
        note = 'Delivery unknown. Inspect Codex and reconnect; this request will not be resent.';
      }
    } finally {
      scope.releaseRequest(controller);
      if (at === revision) {
        busy = false;
        refresh();
      }
    }
  }
  function view(sid?: string): ControlView | undefined {
    if (!current || (sid && sid !== current.thread)) return undefined;
    return {
      snapshot: current,
      busy,
      uncertain,
      note,
      send: (text) => void write('send', { text }),
      interrupt: () => void write('interrupt'),
      answer: (request, answer) =>
        void write('answer', { request: request.id, hash: request.hash, answer }),
      reconnect: () => void write('reconnect'),
    };
  }
  return {
    prepare: parseControl,
    adopt(value: ControlSnapshot | null) {
      current = value;
    },
    view,
    destroy() {
      ++revision;
      current = null;
    },
  };
}
