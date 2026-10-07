import type { CatalogTranscriptEntry } from '../state/catalog-transcript-wire';
import type { TranscriptBlock, EntryView } from '../lib/transcript';
import { I } from './registry';
export interface CatalogFieldView {
  text?: string;
  note: string;
  action?: { label: string; busy: boolean; run(): void };
}
/** Native source records can be readable before relationship context is complete. */
export function catalogTranscriptBlocks(
  entries: readonly CatalogTranscriptEntry[],
  fieldView?: (entry: CatalogTranscriptEntry) => CatalogFieldView | null,
): TranscriptBlock[] {
  return entries.map((item) => {
    const { entry_id, native_action_text, clipped, freshness } = item,
      field = fieldView?.(item),
      entry =
        field?.text === undefined
          ? item.entry
          : item.entry.k === 'tool'
            ? { ...item.entry, out: field.text }
            : { ...item.entry, text: field.text };
    const views: EntryView[] = [];
    let notice = 0;
    const label = (text: string) =>
      views.push({
        kind: 'label',
        key: entry_id + ':notice:' + notice++,
        className: 'vnote',
        text,
      });
    if (entry.k === 'u' || entry.k === 'a')
      views.push({
        kind: 'message',
        key: entry_id,
        entryKey: entry_id,
        flavor: entry.k === 'u' ? 'user' : 'assistant',
        text: entry.text,
      });
    else if (entry.k === 'think')
      views.push({
        kind: 'thought',
        key: entry_id,
        entryKey: entry_id,
        mode: entry.text ? 'readable' : entry.pending ? 'pending' : 'masked',
        text: entry.text,
      });
    else if (entry.k === 'tool') {
      // Until the scoped result/attachment APIs are advertised, preserve only the native preview.
      // Removing expansion handles prevents the shared renderer from requesting workspace endpoints.
      const { more, script, scriptText, scriptTruncated, scriptFailed, ...preview } = entry;
      views.push({
        kind: 'tool',
        key: entry_id,
        entryKey: entry_id,
        step: {
          className: 'step' + (entry.ok === false ? ' err' : ''),
          key: entry_id,
          entryKey: entry_id,
          running: false,
          background: false,
          waiting: false,
          label: entry.title ?? entry.arg,
          named: !!entry.title,
          verb: entry.name,
          status: entry.unfinished
            ? 'Result not recorded'
            : entry.exit == null
              ? null
              : 'exit ' + entry.exit,
          icon: I.run,
          chevron: I.chev,
          data: preview,
        },
      });
      if (more?.length || script || scriptText || scriptTruncated || scriptFailed)
        label('Full result unavailable for this source. The recorded preview is shown.');
    } else if (entry.k === 'h') {
      label('Native action · related session context is incomplete');
      views.push({
        kind: 'message',
        key: entry_id,
        entryKey: entry_id,
        flavor: 'incoming',
        text: native_action_text ?? '',
      });
    } else if (entry.k === 'end') label(entry.text ?? 'Native session end recorded');
    else if (entry.k === 'bgend') label(entry.label ?? entry.state);
    else if (entry.k === 'harness') label(entry.label);
    else if (entry.k === 'signal')
      label('Native ' + entry.signal.kind + (entry.signal.tag ? ' · ' + entry.signal.tag : ''));
    if (field) {
      views.push({
        kind: 'label',
        key: entry_id + ':field',
        className: 'vnote',
        text: field.note,
        action: field.action,
      });
    } else if (clipped) label('Recorded text preview. The complete source text is not loaded.');
    if (freshness && freshness.state !== 'cached')
      label(
        'Source observation is ' + freshness.state + '. Previously loaded content is retained.',
      );
    if (entry.img?.length) label('Attachments unavailable for this source.');
    return { kind: 'loose', key: entry_id, entries: views };
  });
}
