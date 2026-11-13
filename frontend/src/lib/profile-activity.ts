import type { NamespaceDay } from "$lib/api/profile.js";

/** A compact preview of the visible commits the activity endpoint returned. */
export function summarizeActivityDay(day: NamespaceDay) {
  const authors = new Map<string, number>();
  let first: number | null = null;
  let last: number | null = null;
  for (const repository of day.repositories) {
    for (const commit of repository.commits) {
      const name = commit.author_name.trim() || "Unknown author";
      authors.set(name, (authors.get(name) ?? 0) + 1);
      first =
        first === null ? commit.timestamp : Math.min(first, commit.timestamp);
      last =
        last === null ? commit.timestamp : Math.max(last, commit.timestamp);
    }
  }
  return {
    repositories: [...day.repositories].sort(
      (a, b) =>
        b.commits.length - a.commits.length || a.name.localeCompare(b.name),
    ),
    authors: [...authors]
      .map(([name, count]) => ({ name, count }))
      .sort((a, b) => b.count - a.count || a.name.localeCompare(b.name)),
    first,
    last,
  };
}
