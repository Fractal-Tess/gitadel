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

export function repositoryApi(locator: RepositoryLocator, path = ""): string {
  return `/api/v1/repositories/${encodeURIComponent(locator.namespace)}/${encodeURIComponent(locator.name)}${path}`;
}

export function repositoryActionsApi(
  locator: RepositoryLocator,
  path = "",
): string {
  return repositoryApi(locator, `/actions${path}`);
}

export function repositoryHooksApi(locator: RepositoryLocator): string {
  return `/api/v1/repos/${encodeURIComponent(locator.namespace)}/${encodeURIComponent(locator.name)}/hooks`;
}
