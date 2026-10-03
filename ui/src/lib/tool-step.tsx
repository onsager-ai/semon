import { render } from 'preact';
import { screenText } from './screens';
import { ToolPreview } from './tool-details';
import type { ToolData, ToolDetailsHost } from './tool-details';
export interface ToolStepSnapshot {
  className: string;
  key?: string;
  entryKey?: string;
  tid?: string;
  sid?: string;
  since?: number;
  running: boolean;
  background: boolean;
  waiting: boolean;
  prefix?: string;
  verb?: string;
  label: string;
  named: boolean;
  tip?: string;
  status: string | null;
  backgroundStatus?: string;
  icon: string;
  chevron: string;
  data: ToolData;
}
export function createToolStep(snapshot: ToolStepSnapshot, host: ToolDetailsHost): HTMLDivElement {
  const root = document.createElement('div');
  root.className = snapshot.className;
  if (snapshot.key) root.dataset.e = snapshot.key;
  if (snapshot.entryKey) root.dataset.entryKey = snapshot.entryKey;
  if (snapshot.tid) root.dataset.tid = snapshot.tid;
  if (snapshot.sid) root.dataset.live = snapshot.sid;
  if (snapshot.since != null) root.dataset.since = String(snapshot.since);
  let expanded = false,
    mounted = false;
  function paint() {
    render(
      <ToolStepContents
        snapshot={snapshot}
        expanded={expanded}
        mounted={mounted}
        toggle={() => {
          mounted = true;
          expanded = !expanded;
          paint();
        }}
        host={host}
      />,
      root,
    );
  }
  paint();
  return root;
}

export function ToolStepContents({
  snapshot,
  expanded,
  mounted,
  toggle,
  host,
}: {
  snapshot: ToolStepSnapshot;
  expanded: boolean;
  mounted: boolean;
  toggle(): void;
  host: ToolDetailsHost;
}) {
  function icon(path: string, cls: string) {
    return (
      <svg
        class={cls}
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="1.8"
        stroke-linecap="round"
        stroke-linejoin="round"
        aria-hidden="true"
      >
        <path d={path} />
      </svg>
    );
  }
  return (
    <>
      <button
        type="button"
        aria-expanded={expanded}
        onClick={(event) => {
          if (event.currentTarget.isConnected) toggle();
        }}
      >
        {snapshot.named && <span class="sr-only">{screenText(snapshot.prefix ?? '')}</span>}
        {snapshot.running ? <span class="spin" /> : icon(snapshot.icon, 'icon')}
        {snapshot.background && <span class="bgmark" role="img" aria-label="background" />}
        {!snapshot.named && <span class="sv">{screenText(snapshot.verb ?? '')}</span>}
        {snapshot.named ? (
          <span class="sa st" data-tip={snapshot.tip}>
            {screenText(snapshot.label)}
          </span>
        ) : (
          <code class="sa">{snapshot.label}</code>
        )}
        <span class={'sd' + (snapshot.running ? ' tick' : '')}>
          {screenText(snapshot.status ?? '')}
          {snapshot.background && (
            <>
              <span class="bgw">{'background\u2009 · \u2009'}</span>
              <span class="bgo">{screenText(snapshot.backgroundStatus ?? '')}</span>
            </>
          )}
        </span>
        {icon(snapshot.chevron, 'chev')}
      </button>
      {mounted && (
        <div class="out" hidden={!expanded}>
          <ToolPreview data={snapshot.data} waiting={snapshot.waiting} host={host} />
        </div>
      )}
    </>
  );
}
