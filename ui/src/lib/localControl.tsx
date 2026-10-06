import { useState } from 'preact/hooks';
import type { ControlRequest, ControlView } from './control';
import type { JsonObject } from './model';
function visible(value: unknown): string {
  return String(value ?? '').replace(
    /[\u00ad\u202a-\u202e\u2066-\u2069]/g,
    (char) => '\\u' + char.charCodeAt(0).toString(16).padStart(4, '0'),
  );
}
function Question({ request, view }: { request: ControlRequest; view: ControlView }) {
  const [answers, setAnswers] = useState<Record<string, string>>({});
  const params = request.payload.params;
  const questions =
    params && typeof params === 'object' && !Array.isArray(params) ? params.questions : null;
  if (!Array.isArray(questions)) return <p>Unsupported native question. Answer in Codex.</p>;
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        const result: JsonObject = {};
        for (const [id, value] of Object.entries(answers)) result[id] = [value];
        view.answer(request, { answers: result });
      }}
    >
      {questions.map((value) => {
        if (
          !value ||
          typeof value !== 'object' ||
          Array.isArray(value) ||
          typeof value.id !== 'string' ||
          typeof value.question !== 'string'
        )
          return null;
        const id = String(value.id),
          label = visible(value.question);
        return (
          <label key={id}>
            {label}
            <input
              aria-label={label}
              value={answers[id] ?? ''}
              list={'options-' + request.id + '-' + id}
              onInput={(event) => setAnswers({ ...answers, [id]: event.currentTarget.value })}
            />
            <datalist id={'options-' + request.id + '-' + id}>
              {Array.isArray(value.options) &&
                value.options.map((option) =>
                  option &&
                  typeof option === 'object' &&
                  !Array.isArray(option) &&
                  typeof option.label === 'string' ? (
                    <option value={option.label} />
                  ) : null,
                )}
            </datalist>
          </label>
        );
      })}
      <button
        type="submit"
        disabled={
          view.busy ||
          view.uncertain ||
          !view.snapshot.capabilities.questions ||
          !!request.reason ||
          request.state.state !== 'open' ||
          request.remainingMs === 0
        }
      >
        Answer
      </button>
    </form>
  );
}
function Permission({ request }: { request: ControlRequest }) {
  const p = request.payload.params;
  if (!p || typeof p !== 'object' || Array.isArray(p))
    return <p>Exact request is no longer retained.</p>;
  const item = request.payload.item;
  const changes = item && typeof item === 'object' && !Array.isArray(item) ? item.changes : null;
  return (
    <>
      {typeof p.command === 'string' && (
        <>
          <pre>{visible(p.command)}</pre>
          <p>Working directory: {visible(p.cwd)}</p>
        </>
      )}
      {Array.isArray(changes) &&
        changes.map((change) =>
          change && typeof change === 'object' && !Array.isArray(change) ? (
            <div>
              <p>{visible(change.path)}</p>
              <pre>{visible(change.diff)}</pre>
            </div>
          ) : null,
        )}
      {typeof p.reason === 'string' && <p>{visible(p.reason)}</p>}
      {typeof p.command !== 'string' && !Array.isArray(changes) && (
        <pre>{visible(JSON.stringify(request.payload, null, 2))}</pre>
      )}
    </>
  );
}
export function LocalControl({ view }: { view: ControlView }) {
  const [text, setText] = useState('');
  const s = view.snapshot;
  const canSend =
    !view.busy &&
    !view.uncertain &&
    s.connected &&
    (s.activeTurn ? s.capabilities.steer : s.capabilities.input) &&
    !!text.trim();
  const showStop = !!s.activeTurn && s.capabilities.interrupt && !text.trim();
  const pending = s.requests.filter((r) => r.state.state === 'open' || r.state.state === 'claimed');
  const recent = s.requests
    .filter((r) => r.state.state !== 'open' && r.state.state !== 'claimed')
    .slice(-10);
  return (
    <section class="local-control" aria-label="Conversation controls">
      {!s.connected && <p role="status">Reconnecting to your session…</p>}
      {s.reason && <p>{s.reason}</p>}
      {pending.map((request) => {
        const supported =
          request.kind === 'question'
            ? s.capabilities.questions
            : request.payload.method === 'item/fileChange/requestApproval'
              ? s.capabilities.fileApproval
              : s.capabilities.commandApproval;
        const disabled =
          view.busy ||
          view.uncertain ||
          !supported ||
          !!request.reason ||
          request.state.state !== 'open' ||
          request.remainingMs === 0;
        return (
          <article key={request.id}>
            <h3>{request.kind === 'question' ? 'Codex question' : 'Codex permission'}</h3>
            {request.reason && <p>{request.reason}</p>}
            {request.state.state === 'claimed' && (
              <p>Response sent; waiting for Codex to answer or clear the request.</p>
            )}
            {request.kind === 'question' ? (
              <Question request={request} view={view} />
            ) : (
              <>
                <Permission request={request} />
                <div>
                  <button
                    type="button"
                    disabled={disabled}
                    onClick={() => view.answer(request, { decision: 'allow' })}
                  >
                    Allow once
                  </button>
                  <button
                    type="button"
                    disabled={disabled}
                    onClick={() => view.answer(request, { decision: 'deny' })}
                  >
                    Deny
                  </button>
                </div>
              </>
            )}
          </article>
        );
      })}
      <form
        class="sh-composer sh-composer-conversation"
        aria-label="Send a follow-up"
        aria-busy={view.busy || undefined}
        onSubmit={async (event) => {
          event.preventDefault();
          if (!canSend) return;
          const submitted = text;
          if (await view.send(submitted))
            setText((current) => (current === submitted ? '' : current));
        }}
      >
        <textarea
          class="sh-composer-input"
          aria-label="Message to Codex"
          aria-description="Enter to send. Shift+Enter for a new line."
          rows={Math.min(6, Math.max(1, text.split('\n').length))}
          placeholder={s.activeTurn ? 'Add instructions…' : 'Message Codex…'}
          value={text}
          onInput={(event) => setText(event.currentTarget.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
              event.preventDefault();
              if (canSend) event.currentTarget.form?.requestSubmit();
            }
          }}
        />
        <button
          type={showStop ? 'button' : 'submit'}
          class="ibtn sh-composer-send"
          aria-label={showStop ? 'Stop response' : 'Send'}
          data-tip={showStop ? 'Stop response' : 'Send message'}
          disabled={showStop ? view.busy || view.uncertain : !canSend}
          onClick={() => {
            if (showStop) view.interrupt();
          }}
        >
          {view.busy ? (
            <span class="spin" aria-hidden="true" />
          ) : (
            <svg
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
              aria-hidden="true"
            >
              {showStop ? (
                <rect x="6" y="6" width="12" height="12" rx="2" fill="currentColor" stroke="none" />
              ) : (
                <path d="M12 19V5m-6 6 6-6 6 6" />
              )}
            </svg>
          )}
        </button>
      </form>
      {(!s.connected || view.uncertain) && (
        <div class="local-control-actions">
          <button
            type="button"
            class="btn sh-quiet"
            disabled={view.busy}
            onClick={() => view.reconnect()}
          >
            Reconnect
          </button>
        </div>
      )}
      {view.note && <p role="status">{view.note}</p>}
      {recent.length > 0 && (
        <details>
          <summary>Recent requests</summary>
          <ul>
            {recent.map((r) => (
              <li key={r.id}>
                {r.kind === 'question' ? 'Question' : 'Permission'} ·{' '}
                {r.state.reason === 'answered_or_cleared'
                  ? 'Answered or cleared'
                  : (r.state.reason ?? r.state.state)}
              </li>
            ))}
          </ul>
        </details>
      )}
    </section>
  );
}
