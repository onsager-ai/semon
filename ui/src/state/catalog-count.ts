import { catalogSessionKey } from './catalog-wire';
import type { CatalogPage, CatalogSession } from './catalog-wire';

/** Evidence about an accepted bounded list page, not a workspace census. */
export interface CatalogCountWitness {
  source: string;
  generation: string;
  filters: string;
  firstPage: boolean;
}

export function catalogCountWitness(
  page: CatalogPage,
  filters: string,
  firstPage: boolean,
): CatalogCountWitness {
  return { source: page.machine, generation: page.generation, filters, firstPage };
}

/** Only a complete, unpaged observation of this exact source/filter scope is a count. */
export function completeCatalogCount({
  page,
  items,
  witness,
  source,
  filters,
  searchRequested,
  updating,
  more,
}: {
  page: CatalogPage | null;
  items: readonly Pick<CatalogSession, 'key' | 'machine'>[];
  witness: CatalogCountWitness | null;
  source: string;
  filters: string;
  searchRequested: boolean;
  updating: boolean;
  more: boolean;
}): boolean {
  if (
    !page ||
    !witness ||
    updating ||
    more ||
    !witness.firstPage ||
    witness.source !== source ||
    page.machine !== source ||
    witness.generation !== page.generation ||
    witness.filters !== filters ||
    page.read_scope !== 'retained_history' ||
    page.completeness.state !== 'complete' ||
    page.next_cursor !== null ||
    (searchRequested && page.search === null) ||
    (page.search !== null && (!page.search.index_complete || page.search.partial)) ||
    page.items.length > 100 ||
    items.length !== page.items.length
  )
    return false;
  const keys = new Set(page.items.map(catalogSessionKey));
  return (
    keys.size === page.items.length &&
    new Set(items.map(catalogSessionKey)).size === items.length &&
    items.every((item) => keys.has(catalogSessionKey(item)))
  );
}
