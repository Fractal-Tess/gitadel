/** How many audit events one request asks for. */
export const AUDIT_PAGE_SIZE = 100;

type Identified = { readonly id: string | number };

export type AuditPage<T> = { events: T[]; hasMore: boolean };

/** The state after the first page: a full page means older events may exist. */
export function firstAuditPage<T extends Identified>(
  events: T[],
): AuditPage<T> {
  return { events, hasMore: events.length === AUDIT_PAGE_SIZE };
}

/**
 * Adds the page fetched at `offset = loaded.length` below the loaded events.
 *
 * A server that does not know `offset` answers with the newest events again;
 * that shows as a page starting with the event already at the top, and ends
 * the list instead of repeating it.
 */
export function appendAuditPage<T extends Identified>(
  loaded: readonly T[],
  page: readonly T[],
): AuditPage<T> {
  if (page.length === 0 || page[0]?.id === loaded[0]?.id) {
    return { events: [...loaded], hasMore: false };
  }
  // Events recorded since the first page push older ones onto the next page.
  const known = new Set(loaded.map((event) => event.id));
  const added = page.filter((event) => !known.has(event.id));
  return {
    events: [...loaded, ...added],
    hasMore: added.length > 0 && page.length === AUDIT_PAGE_SIZE,
  };
}
