import { render } from 'preact';
import { screenText } from './screens';
export interface SentencePart { text: string; className: string; tip?: string; label?: string; action?: { kind: 'session' | 'machine' | 'sender'; id: string; turn?: string } }
export interface SentenceSnapshot { icon: string; parts: readonly SentencePart[] }
export interface SentenceHost { session(id: string, turn?: string): void; machine(id: string): void; sender(id: string): void }
export function Sentence({ parts, host }: { parts: readonly SentencePart[]; host: SentenceHost }) {
  return <>{parts.map((part, i) => part.action ? <button key={i} class={part.className} type="button" aria-label={part.label} data-tip={part.tip} onClick={event => { event.stopPropagation(); if (!event.currentTarget.isConnected || !part.action) return; if (part.action.kind === 'session') host.session(part.action.id, part.action.turn); else if (part.action.kind === 'machine') host.machine(part.action.id); else host.sender(part.action.id); }}>{screenText(part.text)}</button> : <span key={i} class={part.className} data-tip={part.tip}>{screenText(part.text)}</span>)}</>;
}
export function createSentenceParts(snapshot: SentenceSnapshot, host: SentenceHost): Node[] {
  const root = document.createDocumentFragment(); render(<Sentence parts={snapshot.parts} host={host} />, root); return [...root.childNodes];
}
