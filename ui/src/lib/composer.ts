/** Enhances host-owned native composers. Data, persistence and submit remain
 * with the consumer. Native details/textarea are the no-JS fallback. */
export function mountComposers(root: ParentNode = document): { destroy(): void } {
  const composers = [...root.querySelectorAll<HTMLElement>('.sh-composer')];
  const abort = new AbortController();
  for (const composer of composers) {
    const pickers = [...composer.querySelectorAll<HTMLDetailsElement>('.sh-picker')];
    for (const picker of pickers) {
      picker.addEventListener(
        'toggle',
        () => {
          if (!picker.open) return;
          for (const other of pickers) if (other !== picker) other.open = false;
        },
        { signal: abort.signal },
      );
    }
    composer.addEventListener(
      'keydown',
      (event) => {
        if (event.key !== 'Escape') return;
        const open = pickers.find((picker) => picker.open);
        if (!open) return;
        event.preventDefault();
        event.stopPropagation();
        open.open = false;
        open.querySelector<HTMLElement>('summary')?.focus();
      },
      { signal: abort.signal },
    );
  }
  return {
    destroy() {
      abort.abort();
      for (const composer of composers)
        for (const picker of composer.querySelectorAll<HTMLDetailsElement>('.sh-picker'))
          picker.open = false;
    },
  };
}
