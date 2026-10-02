/** The page size requested from list endpoints, and the most they return. */
export const LIST_PAGE_SIZE = 100;

type Identified = { readonly id: string | number };

export type LoadedPages<T> = {
  items: T[];
  /** How many pages the items span, which a later reload fetches again. */
  pages: number;
  hasMore: boolean;
};

/**
 * Whether another page may follow. Only an exactly full page says so: a
 * server that does not paginate answers with everything it has, and anything
 * longer than a page is therefore the complete list.
 */
export function isFullPage(page: readonly unknown[]): boolean {
  return page.length === LIST_PAGE_SIZE;
}

/**
 * Adds a page to the items already loaded, skipping ones that are present.
 * A page that contributes nothing ends the list: it is what a server that
 * ignores `page` sends when asked for the second one.
 */
export function appendPage<T extends Identified>(
  loaded: LoadedPages<T>,
  page: readonly T[],
): LoadedPages<T> {
  const known = new Set(loaded.items.map((item) => item.id));
  const added = page.filter((item) => !known.has(item.id));
  if (added.length === 0) return { ...loaded, hasMore: false };
  return {
    items: [...loaded.items, ...added],
    pages: loaded.pages + 1,
    hasMore: isFullPage(page),
  };
}

/**
 * Loads the first `pages` pages in order, stopping early once the list ends.
 */
export async function loadPages<T extends Identified>(
  load: (page: number) => Promise<T[]>,
  pages = 1,
): Promise<LoadedPages<T>> {
  let loaded: LoadedPages<T> = { items: [], pages: 0, hasMore: true };
  while (loaded.hasMore && loaded.pages < Math.max(1, pages)) {
    loaded = appendPage(loaded, await load(loaded.pages + 1));
  }
  return loaded;
}
