import { goto } from "$app/navigation";
import { resolve } from "$app/paths";
import { z } from "zod";

import {
  ApiFailure,
  createdTokenSchema,
  jsonBody,
  memberSchema,
  organizationSchema,
  passkeySchema,
  requestEmpty,
  requestJson,
  sshKeySchema,
  tokenSchema,
  webauthnCreationSchema,
  type ApiToken,
  type Member,
  type Organization,
  type PasskeySummary,
  type SshKey,
} from "$lib/api.js";
import type { AppState } from "$lib/state/app-state.svelte.js";
import {
  createCredential,
  creationOptions,
} from "$lib/webauthn.js";

export type AccountSettingsView = "security" | "organizations";

export class AccountSettingsState {
  view = $state<AccountSettingsView>("security");
  passkeys = $state.raw<PasskeySummary[]>([]);
  sshKeys = $state.raw<SshKey[]>([]);
  tokens = $state.raw<ApiToken[]>([]);
  organizations = $state.raw<Organization[]>([]);
  selectedOrganization = $state.raw<Organization | null>(null);
  members = $state.raw<Member[]>([]);
  passkeyName = $state("This device");
  sshKeyName = $state("");
  sshPublicKey = $state("");
  tokenName = $state("");
  tokenRead = $state(true);
  tokenWrite = $state(false);
  tokenSshKeys = $state(false);
  tokenExpiryDays = $state("");
  createdToken = $state<string | null>(null);
  organizationSlug = $state("");
  organizationDisplayName = $state("");
  memberUsername = $state("");
  memberRole = $state<"owner" | "member">("member");
  working = $state(false);
  loading = $state(true);
  error = $state<string | null>(null);
  notice = $state<string | null>(null);

  constructor(private readonly app: AppState) {}

  async initialize(): Promise<void> {
    this.loading = true;
    await this.run(async () => {
      const [passkeys, sshKeys, tokens, organizations] = await Promise.all([
        requestJson("/api/v1/me/passkeys", z.array(passkeySchema)),
        requestJson("/api/v1/me/ssh-keys", z.array(sshKeySchema)),
        requestJson("/api/v1/me/tokens", z.array(tokenSchema)),
        requestJson("/api/v1/organizations", z.array(organizationSchema)),
      ]);
      this.passkeys = passkeys;
      this.sshKeys = sshKeys;
      this.tokens = tokens;
      this.organizations = organizations;
    });
    this.loading = false;
  }

  async logout(): Promise<void> {
    await this.run(async () => {
      await requestEmpty("/api/v1/auth/logout", { method: "POST" });
      await this.app.refreshAuth();
      await goto(resolve("/login"));
    });
  }

  async addPasskey(): Promise<void> {
    await this.run(async () => {
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
      this.passkeys = await requestJson(
        "/api/v1/me/passkeys",
        z.array(passkeySchema),
      );
      this.notice = "Passkey added.";
    });
  }

  async removePasskey(id: string): Promise<void> {
    await this.run(async () => {
      await requestEmpty(`/api/v1/me/passkeys/${id}`, { method: "DELETE" });
      this.passkeys = this.passkeys.filter((passkey) => passkey.id !== id);
    });
  }

  async addSshKey(): Promise<void> {
    await this.run(async () => {
      const key = await requestJson("/api/v1/me/ssh-keys", sshKeySchema, {
        method: "POST",
        body: jsonBody({ name: this.sshKeyName, public_key: this.sshPublicKey }),
      });
      this.sshKeys = [...this.sshKeys, key];
      this.sshKeyName = "";
      this.sshPublicKey = "";
      this.notice = "SSH key added.";
    });
  }

  async removeSshKey(id: string): Promise<void> {
    await this.run(async () => {
      await requestEmpty(`/api/v1/me/ssh-keys/${id}`, { method: "DELETE" });
      this.sshKeys = this.sshKeys.filter((key) => key.id !== id);
    });
  }

  async createApiToken(): Promise<void> {
    await this.run(async () => {
      const scopes = [
        this.tokenRead && "read",
        this.tokenWrite && "write",
        this.tokenSshKeys && "ssh_keys",
      ].filter((scope): scope is string => Boolean(scope));
      const response = await requestJson(
        "/api/v1/me/tokens",
        createdTokenSchema,
        {
          method: "POST",
          body: jsonBody({
            name: this.tokenName,
            scopes,
            expires_in_days: this.tokenExpiryDays
              ? Number(this.tokenExpiryDays)
              : null,
          }),
        },
      );
      this.tokens = [...this.tokens, response.details];
      this.createdToken = response.token;
      this.tokenName = "";
    });
  }

  async revokeToken(id: string): Promise<void> {
    await this.run(async () => {
      await requestEmpty(`/api/v1/me/tokens/${id}`, { method: "DELETE" });
      this.tokens = this.tokens.filter((token) => token.id !== id);
    });
  }

  async createOrganization(): Promise<void> {
    await this.run(async () => {
      const organization = await requestJson(
        "/api/v1/organizations",
        organizationSchema,
        {
          method: "POST",
          body: jsonBody({
            slug: this.organizationSlug,
            display_name: this.organizationDisplayName,
          }),
        },
      );
      this.organizations = [...this.organizations, organization];
      this.organizationSlug = "";
      this.organizationDisplayName = "";
      await this.selectOrganization(organization);
    });
  }

  async selectOrganization(organization: Organization): Promise<void> {
    this.selectedOrganization = organization;
    this.members = await requestJson(
      `/api/v1/organizations/${organization.slug}/members`,
      z.array(memberSchema),
    );
  }

  async addMember(): Promise<void> {
    if (!this.selectedOrganization) return;
    await this.run(async () => {
      const member = await requestJson(
        `/api/v1/organizations/${this.selectedOrganization!.slug}/members`,
        memberSchema,
        {
          method: "POST",
          body: jsonBody({
            username: this.memberUsername,
            role: this.memberRole,
          }),
        },
      );
      this.members = [...this.members, member];
      this.memberUsername = "";
    });
  }

  async removeMember(username: string): Promise<void> {
    if (!this.selectedOrganization) return;
    await this.run(async () => {
      await requestEmpty(
        `/api/v1/organizations/${this.selectedOrganization!.slug}/members/${username}`,
        { method: "DELETE" },
      );
      this.members = this.members.filter(
        (member) => member.username !== username,
      );
    });
  }

  private async run(task: () => Promise<void>): Promise<void> {
    this.working = true;
    this.error = null;
    this.notice = null;
    try {
      await task();
    } catch (caught) {
      this.error =
        caught instanceof ApiFailure || caught instanceof Error
          ? caught.message
          : "The request failed.";
    } finally {
      this.working = false;
    }
  }
}
