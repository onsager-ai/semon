import { render } from 'preact';
import { Markdown } from './richtext';
export interface Attachment { unavailable: boolean; url: string; label: string; width?: number; height?: number }
export interface AttachmentHost { open(url: string, label: string, trigger: HTMLButtonElement): void }
export function renderUserBody(root: HTMLElement, text: string, images: readonly Attachment[], host: AttachmentHost) {
  const failed = new Set<number>();
  function paint() {
    render(<>{images.length > 0 && <div class="attach-row">{images.map((image, i) => image.unavailable || failed.has(i) ? <span key={i} class="attach-na">Image not available</span> : <button key={i} class="attach" type="button" aria-haspopup="dialog" onClick={event => { if (event.currentTarget.isConnected) host.open(image.url, image.label, event.currentTarget); }}><img class={'attach-img' + (image.width ? '' : ' unsized')} alt={image.label} loading="lazy" decoding="async" width={image.width} height={image.height} src={image.url} onError={() => { failed.add(i); paint(); }} /></button>)}</div>}{(!images.length || text) && <Markdown text={text} />}</>, root);
  }
  paint();
}
export interface ImageViewerHost { opened(dialog: HTMLDialogElement): void; closed(dialog: HTMLDialogElement): void }
export interface ImageViewer { dialog: HTMLDialogElement; show(): void; destroy(): void }
export function createImageViewer(url: string, label: string, host: ImageViewerHost): ImageViewer {
  const dialog = document.createElement('dialog'); dialog.className = 'viewer image-viewer'; dialog.setAttribute('aria-label', label);
  let disposed = false, shown = false, failed = false, close: HTMLButtonElement | null = null;
  function finish() {
    if (disposed) return; disposed = true;
    dialog.removeEventListener('close', finish); dialog.removeEventListener('click', backdrop);
    if (dialog.open) dialog.close(); render(null, dialog); dialog.remove();
    if (shown) host.closed(dialog);
  }
  function backdrop(event: MouseEvent) { if (event.target === dialog || (event.target instanceof Element && event.target.classList.contains('vb'))) dialog.close(); }
  function paint() {
    if (disposed) return;
    render(<><div class="vh"><div class="vt"><span>{label}</span></div><button class="vclose" type="button" aria-label="Close" ref={node => { close = node; }} onClick={() => { if (!disposed) dialog.close(); }}><svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18" /></svg></button></div><div class="vb">{failed ? <p class="vnote">Image not available.</p> : <img class="attach-full" alt={label} decoding="async" src={url} onError={() => { failed = true; paint(); }} />}</div></>, dialog);
  }
  paint(); dialog.addEventListener('close', finish); dialog.addEventListener('click', backdrop);
  return { dialog, show() { if (shown || disposed) return; shown = true; document.body.append(dialog); dialog.showModal(); close?.focus(); host.opened(dialog); }, destroy: finish };
}
