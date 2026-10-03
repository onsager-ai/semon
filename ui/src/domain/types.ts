/** Validated data consumed by domain calculations; no document or JSON bag types. */
export type SessionState = 'work' | 'wait' | 'idle' | 'done' | 'err';
export type HandoffStatus = 'work' | 'wait' | 'done' | 'err' | 'new';
export type TokenKind = 'input' | 'output' | 'cache_write' | 'cache_write_5m' | 'cache_write_1h' | 'cache_read' | 'web_search';
export type Usage = Partial<Record<TokenKind, number>>;
export interface ModelCost { usd: number | null; tokens?: Usage; usd_by_kind?: Usage }
export interface Cost { usd: number | null; unpriced_models?: string[]; split_unknown_messages?: number; by_model?: Record<string, ModelCost>; by_day?: Record<string, number> }
export interface Session {
  id: string; name: string; harness: string; state: SessionState; machine: string; start: number; last: number;
  effort?: string; cwd?: string; dir?: string; directory?: string; sessionId?: string; pid?: number;
  wait_edges_truncated?: boolean; busy?: [number, number][]; wait_edges?: { call: string; tool: string; targets: string[]; start: number; end?: number; turn?: string }[];
  reported_runs?: { start: number; cost_usd?: number }[]; cost_check?: { start: number; computed_usd?: number; reported_usd?: number; ok?: boolean }[];
  kind?: string; host?: string; repo?: string; branch?: string; worktree?: string; parent?: string; lane?: boolean;
  model?: string; modelId?: string; role?: boolean; stub?: boolean; movedFrom?: string;
  calls?: number; errors?: number; waiting_since?: number; waiting_for?: string;
  tokens?: number[]; tokens_by_model?: Record<string, Usage>; cost?: Cost;
  activity?: [string, string, number, number?]; tool_calls?: Record<string, number>; signals?: Record<string, number>;
}
interface HandoffBase { id: string; from: string; to: string; at: number; status: HandoffStatus; brief?: string; done?: number; result?: string; target?: string; declined?: boolean }
export type Handoff = HandoffBase & (
  { kind: 'ask' | 'spawn' | 'relay' } |
  { kind: 'move'; fromMachine: string; toMachine: string } |
  { kind: 'toyou'; ask: 'question' | 'decision' | 'result'; answers?: { values?: string[] }[]; answer?: string | string[] }
);
export interface TurnEnd { st?: SessionState; why: string; h?: string; at?: number }
export interface Background { state: 'running' | 'unknown' | 'failed' | 'killed' | 'done'; secs?: string; since?: number; exit?: number; summary?: string }
export type Image = { o: number; b: number; w?: number; h?: number; type?: string; size?: number } & ({ na: true; v?: string } | { na?: false; v: string });
interface EntryBase { img?: Image[]; key?: string; turn?: string; sid?: string; slot?: number; live?: boolean; unfinished?: boolean; bg?: Background; tid?: string }
export type Entry = EntryBase & (
  { k: 'u' | 'a'; text: string; img?: Image[] } | { k: 'think'; text?: string; pending?: boolean; status?: string; secs?: number | string; displaySecs?: number | null } |
  { k: 'h'; id: string } | { k: 'end'; text?: string; ret?: { to: string; failed?: boolean; at?: number } } |
  ({ k: 'tool'; title?: string; secs?: string; since?: number; exit?: number } & import('../lib/tool-details').ToolData) |
  { k: 'signal'; signal: { kind: string; tag?: string; tool?: string; value?: number; previous?: string } } |
  { k: 'bgend'; call: string; state: string; label?: string } | { k: 'harness'; label: string }
);
export interface Turn { id: string; sid: string; start: Handoff | null; at?: number; u: { k: 'u'; text: string } | null; entries: Entry[]; out: Handoff[]; sent: Handoff[]; end?: TurnEnd; last?: boolean }
export interface TranscriptMeta { from: number; to: number; total: number; calls?: number; errors?: number; tok?: string; watchTok?: string; newer?: number; origin?: boolean }
export interface DomainState {
  sessions: Record<string, Session>; machines: Record<string, string>; handoffs: Handoff[];
  turns: Record<string, Turn[]>; turn: Map<string, Turn>; starts: Map<string, Turn>; holds: Map<string, Turn>; handoff: Map<string, Handoff>;
  transcriptMeta: Record<string, TranscriptMeta>;
}
