import type { AuthorizationCacheScope } from "$lib/cache-scope.js";

export interface RepositoryLocator {
  readonly namespace: string;
  readonly name: string;
}

export interface RepositoryFeatureContext {
  readonly locator: RepositoryLocator;
  readonly scope: AuthorizationCacheScope;
  readonly isScopeCurrent: () => boolean;
}

export interface ErrorCallbacks {
  clearError(): void;
  setError(message: string): void;
}

export function repositoryApi(locator: RepositoryLocator): `/api/v1/repositories/${string}/${string}`;
export function repositoryApi<const P extends string>(locator: RepositoryLocator, path: P): `/api/v1/repositories/${string}/${string}${P}`;
export function repositoryApi(locator: RepositoryLocator, path = "") {
  return `/api/v1/repositories/${encodeURIComponent(locator.namespace)}/${encodeURIComponent(locator.name)}${path}` as const;
}

export function repositoryActionsApi(locator: RepositoryLocator): `/api/v1/repositories/${string}/${string}/actions`;
export function repositoryActionsApi<const P extends string>(locator: RepositoryLocator, path: P): `/api/v1/repositories/${string}/${string}/actions${P}`;
export function repositoryActionsApi(
  locator: RepositoryLocator,
  path = "",
): string {
  return repositoryApi(locator, `/actions${path}`);
}

export function repositoryHooksApi(locator: RepositoryLocator): `/api/v1/repos/${string}/${string}/hooks` {
  return `/api/v1/repos/${encodeURIComponent(locator.namespace)}/${encodeURIComponent(locator.name)}/hooks` as const;
}
