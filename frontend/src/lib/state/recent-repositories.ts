const STORAGE_KEY = "gitadel:recent-repositories";
const RECENT_LIMIT = 6;

/**
 * The repositories the viewer actually opened, most recent first, as
 * `namespace/name` paths. The command palette leads with these because a
 * server-ordered list of everything is rarely what someone is reaching for.
 */
export function recentRepositoryPaths(): string[] {
  const stored = globalThis.localStorage?.getItem(STORAGE_KEY);
  if (!stored) return [];
  try {
    const parsed: unknown = JSON.parse(stored);
    if (!Array.isArray(parsed)) return [];
    return parsed
      .filter((entry): entry is string => typeof entry === "string")
      .slice(0, RECENT_LIMIT);
  } catch {
    return [];
  }
}

export function recordRepositoryVisit(namespace: string, name: string): void {
  const path = `${namespace}/${name}`;
  const next = [
    path,
    ...recentRepositoryPaths().filter((entry) => entry !== path),
  ].slice(0, RECENT_LIMIT);
  globalThis.localStorage?.setItem(STORAGE_KEY, JSON.stringify(next));
}
