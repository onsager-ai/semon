import { render } from 'preact';
import { useState } from 'preact/hooks';
import { screenText } from './screens';
export interface OutputGap { unit: string; n: number; of?: number }
export interface ToolData {
  name: string; arg: string; in?: string; out?: string; cwd?: string; live?: boolean; unfinished?: boolean; ok?: boolean | null;
  diff?: readonly (readonly [string, string])[]; changes?: readonly { path: string; move?: string; diff?: readonly (readonly [string, string])[] }[];
  script?: unknown; scriptText?: string; scriptTruncated?: boolean; scriptFailed?: boolean; more?: readonly string[];
  cut?: { original_tokens?: number; parts?: readonly { gap?: OutputGap; text?: string }[] };
  fullFailed?: boolean; fullCut?: readonly string[]; bg?: { summary?: string };
}
export interface ToolDetailsHost { all(inputLabel: string): void; script(): void }
const isCommand = (name: string) => /^(Bash|shell|exec_command|local_shell)$/.test(name);
const gapText = (gap: OutputGap) => (gap.unit === 'tokens' ? 'About ' : '') + (gap.unit === 'lines' && gap.of != null ? gap.n.toLocaleString('en-US') + ' of ' + gap.of.toLocaleString('en-US') + ' lines' : gap.n.toLocaleString('en-US') + ' ' + (gap.unit === 'chars' ? 'characters' : gap.unit)) + ' cut here by Codex';
export const cutNoteText = (cut: NonNullable<ToolData['cut']>) => 'Codex cut this output before the model saw it' + (cut.original_tokens ? ' (about ' + cut.original_tokens.toLocaleString('en-US') + ' tokens in all)' : '') + '.';
function Icon({ path }: { path: string }) { return <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d={path} /></svg>; }
const COPY = 'M9 9h11v11H9zM5 15H4V4h11v1';
const EXPAND = 'M15 3h6v6M9 21H3v-6M21 3l-7 7M3 21l7-7';
function Copy({ text }: { text: string }) {
  const [label, setLabel] = useState('Copy');
  return <button class="link copy" type="button" aria-label="Copy" onClick={event => { const button = event.currentTarget; if (!button.isConnected) return; navigator.clipboard?.writeText(text).then(() => { if (button.isConnected) setLabel('Copied'); }, () => { if (button.isConnected) setLabel('Copy failed'); }); }}><Icon path={COPY} /><span>{label}</span></button>;
}
function Header({ label, text, className = 'io' }: { label: string; text?: string; className?: string }) { return <div class={className}><span>{screenText(label)}</span>{text && <Copy text={text} />}</div>; }
function Diff({ rows }: { rows: NonNullable<ToolData['diff']> }) { return <div class="diff">{rows.map(([cls, text], i) => <div key={i} class={cls}>{screenText(text)}</div>)}</div>; }
function Output({ data }: { data: ToolData }) {
  return data.cut?.parts?.length ? <div class="cutout">{data.cut.parts.map((part, i) => part.gap ? <div key={i} class="cutgap">{screenText(gapText(part.gap))}</div> : <pre key={i}>{part.text}</pre>)}</div> : <pre>{data.out}</pre>;
}
function ViewAll({ label, action }: { label: string; action(): void }) { return <button class="viewall" type="button" onClick={event => { if (event.currentTarget.isConnected) action(); }}><Icon path={EXPAND} /><span>{label}</span></button>; }
export function ToolPreview({ data, waiting, host }: { data: ToolData; waiting: boolean; host: ToolDetailsHost }) {
  const command = data.in ?? (isCommand(data.name) ? data.arg : null), inCut = typeof command === 'string' && command.split('\n').length > 12;
  const inputLabel = command && isCommand(data.name) ? 'Command' : 'Input';
  const lines = (data.out ?? '').split('\n'), cut = lines.length > 12, tail = data.ok === false, parts = !!data.cut?.parts?.length, more = !!data.more?.length;
  const shown = !cut || parts ? lines : tail ? lines.slice(-12) : lines.slice(0, 12);
  const changed = !!data.changes || !!data.diff;
  const all = () => host.all(inputLabel);
  return <>{command ? <><Header label={inputLabel} text={command} /><pre class={'in' + (inCut ? ' clip clipped' : '')}>{command}</pre></> : !changed && <><Header label={/^(Read|Grep|Glob)$/.test(data.name) ? data.name === 'Read' ? 'File' : 'Pattern' : 'Input'} /><pre class="in">{data.arg}</pre></>}{data.cwd && data.cwd !== '.' && <Header label={'Working directory · ' + data.cwd} />}
    {data.changes ? <>{data.changes.map((change, i) => <><Header key={'head' + i} label={'Change · ' + change.path + (change.move ? ' → ' + change.move : '')} />{change.diff?.length ? <Diff key={'diff' + i} rows={change.diff} /> : <div class="noout">No diff recorded</div>}</>)}{!data.changes.length && <div class="noout">No changes recorded</div>}</> : data.diff ? <><Header label={'Change · ' + data.arg} /><Diff rows={data.diff} /></> : !data.out ? <><Header label="Output" /><div class="noout">{screenText(data.live ? waiting ? 'Waiting for your input or permission · no output yet' : 'Running · no output yet' : data.unfinished ? 'No result recorded: the machine stopped responding while this ran.' : 'No output')}</div></> : <><Header label={parts || !cut ? 'Output' : (tail ? 'Output · last ' : 'Output · first ') + '12' + (more ? '' : ' of ' + lines.length) + ' lines'} />{parts ? <Output data={data} /> : <pre>{shown.join('\n')}</pre>}{data.cut ? <div class="cutnote">{screenText(cutNoteText(data.cut))}</div> : [data.in, data.out].some(text => /…(\(truncated\))?\s*$/.test(text ?? '')) && <div class="cutnote">Cut short in this copy of the logs</div>}</>}
    {(inCut || !changed && !!data.out && ((cut && !parts) || more)) && <ViewAll label={changed || more || parts || inCut ? 'View all' : 'View all ' + lines.length + ' lines'} action={all} />}{data.script != null && <button class="viewall viewscript" type="button" onClick={event => { if (event.currentTarget.isConnected) host.script(); }}><Icon path={EXPAND} /><span>View script</span></button>}{data.bg?.summary && <><div class="io">Finished</div><pre class="finished">{data.bg.summary}</pre></>}</>;
}
export function createStepDetail(data: ToolData, waiting: boolean, host: ToolDetailsHost): HTMLDivElement {
  const root = document.createElement('div'); root.className = 'out'; render(<ToolPreview data={data} waiting={waiting} host={host} />, root); return root;
}
function Note({ data, text }: { data: ToolData; text?: string }) { return data.cut ? <p class="vnote">{screenText(cutNoteText(data.cut))}</p> : !data.fullFailed && /…(\(truncated\))?\s*$/.test(text ?? '') ? <p class="vnote">Cut short in this copy of the logs.</p> : null; }
export function renderFullTool(root: HTMLElement, data: ToolData, inputLabel: string, waiting: boolean) {
  render(<>{data.scriptText !== undefined ? <><Header className="vs" label="Script" text={data.scriptText} /><pre class="script">{data.scriptText}</pre>{data.scriptTruncated && <p class="vnote">Cut at 8 MB: the rest isn't shown.</p>}</> : data.scriptFailed ? <p class="vnote">Couldn't load the script from these logs.</p> : <>{data.in && <><Header className="vs" label={inputLabel} text={data.in} /><pre class="in">{data.in}</pre><Note data={data} text={data.in} /></>}{data.changes ? <>{data.changes.map((change, i) => <><Header key={i} className="vs" label={'Change · ' + change.path + (change.move ? ' → ' + change.move : '')} /><Diff rows={change.diff ?? []} /></>)}{!data.changes.length && <p class="vnote">No changes recorded.</p>}</> : data.diff ? <><Header className="vs" label="Change" /><Diff rows={data.diff} /></> : <><Header className="vs" label="Output" text={data.out} />{data.out ? <><Output data={data} /><Note data={data} text={data.out} /></> : <p class="vnote">{screenText(data.live ? waiting ? 'Waiting for your input or permission · no output yet' : 'Running · no output yet' : data.unfinished ? 'No result recorded.' : 'No output.')}</p>}</>}</>}{data.bg?.summary && <><Header className="vs" label="Finished" text={data.bg.summary} /><pre>{data.bg.summary}</pre></>}{data.fullFailed && <p class="vnote">Couldn't load the full text: this is the preview.</p>}{!!data.fullCut?.length && <p class="vnote">Cut at 8 MB: the rest isn't shown.</p>}</>, root);
}
