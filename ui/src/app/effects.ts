/** Document effects are owned by one mount and cannot call back after teardown. */
export class EffectScope {
  private readonly cleanups = new Set<() => void>();
  private disposed = false;
  own(cleanup: () => void) { if (this.disposed) cleanup(); else this.cleanups.add(cleanup); return cleanup; }
  listen<K extends keyof DocumentEventMap>(target: Document, type: K, listener: (event: DocumentEventMap[K]) => void, options?: AddEventListenerOptions | boolean): void;
  listen<K extends keyof WindowEventMap>(target: Window, type: K, listener: (event: WindowEventMap[K]) => void, options?: AddEventListenerOptions | boolean): void;
  listen<K extends keyof HTMLElementEventMap>(target: HTMLElement, type: K, listener: (event: HTMLElementEventMap[K]) => void, options?: AddEventListenerOptions | boolean): void;
  listen(target: EventTarget, type: string, listener: EventListener, options?: AddEventListenerOptions | boolean): void;
  listen(target: EventTarget, type: string, listener: EventListener, options?: AddEventListenerOptions | boolean) {
    if (this.disposed) return; const guarded: EventListener = event => { if (!this.disposed) listener(event); };
    target.addEventListener(type, guarded, options); this.own(() => target.removeEventListener(type, guarded, options));
  }
  timeout(callback: () => void, delay = 0): number { if (this.disposed) return 0; let cleanup: () => void; const id = window.setTimeout(() => { this.cleanups.delete(cleanup); if (!this.disposed) callback(); }, delay); cleanup = this.own(() => clearTimeout(id)); return id; }
  interval(callback: () => void, delay: number): number { if (this.disposed) return 0; const id = window.setInterval(() => { if (!this.disposed) callback(); }, delay); this.own(() => clearInterval(id)); return id; }
  frame(callback: FrameRequestCallback): number { if (this.disposed) return 0; let cleanup: () => void; const id = requestAnimationFrame(time => { this.cleanups.delete(cleanup); if (!this.disposed) callback(time); }); cleanup = this.own(() => cancelAnimationFrame(id)); return id; }
  request(): AbortController { const controller = new AbortController(); this.own(() => controller.abort()); return controller; }
  destroy() { if (this.disposed) return; this.disposed = true; for (const cleanup of this.cleanups) cleanup(); this.cleanups.clear(); }
}
