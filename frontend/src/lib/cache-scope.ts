export type AuthorizationCacheScope = Readonly<{
  viewer: string | null;
  epoch: number;
}>;

export function authorizationCacheScopeKey(
  scope: AuthorizationCacheScope,
): string {
  return `${scope.viewer ?? "anonymous"}\0${scope.epoch}`;
}
