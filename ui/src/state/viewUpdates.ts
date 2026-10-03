export type ViewChange =
  | { readonly kind: 'model'; readonly revision: number }
  | { readonly kind: 'transcript'; readonly revision: number; readonly session: string };
/** Accepted synchronous transactions publish once, after their complete state is visible. */
export class ViewUpdates {
  private listeners = new Set<(change: ViewChange) => void>();
  private disposed = false;
  private sequence = 0;
  private modelSequence = 0;
  get revision() {
    return this.sequence;
  }
  get modelRevision() {
    return this.modelSequence;
  }
  subscribe(listener: (change: ViewChange) => void, session?: string): () => void {
    if (this.disposed) return () => {};
    const selected = (change: ViewChange) => {
      if (session === undefined || change.kind === 'model' || change.session === session)
        listener(change);
    };
    this.listeners.add(selected);
    return () => {
      this.listeners.delete(selected);
    };
  }
  model() {
    if (this.disposed) return;
    this.modelSequence++;
    this.publish({ kind: 'model', revision: ++this.sequence });
  }
  transcript(session: string) {
    if (!this.disposed) this.publish({ kind: 'transcript', session, revision: ++this.sequence });
  }
  private publish(change: ViewChange) {
    for (const listener of this.listeners) listener(change);
  }
  destroy() {
    this.disposed = true;
    this.listeners.clear();
  }
}
