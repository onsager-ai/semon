import type { Entry } from '../domain/types';
import { object,optional,text } from '../domain/validate';
import type { ToolData } from '../lib/tool-details';
import { parseEntry } from '../state/transcript-wire';
type ToolEntry = Extract<Entry, {k: 'tool'}> & {full?: boolean; scriptLoaded?: boolean};
interface ToolLoaderHost {
  api: (path: string, signal?: AbortSignal | undefined, unchanged?: boolean) => Promise<unknown>;
  enc: (uriComponent: string | number | boolean) => string;
}
/** Owns toolLoader behavior through explicit application ports. */
export function createToolLoader(host: ToolLoaderHost) {
  function parseToolDetail(data: Record<string, unknown>): ToolData {
    const entry = parseEntry({ ...data, k: 'tool', name: '', arg: '', ok: true });
    if (entry.k !== 'tool') throw new Error('Invalid tool details'); return entry;
  }
  function fullOf(e: ToolEntry): Promise<Partial<Omit<ToolData, 'bg'>>> {
    return Promise.all((e.more ?? []).map(async part => ({part, data: object(await host.api('/api/entry?sid=' + host.enc(e.sid ?? '') + '&slot=' + e.slot + '&as=' + part))}))).then(parts => {
      const full: Partial<Omit<ToolData, 'bg'>> = {}, fullCut: string[] = [];
      for (const {part, data} of parts) {
        if (part === 'diff') { full.diff = parseToolDetail(data).diff; full.changes = parseToolDetail(data).changes; }
        else if (part === 'out') { full.out = optional(data.text, text); full.cut = parseToolDetail(data).cut; }
        else if (part === 'in') full.in = optional(data.text, text);
        else if (part === 'arg') full.arg = optional(data.text, text);
        if (data.truncated === true) fullCut.push(part);
      }
      full.fullCut = fullCut; return full;
    });
  }
  // Real URLs: every screen has one, and the server serves this page for each.

  return {fullOf};
}
