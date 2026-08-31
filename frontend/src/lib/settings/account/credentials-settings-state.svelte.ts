import {
  getLocalTimeZone,
  today,
  type CalendarDate,
} from "@internationalized/date";
import { toast } from "svelte-sonner";
import { z } from "zod";

import {
  passkeySchema,
  createdTokenSchema,
  sshKeySchema,
  type ApiToken,
  type PasskeySummary,
  type SshKey,
} from "$lib/api/account.js";
import { webauthnCreationSchema } from "$lib/api/auth.js";
import {
  ApiFailure,
  jsonBody,
  requestEmpty,
  requestJson,
} from "$lib/api/transport.js";
import type { AuthorizationCacheScope } from "$lib/cache-scope.js";
import { createCredential, creationOptions } from "$lib/webauthn.js";

type ScopeGuard = (scope: AuthorizationCacheScope) => boolean;
type CredentialChange =
  | { dataset: "passkeys"; value: PasskeySummary[] }
  | { dataset: "ssh-keys"; value: SshKey[] }
  | { dataset: "api-tokens"; value: ApiToken[] };
type Changed = (change: CredentialChange) => void;

export class CredentialsSettingsState {
  passkeys = $state.raw<PasskeySummary[]>([]);
  sshKeys = $state.raw<SshKey[]>([]);
  tokens = $state.raw<ApiToken[]>([]);
  passkeyName = $state("This device");
  sshKeyName = $state("");
  sshPublicKey = $state("");
  tokenName = $state("");
  tokenRead = $state(true);
  tokenWrite = $state(false);
  tokenSshKeys = $state(false);
  tokenExpiresOn = $state<CalendarDate | undefined>();
  createdToken = $state<string | null>(null);
  working = $state(false);
  private scope: AuthorizationCacheScope;
  constructor(
    initial: {
      passkeys: PasskeySummary[];
      sshKeys: SshKey[];
      tokens: ApiToken[];
    },
    scope: AuthorizationCacheScope,
    private readonly current: ScopeGuard,
    private readonly changed: Changed,
  ) {
    this.scope = scope;
    this.passkeys = initial.passkeys;
    this.sshKeys = initial.sshKeys;
    this.tokens = initial.tokens;
  }

  setScope(scope: AuthorizationCacheScope): void {
    this.scope = scope;
  }

  async addPasskey(): Promise<boolean> {
    const scope = this.scope;
    return this.run(scope, async () => {
      const challenge = await requestJson(
        "/api/v1/me/passkeys/register/start",
        webauthnCreationSchema,
        { method: "POST", body: jsonBody({ name: this.passkeyName }) },
      );
      const credential = await createCredential(
        creationOptions(challenge.options.publicKey),
      );
      await requestEmpty("/api/v1/me/passkeys/register/finish", {
        method: "POST",
        body: jsonBody({ challenge_id: challenge.challenge_id, credential }),
      });
      const passkeys = await requestJson(
        "/api/v1/me/passkeys",
        z.array(passkeySchema),
      );
      if (!this.current(scope)) return;
      this.passkeys = passkeys;
      this.changed({ dataset: "passkeys", value: this.passkeys });
      toast.success("Passkey added");
    });
  }

  async removePasskey(id: string): Promise<boolean> {
    const scope = this.scope;
    return this.run(scope, async () => {
      await requestEmpty(`/api/v1/me/passkeys/${id}`, { method: "DELETE" });
      if (!this.current(scope)) return;
      this.passkeys = this.passkeys.filter((passkey) => passkey.id !== id);
      this.changed({ dataset: "passkeys", value: this.passkeys });
      toast.success("Passkey removed");
    });
  }

  async addSshKey(): Promise<boolean> {
    const scope = this.scope;
    return this.run(scope, async () => {
      const key = await requestJson("/api/v1/me/ssh-keys", sshKeySchema, {
        method: "POST",
        body: jsonBody({
          name: this.sshKeyName,
          public_key: this.sshPublicKey,
        }),
      });
      if (!this.current(scope)) return;
      this.sshKeys = [...this.sshKeys, key];
      this.sshKeyName = "";
      this.sshPublicKey = "";
      this.changed({ dataset: "ssh-keys", value: this.sshKeys });
      toast.success("SSH key added");
    });
  }

  async removeSshKey(id: string): Promise<boolean> {
    const scope = this.scope;
    return this.run(scope, async () => {
      await requestEmpty(`/api/v1/me/ssh-keys/${id}`, { method: "DELETE" });
      if (!this.current(scope)) return;
      this.sshKeys = this.sshKeys.filter((key) => key.id !== id);
      this.changed({ dataset: "ssh-keys", value: this.sshKeys });
      toast.success("SSH key removed");
    });
  }

  async createApiToken(): Promise<boolean> {
    const scope = this.scope;
    return this.run(scope, async () => {
      const scopes = [
        this.tokenRead && "read",
        this.tokenWrite && "write",
        this.tokenSshKeys && "ssh_keys",
      ].filter((scope): scope is string => Boolean(scope));
      const expiresInDays =
        this.tokenExpiresOn?.compare(today(getLocalTimeZone())) ?? null;
      const response = await requestJson(
        "/api/v1/me/tokens",
        createdTokenSchema,
        {
          method: "POST",
          body: jsonBody({
            name: this.tokenName,
            scopes,
            expires_in_days: expiresInDays,
          }),
        },
      );
      if (!this.current(scope)) return;
      this.tokens = [...this.tokens, response.details];
      this.createdToken = response.token;
      this.tokenName = "";
      this.tokenExpiresOn = undefined;
      this.tokenRead = true;
      this.tokenWrite = false;
      this.tokenSshKeys = false;
      this.changed({ dataset: "api-tokens", value: this.tokens });
      toast.success("API token created");
    });
  }

  async revokeToken(id: string): Promise<boolean> {
    const scope = this.scope;
    return this.run(scope, async () => {
      await requestEmpty(`/api/v1/me/tokens/${id}`, { method: "DELETE" });
      if (!this.current(scope)) return;
      this.tokens = this.tokens.filter((token) => token.id !== id);
      this.changed({ dataset: "api-tokens", value: this.tokens });
      toast.success("API token revoked");
    });
  }

  private async run(
    scope: AuthorizationCacheScope,
    task: () => Promise<void>,
  ): Promise<boolean> {
    this.working = true;
    try {
      await task();
      return true;
    } catch (caught) {
      if (!this.current(scope)) return false;
      const message =
        caught instanceof ApiFailure || caught instanceof Error
          ? caught.message
          : "The request failed.";
      toast.error(message);
      return false;
    } finally {
      this.working = false;
    }
  }
}
