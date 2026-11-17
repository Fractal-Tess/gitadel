// Compiled by svelte-check; never executed. Removing a rejection must make this
// fixture fail with an unused @ts-expect-error, rather than silently weakening it.
import { z } from 'zod';
import { authStatusSchema, passwordLoginResponseSchema } from '../src/lib/api/auth.js';
import { historySchema } from '../src/lib/api/repositories.js';
import { issueSchema } from '../src/lib/api/issues.js';
import { backupProvidersSchema, backupProviderCatalogItemSchema } from '../src/lib/api/backups.js';
import { apiQuery, requestJson, requestEmpty, jsonBody } from '../src/lib/api/transport.js';
import { getAuthStatus, getRepositoriesNamespaceNameHistory } from '../src/lib/api/generated/sdk.gen.js';

export async function contractTypingChecks(): Promise<void> {
  await requestJson('/api/v1/auth/status', authStatusSchema);
  await requestJson('/api/v1/auth/login', passwordLoginResponseSchema, {
    method: 'POST', body: jsonBody({ username: 'fixture', password: 'not-a-real-password' }),
  });
  await requestJson('/api/v1/repositories/owner/repo/history', historySchema, { query: apiQuery({ page: 1 }) });
  await requestJson('/api/v1/repositories/owner/repo/issues/1', issueSchema);
  // @ts-expect-error numeric Rust path parameters require numeric URL segments
  await requestJson('/api/v1/repositories/owner/repo/issues/letters', issueSchema);

  // @ts-expect-error an unregistered URL cannot assert a response shape
  await requestJson('/api/v1/not-a-route', authStatusSchema);
  // @ts-expect-error the method must exist on this URL
  await requestJson('/api/v1/auth/status', authStatusSchema, { method: 'POST' });
  // @ts-expect-error required request fields come from the Rust DTO
  await requestJson('/api/v1/auth/login', passwordLoginResponseSchema, { method: 'POST', body: jsonBody({ username: 'fixture' }) });
  const oldCredentials = { username: 'fixture', password: 'fixture', removed_field: true };
  // @ts-expect-error removed body fields also fail when the payload is prepared in a variable
  await requestJson('/api/v1/auth/login', passwordLoginResponseSchema, { method: 'POST', body: jsonBody(oldCredentials) });
  // @ts-expect-error query values use their Rust types
  await requestJson('/api/v1/repositories/owner/repo/history', historySchema, { query: apiQuery({ page: '1' }) });
  // @ts-expect-error query fields must be registered, too
  await requestJson('/api/v1/repositories/owner/repo/history', historySchema, { query: apiQuery({ imaginary: true }) });
  const oldQuery = { page: 1, removed_field: true };
  // @ts-expect-error removed query fields also fail when prepared in a variable
  await requestJson('/api/v1/repositories/owner/repo/history', historySchema, { query: apiQuery(oldQuery) });
  // @ts-expect-error raw query strings cannot bypass typed query options
  await requestJson('/api/v1/repositories/owner/repo/history?page=1', historySchema);
  // @ts-expect-error a required backup key must be supplied
  await requestEmpty('/api/v1/admin/backup/providers/id/backups/object', { method: 'DELETE' });
  // @ts-expect-error a removed response field cannot survive in a manual validator
  await requestJson('/api/v1/', z.object({ api_version: z.string(), application_version: z.string(), obsolete: z.string() }));
  // @ts-expect-error a newly required response field cannot be dropped
  await requestJson('/api/v1/', z.object({ api_version: z.string() }));
  // @ts-expect-error all Rust enum variants must be accepted by the runtime validator
  await requestJson('/api/v1/admin/backup/providers', backupProvidersSchema.extend({ providers: z.array(backupProviderCatalogItemSchema.extend({ slug: z.literal('filesystem') })) }));

  const status = await getAuthStatus({ throwOnError: true });
  // @ts-expect-error generated SDK responses have no invented properties
  status.data.no_such_field;
  // @ts-expect-error the generated SDK also rejects the wrong query type directly
  await getRepositoriesNamespaceNameHistory({ path: { namespace: 'owner', name: 'repo' }, query: { page: '1' } });
}
