/** One selected reader's automatic retry budget. Manual retry starts a new budget. */
export const CATALOG_RECOVERY_WALL_BUDGET_MS = 45_000;

export type CatalogRecoveryProgress = Partial<
  Record<'source' | 'catalog' | 'identity', string>
>;

export class CatalogRecovery {
  private failures = 0;
  private startedAt: number | null = null;
  private terminal = false;
  private progress: CatalogRecoveryProgress = {};

  get stopped(): boolean {
    if (
      !this.terminal &&
      this.startedAt !== null &&
      Date.now() - this.startedAt >= CATALOG_RECOVERY_WALL_BUDGET_MS
    )
      this.terminal = true;
    return this.terminal;
  }

  fail(now = Date.now()): number | null {
    if (this.stopped || this.expired(now)) {
      this.terminal = true;
      return null;
    }
    this.startedAt ??= now;
    ++this.failures;
    if (this.failures > 3) {
      this.terminal = true;
      return null;
    }
    const delay = 1000 * 2 ** (this.failures - 1);
    if (now + delay >= this.startedAt + CATALOG_RECOVERY_WALL_BUDGET_MS) {
      this.terminal = true;
      return null;
    }
    return delay;
  }

  /**
   * Update only dimensions actually returned by this endpoint. A changed
   * published source/catalog generation can reopen an exhausted reader, while
   * repeated metadata polls cannot extend its fixed wall-clock budget.
   */
  observeProgress(progress: CatalogRecoveryProgress, now = Date.now()): boolean {
    let changed = false;
    for (const key of ['source', 'catalog', 'identity'] as const) {
      const next = progress[key];
      if (next === undefined) continue;
      const previous = this.progress[key];
      if (previous !== undefined && previous !== next) changed = true;
      this.progress[key] = next;
    }
    if (!changed) return false;
    if (this.expired(now)) {
      this.terminal = true;
      return false;
    }
    this.failures = 0;
    if (!this.terminal) return false;
    this.terminal = false;
    return true;
  }

  reset(): void {
    this.failures = 0;
    this.startedAt = null;
    this.terminal = false;
  }

  private expired(now: number): boolean {
    return this.startedAt !== null && now - this.startedAt >= CATALOG_RECOVERY_WALL_BUDGET_MS;
  }
}
