import { render } from 'preact';
import { createPanelChrome } from '../lib/panel';
import { requestJson } from '../lib/api';

export interface PairingCode {
  code: string;
  expires: number;
}
export function parsePairingCode(value: unknown): PairingCode {
  const data = value as Partial<PairingCode> | null;
  if (
    !data ||
    typeof data.code !== 'string' ||
    !/^[0-9A-Z]{5}-[0-9A-Z]{5}$/.test(data.code) ||
    !Number.isSafeInteger(data.expires) ||
    data.expires! <= 0
  )
    throw new Error('Invalid pairing code');
  return { code: data.code, expires: data.expires! };
}
export function PairingContent({
  code,
  pending,
  note,
  copied,
  issue,
  copy,
}: {
  code: PairingCode | null;
  pending: boolean;
  note: string;
  copied: boolean;
  issue(): void;
  copy(): void;
}) {
  const expired = !!code && code.expires * 1000 <= Date.now();
  return (
    <>
      <p>Run this command on the machine you want to pair. Sync starts automatically.</p>
      {code && (
        <>
          <pre class="pair-command">
            <code>{`semon pair ${location.origin} ${code.code}`}</code>
          </pre>
          <p class="sub">
            {expired
              ? 'This code has expired.'
              : 'Expires ' + new Date(code.expires * 1000).toLocaleString() + '. Use it once.'}
          </p>
        </>
      )}
      {note && <p role="status">{note}</p>}
      <div class="btn-row sh-actions">
        {code && (
          <button type="button" class="btn sh-compact" disabled={pending || expired} onClick={copy}>
            {copied ? 'Copied' : 'Copy command'}
          </button>
        )}
        <button type="button" class="btn sh-compact" disabled={pending} onClick={issue}>
          {pending ? 'Getting code…' : code ? 'Get a new code' : 'Get pairing code'}
        </button>
      </div>
      <p class="sub">
        Getting a new code replaces the previous code. Only share it with a machine you want to
        pair.
      </p>
    </>
  );
}
export function openPairMachine(
  host: { opened(panel: { destroy(): void }): void; closed(panel: { destroy(): void }): void },
  trigger: HTMLElement,
) {
  let disposed = false,
    pending = false,
    code: PairingCode | null = null,
    note = '',
    copied = false;
  let request: AbortController | null = null;
  const panel = createPanelChrome(
    { title: 'Pair a machine' },
    {
      opened() {
        host.opened(owner);
      },
      closed() {
        disposed = true;
        request?.abort();
        window.clearInterval(timer);
        render(null, panel.body);
        host.closed(owner);
        if (trigger.isConnected) trigger.focus();
      },
    },
  );
  const owner = {
    destroy() {
      panel.destroy();
    },
  };
  const draw = () => {
    if (!disposed)
      render(
        <PairingContent
          code={code}
          pending={pending}
          note={note}
          copied={copied}
          issue={() => void issue()}
          copy={() => void copy()}
        />,
        panel.body,
      );
  };
  async function issue() {
    if (disposed || pending) return;
    pending = true;
    code = null;
    note = '';
    copied = false;
    draw();
    request = new AbortController();
    try {
      const next = parsePairingCode(
        await requestJson('/api/viewer/pair', request.signal, false, {}),
      );
      if (!disposed) code = next;
    } catch {
      if (!disposed) note = 'Could not get a pairing code. Try again later.';
    } finally {
      pending = false;
      draw();
    }
  }
  async function copy() {
    if (!code || disposed || code.expires * 1000 <= Date.now()) return;
    try {
      await navigator.clipboard.writeText(`semon pair ${location.origin} ${code.code}`);
      if (!disposed) {
        copied = true;
        note = '';
        draw();
      }
    } catch {
      if (!disposed) {
        note = 'Copy is unavailable. Select and copy the command above.';
        draw();
      }
    }
  }
  const timer = window.setInterval(draw, 1000);
  draw();
  panel.show();
  void issue();
  return owner;
}
