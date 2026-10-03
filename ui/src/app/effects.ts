/** Document effects are owned by one mount and cannot call back after teardown. */
export class EffectScope {
  private readonly cleanups = new Set<() => void>();
  private readonly requests = new Map<AbortController, () => void>();
  private readonly timers = new Map<number, () => void>();
  private readonly frames = new Map<number, () => void>();
  private disposed = false;
  own(cleanup: () => void) {
    if (this.disposed) cleanup();
    else this.cleanups.add(cleanup);
    return cleanup;
  }
  listen<K extends keyof DocumentEventMap>(
    target: Document,
    type: K,
    listener: (event: DocumentEventMap[K]) => void,
    options?: AddEventListenerOptions | boolean,
  ): void;
  listen<K extends keyof WindowEventMap>(
    target: Window,
    type: K,
    listener: (event: WindowEventMap[K]) => void,
    options?: AddEventListenerOptions | boolean,
  ): void;
  listen<K extends keyof HTMLElementEventMap>(
    target: HTMLElement,
    type: K,
    listener: (event: HTMLElementEventMap[K]) => void,
    options?: AddEventListenerOptions | boolean,
  ): void;
  listen(
    target: EventTarget,
    type: string,
    listener: EventListener,
    options?: AddEventListenerOptions | boolean,
  ): void;
  listen(
    target: EventTarget,
    type: string,
    listener: EventListener,
    options?: AddEventListenerOptions | boolean,
  ) {
    if (this.disposed) return;
    const guarded: EventListener = (event) => {
      if (!this.disposed) listener(event);
    };
    target.addEventListener(type, guarded, options);
    this.own(() => target.removeEventListener(type, guarded, options));
  }
  private retire(collection: Map<number, () => void>, id: number) {
    const cleanup = collection.get(id);
    if (cleanup) this.cleanups.delete(cleanup);
    collection.delete(id);
  }
  timeout(callback: () => void, delay = 0): number {
    if (this.disposed) return 0;
    const id = window.setTimeout(() => {
      this.retire(this.timers, id);
      if (!this.disposed) callback();
    }, delay);
    this.timers.set(
      id,
      this.own(() => {
        this.retire(this.timers, id);
        window.clearTimeout(id);
      }),
    );
    return id;
  }
  clearTimeout(id: number | undefined) {
    if (id !== undefined) this.timers.get(id)?.();
  }
  interval(callback: () => void, delay: number): number {
    if (this.disposed) return 0;
    const id = window.setInterval(() => {
      if (!this.disposed) callback();
    }, delay);
    this.timers.set(
      id,
      this.own(() => {
        this.retire(this.timers, id);
        window.clearInterval(id);
      }),
    );
    return id;
  }
  clearInterval(id: number | undefined) {
    if (id !== undefined) this.timers.get(id)?.();
  }
  frame(callback: FrameRequestCallback): number {
    if (this.disposed) return 0;
    const id = requestAnimationFrame((time) => {
      this.retire(this.frames, id);
      if (!this.disposed) callback(time);
    });
    this.frames.set(
      id,
      this.own(() => {
        this.retire(this.frames, id);
        cancelAnimationFrame(id);
      }),
    );
    return id;
  }
  cancelFrame(id: number | undefined) {
    if (id !== undefined) this.frames.get(id)?.();
  }
  request(): AbortController {
    const controller = new AbortController();
    if (this.disposed) {
      controller.abort();
      return controller;
    }
    const aborted = () => this.releaseRequest(controller),
      cleanup = this.own(() => controller.abort());
    const release = () => {
      controller.signal.removeEventListener('abort', aborted);
      this.cleanups.delete(cleanup);
      this.requests.delete(controller);
    };
    this.requests.set(controller, release);
    controller.signal.addEventListener('abort', aborted, { once: true });
    return controller;
  }
  releaseRequest(controller: AbortController) {
    this.requests.get(controller)?.();
  }
  destroy() {
    if (this.disposed) return;
    this.disposed = true;
    for (const cleanup of this.cleanups) cleanup();
    this.cleanups.clear();
    this.requests.clear();
  }
}
