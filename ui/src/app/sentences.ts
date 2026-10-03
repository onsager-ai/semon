import { machineShorts } from '../domain/format';
import type { Handoff } from '../domain/types';
import type { SentenceHost, SentencePart, SentenceSnapshot } from '../lib';
import { I } from './registry';
interface SentencesHost {
  nameOf: (id: string) => string;
  SESS: Record<string, import('../domain/types').Session>;
  STARTS: Map<string, import('../domain/types').Turn>;
  MACHINE: Record<string, string>;
  goSession: (id: string, turn?: string | undefined) => void;
  go: (
    r: import('../navigation/routes').ApplicationRoute,
    fromHistory?: boolean,
    prepared?: boolean,
    nextContent?: import('../viewer-host').ViewerContent | null,
  ) => void;
  HID: Map<string, import('../domain/types').Handoff>;
  openSender: (h: import('../domain/types').Handoff) => void;
}
/** Owns sentences behavior through explicit application ports. */
export function createSentences(host: SentencesHost) {
  function sentenceSnapshot(
    h: Handoff,
    viewer: string | null | undefined,
    links = false,
  ): SentenceSnapshot {
    const parts: SentencePart[] = [],
      text = (className: string, text: string) => parts.push({ className, text });
    const who = (id: string, action?: SentencePart['action'], label?: (name: string) => string) => {
      const name = host.nameOf(id),
        linked = links && action && id !== 'you' && id !== viewer && host.SESS[id];
      parts.push({
        text: name,
        className: linked ? 'who-link' : 'who',
        action: linked ? action : undefined,
        label: linked ? label?.(name) : undefined,
      });
    };
    const sender: SentencePart['action'] = { kind: 'sender', id: h.id },
      recipient: SentencePart['action'] = {
        kind: 'session',
        id: h.to,
        turn: host.STARTS.get(h.id)?.id,
      };
    const senderLabel = (name: string) => 'Open ' + name + ' where it sent this',
      recipientLabel = (name: string) => 'Open ' + name + ' at the turn this started';
    let path;
    if (h.kind === 'ask') {
      path = I.ask;
      who('you');
      text('verb', ' asked ');
      who(h.to, recipient, recipientLabel);
    } else if (h.kind === 'spawn' || h.kind === 'relay') {
      if (viewer === h.to) {
        path = I.in;
        text('verb', h.kind === 'spawn' ? 'Brief from ' : 'Relay from ');
        who(h.from, sender, senderLabel);
      } else {
        path = I.out;
        who(h.from, sender, senderLabel);
        text(
          'verb',
          h.kind === 'spawn'
            ? ' handed off to ' +
                (host.SESS[h.to]?.kind === 'Subagent'
                  ? 'subagent'
                  : (host.SESS[h.to]?.kind ?? '')) +
                ' '
            : ' relayed to ',
        );
        who(h.to, recipient, recipientLabel);
      }
    } else if (h.kind === 'move') {
      path = I.move;
      const short = machineShorts(
        [h.fromMachine, h.toMachine].map((id) => [id, host.MACHINE[id] ?? id]),
      );
      const machine = (id: string) =>
        parts.push({
          className: 'verb mach',
          text: short.get(id) ?? id,
          tip: 'Machine: ' + (host.MACHINE[id] ?? id),
          action: links ? { kind: 'machine', id } : undefined,
          label: links ? 'Open machine ' + (host.MACHINE[id] ?? id) : undefined,
        });
      text('verb', 'Semon moved ');
      who(h.to, { kind: 'session', id: h.to }, (name: string) => 'Open ' + name);
      text('verb', ' from ');
      machine(h.fromMachine);
      text('verb', ' to ');
      machine(h.toMachine);
    } else if (h.kind === 'toyou') {
      path =
        h.status === 'done' && (h.ask === 'question' || h.ask === 'decision')
          ? I.done
          : h.ask === 'question'
            ? I.qc
            : h.ask === 'decision'
              ? I.decide
              : I.result;
      who(h.from, sender, senderLabel);
      text(
        'verb',
        { question: ' asked you', result: ' sent you a result', decision: ' needs your decision' }[
          h.ask
        ],
      );
    }
    return { icon: path ?? '', parts };
  }
  const sentenceHost: SentenceHost = {
    session(id, turn) {
      host.goSession(id, turn);
    },
    machine(id) {
      host.go({ v: 'machine', id });
    },
    sender(id) {
      const h = host.HID.get(id);
      if (h) host.openSender(h);
    },
  };

  return { sentenceHost, sentenceSnapshot };
}
