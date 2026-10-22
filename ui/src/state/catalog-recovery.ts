/** One selected reader's automatic retry budget. Manual retry starts a new budget. */
export class CatalogRecovery {
  private failures = 0;
  get stopped(): boolean {
    return this.failures > 3;
  }
  fail(): number | null {
    ++this.failures;
    return this.stopped ? null : 1000 * 2 ** (this.failures - 1);
  }
  reset(): void {
    this.failures = 0;
  }
}
