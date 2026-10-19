import { createFacetChrome, createSelect, type FacetField } from '../lib';
import type { CatalogFilterDescription } from '../lib/catalog';
import type { CatalogSession } from '../state/catalog-wire';
import { HARNESS } from './registry';
import { shortModel } from '../domain/format';

export interface CatalogFilters {
  machine?: string;
  harness: string;
  repo: string;
  model: string;
  q: string;
}
/** Shared filter presentation over bounded metadata; no transport or read owner. */
export function createCatalogBrowser(host: {
  canOpen(): boolean;
  opened(panel: { destroy(): void }): void;
  closed(panel: { destroy(): void }): void;
  changed(filters: CatalogFilters): void;
  clearFilters(): void;
}) {
  let filters: CatalogFilters = { harness: '', repo: '', model: '', q: '' },
    before = '',
    panel: { destroy(): void } | null = null;
  const options = new Map<string, Set<string>>();
  const owner = createFacetChrome({
    select: (label, value, onChange) => createSelect({ label, value, options: [], onChange }),
    canOpen: host.canOpen,
    change(key, value) {
      filters = { ...filters, [key]: value };
    },
    cleared(key) {
      filters = { ...filters, [key]: '' };
      host.changed(filters);
    },
    clear() {
      filters = { machine: '', harness: '', repo: '', model: '', q: filters.q };
    },
    opened(dialog) {
      before = JSON.stringify(filters);
      panel = { destroy: () => dialog.close() };
      host.opened(panel);
    },
    closed(_dialog, reason) {
      if (panel) host.closed(panel);
      panel = null;
      if (reason === 'dismissed' && before !== JSON.stringify(filters)) host.changed(filters);
    },
  });
  return {
    element: owner.element,
    update(
      values: CatalogFilters,
      rows: readonly CatalogSession[],
      source: string,
      context: CatalogFilterDescription,
      machines: readonly { source_key: string; label: string }[] = [],
      workspace = false,
    ) {
      // Options are observations of the loaded source, never a workspace census.
      for (const key of [...options.keys()])
        if (!key.startsWith(source + '\u0000')) options.delete(key);
      if (!panel) filters = { ...values };
      const fields: FacetField[] = (['repo', 'harness', 'model'] as const).map((key) => {
        const address = source + '\u0000' + key,
          seen = options.get(address) ?? new Set<string>();
        if (key === 'repo') seen.add('__none__');
        for (const row of rows) seen.add(row[key] ?? '__none__');
        if (filters[key]) seen.add(filters[key]);
        // Retain only bounded metadata choices while filters narrow the list.
        while (seen.size > 300) seen.delete(seen.values().next().value!);
        options.set(address, seen);
        const label = (value: string) =>
          key === 'harness'
            ? (HARNESS[value] ?? value)
            : key === 'model'
              ? shortModel(value)
              : value === '__none__'
                ? 'No repo (roles)'
                : value;
        return {
          key,
          label: key === 'repo' ? 'Repo' : key === 'harness' ? 'Harness' : 'Model',
          value: filters[key],
          display: label(filters[key]),
          options: [
            {
              value: '',
              label:
                key === 'repo' ? 'All repos' : key === 'harness' ? 'All harnesses' : 'All models',
            },
            ...[...seen]
              .filter(Boolean)
              .sort()
              .map((value) => ({ value, label: label(value) })),
          ],
        };
      });
      // Hosted machine choices come from the authorized source inventory; the
      // standalone reader keeps its existing neutral facet.
      fields.splice(1, 0, {
        key: 'machine',
        label: 'Machine',
        value: workspace ? (filters.machine ?? '') : '',
        display:
          workspace && filters.machine
            ? (machines.find((m) => m.source_key === filters.machine)?.label ?? filters.machine)
            : 'All machines',
        options: [
          { value: '', label: 'All machines' },
          ...(workspace ? machines.map((m) => ({ value: m.source_key, label: m.label })) : []),
        ],
      });
      const filtered = !!(
        values.machine ||
        values.harness ||
        values.repo ||
        values.model ||
        values.q
      );
      owner.update(fields, {
        tooltip: context.searchQualification,
        action: filtered ? { label: 'Clear filters', run: host.clearFilters } : undefined,
      });
    },
    destroy() {
      owner.destroy();
      options.clear();
    },
  };
}
