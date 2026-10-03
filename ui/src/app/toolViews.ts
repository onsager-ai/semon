import type { ViewerModelStore } from '../state/model';
import type { NavigationController } from '../navigation/routes';
import type { createToolLoader } from './toolLoader';
import type { createSessionChrome } from './sessionChrome';
import type { createTransport } from './transport';
import type { createLiveModel } from './liveModel';
import type { createApplicationRefresh } from './applicationRefresh';
import type { createDomain } from '../domain/calculations';
import type { createScreenViews } from './screenViews';
import type { createTranscriptView } from './transcriptView';
import type { createRecentNavigation } from './recentNavigation';
import { preview, shortModel } from '../domain/format';
import type { Cost, Entry, Handoff, Session } from '../domain/types';
import { object, optional, text } from '../domain/validate';
import type { Attachment, ChildView, FooterView } from '../lib';
import { createImageViewer, renderFullTool } from '../lib';
import type { ToolData } from '../lib/tool-details';
import { I, STATE } from './registry';
type ToolEntry = Extract<Entry, { k: 'tool' }> & { full?: boolean; scriptLoaded?: boolean };
interface ToolViewsHost {
  disposed: boolean;
  navigation: NavigationController;
  dialogs: Map<HTMLDialogElement, { destroy(): void }>;
  dur: (a: number, b: number | null | undefined) => string;
  clock: (t: number) => string;

  recentNavigation: Pick<ReturnType<typeof createRecentNavigation>, 'COST_TIP'>;

  transcriptView: Pick<ReturnType<typeof createTranscriptView>, 'verbNow'>;

  screenViews: Pick<ReturnType<typeof createScreenViews>, 'harnessSnapshot'>;

  domain: Pick<
    ReturnType<typeof createDomain>,
    'countOf' | 'costText' | 'costForSession' | 'nameOf' | 'asMoney' | 'costMissing' | 'callsText'
  >;

  applicationRefreshOwner: Pick<ReturnType<typeof createApplicationRefresh>, 'refresh'>;

  liveModelOwner: Pick<ReturnType<typeof createLiveModel>, 'LIVE'>;

  transportOwner: Pick<ReturnType<typeof createTransport>, 'enc' | 'api'>;

  modelStore: Pick<ViewerModelStore, 'sessions' | 'holds' | 'starts'>;

  sessionChrome: Pick<ReturnType<typeof createSessionChrome>, 'panel' | 'kindText'>;

