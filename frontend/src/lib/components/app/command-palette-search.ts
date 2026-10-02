function isSubsequence(haystack: string, needle: string): boolean {
  let cursor = 0;
  for (const character of haystack) {
    if (character === needle[cursor]) cursor += 1;
    if (cursor === needle.length) return true;
  }
  return false;
}

export type SearchQuery = {
  needle: string;
  terms: string[];
  topics: string[];
};

/**
 * Splits what was typed into free-text terms and `topic:` filters, all
 * lowercased.
 */
export function parseSearchQuery(value: string): SearchQuery {
  const terms: string[] = [];
  const topics: string[] = [];
  for (const token of value.trim().toLowerCase().split(/\s+/)) {
    if (token.startsWith("topic:") && token.length > "topic:".length) {
      topics.push(token.slice("topic:".length));
    } else if (token) {
      terms.push(token);
    }
  }
  return { needle: terms.join(" "), terms, topics };
}

/** The lowercased fields of a repository that a search runs against. */
export type SearchableRepository = {
  normalizedName: string;
  normalizedPath: string;
  normalizedDescription: string;
};

/**
 * Ranks a repository against the terms; 0 means some term matched nothing.
 * Every term must match the name, path or description, where a prefix beats
 * a substring and a substring beats letters merely appearing in order.
 */
export function score(
  repository: SearchableRepository,
  terms: string[],
): number {
  if (terms.length === 0) return 1;
  const fields = [
    [repository.normalizedName, 100],
    [repository.normalizedPath, 80],
    [repository.normalizedDescription, 30],
  ] as const;
  let total = 0;
  for (const term of terms) {
    const best = fields.reduce((current, [field, weight]) => {
      if (field.startsWith(term)) return Math.max(current, weight);
      if (field.includes(term)) return Math.max(current, weight - 20);
      if (isSubsequence(field, term)) return Math.max(current, weight - 45);
      return current;
    }, 0);
    if (!best) return 0;
    total += best;
  }
  if (repository.normalizedPath.includes(terms.join(" "))) total += 20;
  return total;
}
