/** Unknown JSON enters here. Validation copies only declared fields. */
export function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw new Error('Invalid object');
  return Object.fromEntries(Object.entries(value));
}
export function text(value: unknown): string {
  if (typeof value !== 'string') throw new Error('Invalid string');
  return value;
}
export function number(value: unknown): number {
  if (typeof value !== 'number' || !Number.isFinite(value)) throw new Error('Invalid number');
  return value;
}
export function boolean(value: unknown): boolean {
  if (typeof value !== 'boolean') throw new Error('Invalid boolean');
  return value;
}
export function optional<T>(value: unknown, parse: (value: unknown) => T): T | undefined {
  return value == null ? undefined : parse(value);
}
export function array<T>(value: unknown, parse: (value: unknown) => T): T[] {
  if (!Array.isArray(value)) throw new Error('Invalid array');
  return value.map(parse);
}
export function dictionary<T>(value: unknown, parse: (value: unknown) => T): Record<string, T> {
  return Object.fromEntries(Object.entries(object(value)).map(([id, row]) => [id, parse(row)]));
}
export function state(value: unknown) {
  switch (value) {
    case 'work':
    case 'wait':
    case 'idle':
    case 'done':
    case 'err':
      return value;
    default:
      throw new Error('Invalid session state');
  }
}
export function status(value: unknown) {
  switch (value) {
    case 'work':
    case 'wait':
    case 'done':
    case 'err':
    case 'new':
      return value;
    default:
      throw new Error('Invalid handoff status');
  }
}
