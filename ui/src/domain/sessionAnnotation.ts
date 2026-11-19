import type { SessionAnnotation } from '../lib/sessionAnnotation';
export type { SessionAnnotation } from '../lib/sessionAnnotation';
import { number, object, text } from './validate';

export function parseSessionAnnotation(value: unknown): SessionAnnotation {
  const row = object(value);
  const positive = (value: unknown) => {
    if (value === null) return null;
    const n = number(value);
    if (!Number.isInteger(n) || n <= 0) throw Error('Invalid annotation number');
    return n;
  };
  const system = text(row.system),
    repository = text(row.repository),
    ticket = text(row.ticket);
  const title = row.title === null ? null : text(row.title);
  if (
    !system ||
    !ticket ||
    !/^[^/]+\/[^/]+$/.test(repository) ||
    (title !== null && [...title].length > 200) ||
    row.source !== 'annotation'
  )
    throw Error('Invalid session annotation');
  return {
    system,
    repository,
    ticket,
    issue: positive(row.issue),
    pr: positive(row.pr),
    title,
    source: 'annotation',
  };
}