  toolLoaderOwner: Pick<ReturnType<typeof createToolLoader>, 'fullOf'>;
}
/** Owns toolViews behavior through explicit application ports. */
export function createToolViews(host: ToolViewsHost) {
  let viewerEl: HTMLDialogElement | null = null;
  let skipPop = false;
  // What a step shows opened: what was asked first (the command, the file, the input), then what came back. A failed command
  // shows the end of its output, where the failure is; anything else shows the start. "View all" opens the whole call.
  // The whole call, in a full sheet: what the preview cut, fetched from the server when it is longer than the preview.
  function openStepViewer(e: ToolEntry, v: string, ic: string, inLabel: string = 'Input') {
    if (host.disposed) return;
    if (e.more?.length && e.slot != null && !e.full) {
      const open = (f: Partial<Omit<ToolData, 'bg'>>) =>
        openStepViewer({ ...e, ...f, full: true }, v, ic, inLabel);
      host.toolLoaderOwner.fullOf(e).then(open, () => open({ fullFailed: true }));
      return;
    }
    const status = e.live
      ? 'Running · ' + e.secs
      : e.unfinished
        ? 'No result'
        : e.ok
          ? 'Done · ' + e.secs
          : e.ok === null
            ? 'Exit unknown · ' + e.secs
            : 'Failed · ' + e.secs;
    const { body, show: open } = host.sessionChrome.panel(v + ' ' + e.arg, {
      cls: 'full',
      sub: e.name + ' · ' + status,
      label: v + ' ' + e.arg,
    });
    body.classList.add('viewer-b');
    renderFullTool(body, e, inLabel, host.modelStore.sessions[e.sid ?? '']?.state === 'wait');
    open();
  }

  // Images a prompt attached: thumbnails above its text, each opening the image whole in the viewer sheet. The page never holds
  // their bytes: /api/tx names each image by its line, block and content version, /api/attachment serves it, and the <img> is
  // made here with its src set to that URL. Its box is sized from the width and height /api/tx read from the image's header
  // (at most 200×160, its shape kept), so nothing moves when it loads; one whose size isn't known gets a fixed box. An image the
  // logs don't hold (a path, a redacted copy) is a quiet chip, and so is one that fails to load.
  const THUMB_W = 200,
    THUMB_H = 160;
  const IMAGE_KIND: Record<string, string> = {
    'image/png': 'PNG',
    'image/jpeg': 'JPEG',
    'image/gif': 'GIF',
    'image/webp': 'WebP',
  };
  const sizeText = (n: number) =>
    n >= 1048576
      ? (n / 1048576).toFixed(1) + ' MB'
      : n >= 1024
        ? Math.round(n / 1024) + ' KB'
        : n + ' bytes';
  function attachmentSnapshot(e: Entry): Attachment[] {
    const images = (e.img ?? []).map((a, i) => {
      const k =
        a.w != null && a.h != null && a.w > 0 && a.h > 0
          ? Math.min(1, THUMB_W / a.w, THUMB_H / a.h)
          : null;
      return {
        unavailable: !!a.na,
        url:
          '/api/attachment?sid=' +
          host.transportOwner.enc(e.sid ?? '') +
          '&o=' +
          host.transportOwner.enc(a.o) +
          '&b=' +
          host.transportOwner.enc(a.b) +
          '&v=' +
          host.transportOwner.enc(a.v ?? ''),
        label:
          'Attached image ' +
          (i + 1) +
          ' (' +
          (IMAGE_KIND[a.type ?? ''] ?? 'image') +
          ', ' +
          sizeText(a.size ?? 0) +
          ')',
        width: k ? Math.max(1, Math.round(a.w! * k)) : undefined,
        height: k ? Math.max(1, Math.round(a.h! * k)) : undefined,
      };
    });
    return images;
  }
  function openImage(url: string, label: string, from: HTMLButtonElement) {
    const image = createImageViewer(url, label, {
      opened(d) {
        viewerEl = d;
        document.documentElement.classList.add('viewer-open');
        try {
          history.pushState({ ...host.navigation.route, sheet: 1 }, '');
        } catch {}
      },
      closed(d) {
        host.dialogs.delete(d);
        if (host.disposed) return;
        document.documentElement.classList.remove('viewer-open');
        if (viewerEl === d) {
          viewerEl = null;
          if (history.state?.sheet) {
            skipPop = true;
            history.back();
          }
        }
        if (host.liveModelOwner.LIVE.pending) host.applicationRefreshOwner.refresh();
        (from.isConnected
          ? from
          : [...document.querySelectorAll<HTMLElement>('button.attach')].find(
              (x) => x.querySelector('img')?.getAttribute('src') === url,
            )
        )?.focus();
      },
    });
    host.dialogs.set(image.dialog, image);
    image.show();
  }

  function openScript(e: ToolEntry) {
    const done = (fields: Partial<Omit<ToolData, 'bg'>>) =>
      openStepViewer({ ...e, ...fields, full: true }, 'View script', 'run', 'Script');
    host.transportOwner
      .api(
        '/api/entry?sid=' + host.transportOwner.enc(e.sid ?? '') + '&slot=' + e.slot + '&as=script',
      )
      .then((value) => {
        const result = object(value);
        done({
          scriptText: optional(result.text, text),
          scriptTruncated: result.truncated === true,
        });
      })
      .catch(() => done({ scriptFailed: true }));
  }

  // A brief or message clamped to three lines; "Show more" opens it in place, and only appears when it is cut.
  function childSnapshot(h: Handoff, c: Session): ChildView {
    const calls = host.domain.countOf(c, 'calls'),
      holder = host.modelStore.holds.get(h.id);
    return {
      kind: 'child',
      handoff: h.id,
      id: c.id,
      turn: host.modelStore.starts.get(h.id)?.id,
      name: c.name,
      state: c.state,
      stateLabel: STATE[c.state] ?? c.state,
      meta: [
        host.sessionChrome.kindText(c),
        shortModel(c.model),
        host.dur(c.start, c.state === 'work' ? null : c.last),
        (calls ?? '—') + (calls === 1 ? ' step' : ' steps'),
        host.domain.costText(host.domain.costForSession(c.id, true)),
      ].join(' · '),
      mark: host.screenViews.harnessSnapshot(c.harness),
      brief: preview(h.brief ?? ''),
      result: h.result || undefined,
      failed: h.status === 'err',
      activity:
        !h.result && c.state === 'work' && c.activity
          ? [host.transcriptView.verbNow(c.activity[0]), c.activity[1]]
          : undefined,
      trace: holder?.id,
      traceLabel: holder ? 'Open run view for ' + host.domain.nameOf(h.from) : undefined,
      chevron: I.chev,
    };
  }
  // The tips on a session footer's items, as plain text (the tooltip sets it with textContent): calls by tool, times, cost by kind.
  const callsTip = (s: Session) => {
    const parts = Object.entries(s.tool_calls ?? {})
        .sort((a, b) => b[1] - a[1] || (a[0] < b[0] ? -1 : 1))
        .map(([name, n]) => name + ' ' + n),
      failed = host.domain.countOf(s, 'errors');
    if (failed) parts.push(failed + ' failed');
    return parts.join(' · ');
  };
  const timeTip = (s: Session, h: Handoff | undefined, finished: boolean) =>
    'Started ' +
    host.clock(s.start) +
    ' · last activity ' +
    host.clock(s.last) +
    (finished ? ' · finished ' + host.clock(h?.done ?? s.last) : '');
  function costTip(cost: Cost) {
    const groups = Object.entries(cost.by_model ?? {}).map(([id, m]) => {
      const k = m.usd_by_kind ?? {},
        kinds = [
          ['Input', k.input],
          ['Output', k.output],
          ['Cache read', k.cache_read],
          ['Cache write', (Number(k.cache_write_5m) || 0) + (Number(k.cache_write_1h) || 0)],
        ];
      if (Number(k.web_search) >= 0.005) kinds.push(['Web search', k.web_search]);
      return {
        id,
        text: kinds
          .map(([label, usd]) => label + ' ' + host.domain.asMoney(Number(usd) || 0))
          .join(' · '),
      };
    });
    return (
      (groups.length > 1
        ? groups.map((g) => g.id + ': ' + g.text).join('; ')
        : groups.map((g) => g.text).join('')) +
      (groups.length ? '. ' : '') +
      host.recentNavigation.COST_TIP
    );
  }
  // What a footer shows: its text, its tips, its state, and whether it has the button to the parent.
  // The line a session page ends in, for a child (h: its origin) and any other session alike: the state, the calls, the time and
  // the API-equivalent cost, each with its breakdown as a tip. A returned child keeps "Returned to <parent>" and its button.
  function footerSnapshot(s: Session, h: Handoff | undefined): FooterView {
    const done = s.state === 'done' || s.state === 'err',
      finished = h ? done || h.status === 'done' || h.status === 'err' : done;
    const state = h
      ? finished
        ? s.state === 'err' || h.status === 'err'
          ? 'err'
          : 'done'
        : 'work'
      : s.state;
    const cost = host.domain.costForSession(s.id),
      priced = cost.usd != null && !host.domain.costMissing(cost).length && cost.usd >= 0.005,
      items: FooterView['items'][number][] = [];
    if (!h || !finished)
      items.push({
        kind: 'calls',
        text: host.domain.callsText(host.domain.countOf(s, 'calls')),
        tip: callsTip(s),
      });
    items.push({
      kind: 'time',
      text: host.dur(s.start, state === 'work' ? null : s.last),
      tip: timeTip(s, h, finished),
    });
    if (priced)
      items.push({ kind: 'cost', text: host.domain.asMoney(cost.usd ?? 0), tip: costTip(cost) });
    return {
      state,
      stateLabel: STATE[state] ?? state,
      text:
        h && finished
          ? 'Returned to ' + host.domain.nameOf(h.from) + ' · ' + STATE[state]
          : STATE[state],
      items,
      parent:
        h && finished
          ? {
              id: h.from,
              turn: host.modelStore.holds.get(h.id)?.id,
              name: host.domain.nameOf(h.from),
            }
          : undefined,
    };
  }

  return {
    get viewerEl() {
      return viewerEl;
    },
    set viewerEl(value: HTMLDialogElement | null) {
      viewerEl = value;
    },
    get skipPop() {
      return skipPop;
    },
    set skipPop(value: boolean) {
      skipPop = value;
    },
    openStepViewer,
    openScript,
    openImage,
    attachmentSnapshot,
    childSnapshot,
    footerSnapshot,
  };
}
