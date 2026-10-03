import { render } from 'preact';
import { claimScreen, releaseScreen, screenText } from './screens';
const shapes = [[true,[1,3,5],true],[false,[1,2,4],true],[true,[2,1,3],false],[false,[1,1,5],true]] as const;
export function renderPlaceholder(root: HTMLElement, error?: string, retry?: () => void) {
  releaseScreen(root); claimScreen(root,'placeholder',() => {});
  render(error ? <div class="load-error" role="alert"><p class="empty">{screenText(error)}</p>{retry && <button class="more" type="button" onClick={event => { if (event.currentTarget.isConnected) retry(); }}>Try again</button>}</div> : <div class="skeleton" aria-hidden="true">{Array.from({ length: 6 },(_,i) => { const [bubble,lines,step] = shapes[i % shapes.length]!; return <div key={i} class="sk-turn">{bubble && <span class="sk-line sk-bubble sk-w3" />}<div class="sk-text">{lines.map((width,j) => <span key={j} class={'sk-line sk-w' + width} />)}</div>{step && <span class="sk-line sk-step sk-w2" />}</div>; })}</div>,root);
}
export function createStatusNote(text: string, className: string, role: 'status' | 'alert' = 'status') { const root = document.createElement('div'); root.className = 'viewer-status-slot'; render(<p class={className} role={role}>{screenText(text)}</p>,root); return root; }

export function createLiveRegion() { const root = document.createElement("div"); root.className = "sr-only"; root.setAttribute("role", "status"); root.setAttribute("aria-live", "polite"); return root; }
