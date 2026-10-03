import { object, text, number, boolean, array, dictionary } from '../domain/validate';
type Parser<T> = (value: unknown) => T;
/** Each declared field is parsed before this copied record can be returned. */
function shape<S extends Record<string, Parser<unknown>>>(fields: S) {
  return (value: unknown): { [K in keyof S]: ReturnType<S[K]> } => {
    const source = object(value),
      parsed = Object.fromEntries(
        Object.entries(fields).map(([key, parse]) => [key, parse(source[key])]),
      );
    return parsed as { [K in keyof S]: ReturnType<S[K]> };
  };
}
const list =
  <T>(parse: Parser<T>) =>
  (value: unknown) =>
    array(value, parse);
const nullable =
  <T>(parse: Parser<T>) =>
  (value: unknown) =>
    value == null ? null : parse(value);
const optional =
  <T>(parse: Parser<T>) =>
  (value: unknown) =>
    value == null ? undefined : parse(value);
const strings = list(text);
const money = shape({ usd: nullable(number), unpriced_models: strings });
const period = shape({
  agent_ms: number,
  started: number,
  turns: number,
  tools: number,
  errors: number,
  peak: number,
  wait_ms: number,
  median_wait_ms: number,
  longest_wait_ms: number,
  cost: money,
});
const busy = shape({ sid: text, ms: number });
const priced = shape({ sid: text, usd: nullable(number), unpriced_models: strings });
const column = shape({
  from: number,
  to: number,
  claude_ms: number,
  codex_ms: number,
  sessions: list(busy),
  more: number,
});
const day = shape({
  from: number,
  to: number,
  claude_usd: number,
  codex_usd: number,
  sessions: list(priced),
  more: number,
});
const breakdown = shape({
  repo: optional(nullable(text)),
  machine: optional(text),
  harness: optional(text),
  model: optional(text),
  ms: number,
  usd: number,
  sessions: number,
  unpriced_models: strings,
});
const tokens = shape({
  input: optional(number),
  output: optional(number),
  cache_read: optional(number),
  cache_write: optional(number),
});
const item = shape({
  sid: text,
  models: optional(strings),
  cost_usd: nullable(number),
  pr_url: nullable(text),
});
const model = shape({
  model: text,
  band: text,
  n: number,
  small_sample: boolean,
  first_pass_acceptance: nullable(number),
  acceptance_n: number,
  median_review_rounds: nullable(number),
  review_rounds_n: number,
  median_red_ci_heads: nullable(number),
  red_ci_n: number,
  median_model_ms: nullable(number),
  model_time_n: number,
  median_cost_usd: nullable(number),
  cost_n: number,
  allowance_per_million_input: nullable(number),
  allowance_n: number,
  tokens: tokens,
  tokens_n: number,
  items: list(item),
  items_more: number,
});
export const parseAnalytics = shape({
  days: number,
  version: text,
  current: period,
  previous: period,
  longest_current_wait: nullable(busy),
  calls_unknown: number,
  agents: shape({ unit: text, columns: list(column) }),
  cost: nullable(shape({ days: list(day), unpriced_models: strings })),
  breakdown: shape({ repo: list(breakdown), machine: list(breakdown), model: list(breakdown) }),
  top: shape({ busy: list(busy), waited: list(busy), cost: list(priced) }),
  sessions: (v: unknown) => dictionary(v, shape({ name: text, harness: text })),
  models: optional(
    shape({ groups: list(model), unknown_reasons: (v: unknown) => dictionary(v, text) }),
  ),
  allowance: nullable(
    shape({
      recorded_at: number,
      windows: list(shape({ minutes: number, used_percent: number, resets_at: number })),
    }),
  ),
  facets: shape({ repo: list(nullable(text)), machine: strings, harness: strings, model: strings }),
});
export type AnalyticsData = ReturnType<typeof parseAnalytics>;
export type AnalyticsModelGroup = ReturnType<typeof model>;
export type AnalyticsSliceItem = ReturnType<typeof busy> | ReturnType<typeof priced>;
