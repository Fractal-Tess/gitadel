import { z } from "zod";

import {
  backupProvidersSchema,
  backupSnapshotSchema,
  type BackupProvider,
  type BackupProviderCatalogItem,
  type BackupSnapshot,
} from "$lib/api/backups.js";
import { requestJson } from "$lib/api/transport.js";
import {
  authorizationCacheScopeKey,
  type AuthorizationCacheScope,
} from "$lib/cache-scope.js";

const cacheLifetime = 5 * 60_000;

type BackupSettingsData = {
  providerCatalog: BackupProviderCatalogItem[];
  providers: BackupProvider[];
  selectedProviderId: string | null;
  snapshots: BackupSnapshot[];
};
type CacheEntry = { expiresAt: number; promise: Promise<BackupSettingsData> };
const cache = new Map<string, CacheEntry>();

function providerPath(id: string): string {
  return `/api/v1/admin/backup/providers/${encodeURIComponent(id)}`;
}
async function fetchBackupSettings(): Promise<BackupSettingsData> {
  const loaded = await requestJson("/api/v1/admin/backup/providers", backupProvidersSchema);
  const selectedProviderId = loaded.connections[0]?.id ?? null;
  const snapshots = selectedProviderId
    ? await requestJson(`${providerPath(selectedProviderId)}/backups`, z.array(backupSnapshotSchema))
    : [];
  return { providerCatalog: loaded.providers, providers: loaded.connections, selectedProviderId, snapshots };
}

export function loadBackupSettings(scope: AuthorizationCacheScope, refresh = false): Promise<BackupSettingsData> {
  const key = authorizationCacheScopeKey(scope);
  const existing = cache.get(key);
  if (!refresh && existing && existing.expiresAt > Date.now()) return existing.promise;
  const entry: CacheEntry = { expiresAt: Date.now() + cacheLifetime, promise: fetchBackupSettings() };
  entry.promise.catch(() => { if (cache.get(key) === entry) cache.delete(key); });
  cache.set(key, entry);
  return entry.promise;
}
export function preloadBackupSettings(scope: AuthorizationCacheScope): void {
  void loadBackupSettings(scope).catch(() => undefined);
}
export function clearBackupSettingsCache(): void {
  cache.clear();
}
