import {
  getLocalTimeZone,
  today,
  type CalendarDate,
} from "@internationalized/date";
import { goto } from "$app/navigation";
import { resolve } from "$app/paths";
import { toast } from "svelte-sonner";
import { z } from "zod";

import {
  ApiFailure,
  authResponseSchema,
  createdOauthApplicationSchema,
  createdTokenSchema,
  jsonBody,
  memberSchema,
  organizationSchema,
  passkeySchema,
  requestEmpty,
  requestJson,
  sshKeySchema,
  webauthnCreationSchema,
  type ApiToken,
  type Member,
  type OauthApplication,
  type Organization,
  type PasskeySummary,
  type SshKey,
} from "$lib/api.js";
import {
  clearAccountSettings,
  loadAccountSettings,
  peekAccountSettings,
  updateAccountSettings,
  type AccountSettingsData,
} from "$lib/navigation-cache.js";
import type { AppState } from "$lib/state/app-state.svelte.js";
import { createCredential, creationOptions } from "$lib/webauthn.js";

export type AccountSettingsView = "security" | "applications" | "organizations";

export class AccountSettingsState {
  view = $state<AccountSettingsView>("security");
  passkeys = $state.raw<PasskeySummary[]>([]);
  sshKeys = $state.raw<SshKey[]>([]);
  tokens = $state.raw<ApiToken[]>([]);
  oauthApplications = $state.raw<OauthApplication[]>([]);
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
  tokenExpiresOn = $state<CalendarDate | undefined>();
  createdToken = $state<string | null>(null);
  oauthApplicationName = $state("");
  oauthRedirectUri = $state("");
  createdOauthClientId = $state<string | null>(null);
  createdOauthClientSecret = $state<string | null>(null);
  organizationSlug = $state("");
  organizationDisplayName = $state("");
  memberUsername = $state("");
  memberRole = $state<"owner" | "member">("member");
  username = $state("");
  usernamePassword = $state("");
  currentPassword = $state("");
  newPassword = $state("");
  confirmPassword = $state("");
  working = $state(false);
  loading = $state(true);
  error = $state<string | null>(null);

  #cacheUsername: string;

  constructor(private readonly app: AppState) {
    this.#cacheUsername = app.authStatus?.user?.username ?? "";
    this.username = this.#cacheUsername;
    const cached = this.#cacheUsername
      ? peekAccountSettings(this.#cacheUsername)
      : null;
    if (cached) {
      this.#applySettings(cached);
      this.loading = false;
    }
  }

  async initialize(): Promise<void> {
    const username = this.app.authStatus?.user?.username ?? "";
    if (!username) {
      this.loading = false;
      return;
    }

    this.loading = !peekAccountSettings(username);
    this.username = username;
    await this.run(async () => {
      this.#applySettings(await loadAccountSettings(username));
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

  async updateUsername() {
    await this.run(async () => {
      const previousUsername = this.app.authStatus?.user?.username;
      const response = await requestJson(
        "/api/v1/me/username",
        authResponseSchema,
        {
          method: "PUT",
          body: jsonBody({
            username: this.username,
            current_password: this.usernamePassword,
          }),
        },
      );
      this.username = response.user.username;
      this.usernamePassword = "";
      await this.app.refreshAuth();
      if (previousUsername === response.user.username) {
        toast.info("Username unchanged");
      } else {
        toast.success("Username updated", {
          description:
            "Update remotes that use your previous repository namespace.",
        });
      }
    });
  }

  async updatePassword() {
    if (this.newPassword !== this.confirmPassword) {
      this.error = "The new passwords do not match.";
      toast.error(this.error);
      return;
    }
    await this.run(async () => {
      await requestEmpty("/api/v1/me/password", {
        method: "PUT",
        body: jsonBody({
          current_password: this.currentPassword,
          new_password: this.newPassword,
        }),
      });
      this.currentPassword = "";
      this.newPassword = "";
      this.confirmPassword = "";
      toast.success("Password updated", {
        description: "Other browser sessions were signed out.",
      });
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
      toast.success("Passkey added");
    });
  }

  async removePasskey(id: string): Promise<void> {
    await this.run(async () => {
      await requestEmpty(`/api/v1/me/passkeys/${id}`, { method: "DELETE" });
      this.passkeys = this.passkeys.filter((passkey) => passkey.id !== id);
      toast.success("Passkey removed");
    });
  }

  async addSshKey(): Promise<void> {
    await this.run(async () => {
      const key = await requestJson("/api/v1/me/ssh-keys", sshKeySchema, {
        method: "POST",
        body: jsonBody({
          name: this.sshKeyName,
          public_key: this.sshPublicKey,
        }),
      });
      this.sshKeys = [...this.sshKeys, key];
      this.sshKeyName = "";
      this.sshPublicKey = "";
      toast.success("SSH key added");
    });
  }

  async removeSshKey(id: string): Promise<void> {
    await this.run(async () => {
      await requestEmpty(`/api/v1/me/ssh-keys/${id}`, { method: "DELETE" });
      this.sshKeys = this.sshKeys.filter((key) => key.id !== id);
      toast.success("SSH key removed");
    });
  }

  async createApiToken(): Promise<void> {
    await this.run(async () => {
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
      this.tokens = [...this.tokens, response.details];
      this.createdToken = response.token;
      this.tokenName = "";
      this.tokenExpiresOn = undefined;
      this.tokenRead = true;
      this.tokenWrite = false;
      this.tokenSshKeys = false;
      toast.success("API token created");
    });
  }

  async revokeToken(id: string): Promise<void> {
    await this.run(async () => {
      await requestEmpty(`/api/v1/me/tokens/${id}`, { method: "DELETE" });
      this.tokens = this.tokens.filter((token) => token.id !== id);
      toast.success("API token revoked");
    });
  }

  async createOauthApplication() {
    await this.run(async () => {
      const response = await requestJson(
        "/api/v1/me/oauth-applications",
        createdOauthApplicationSchema,
        {
          method: "POST",
          body: jsonBody({
            name: this.oauthApplicationName,
            redirect_uri: this.oauthRedirectUri,
          }),
        },
      );
      this.oauthApplications = [
        ...this.oauthApplications,
        response.application,
      ];
      this.createdOauthClientId = response.application.client_id;
      this.createdOauthClientSecret = response.client_secret;
      this.oauthApplicationName = "";
      this.oauthRedirectUri = "";
      toast.success("OAuth application created", {
        description: "Save the client secret now.",
      });
    });
  }

  async deleteOauthApplication(id: string) {
    await this.run(async () => {
      await requestEmpty(`/api/v1/me/oauth-applications/${id}`, {
        method: "DELETE",
      });
      this.oauthApplications = this.oauthApplications.filter(
        (application) => application.id !== id,
      );
      if (
        this.createdOauthClientId &&
        !this.oauthApplications.some(
          (application) => application.client_id === this.createdOauthClientId,
        )
      ) {
        this.createdOauthClientId = null;
        this.createdOauthClientSecret = null;
      }
      toast.success("OAuth application revoked");
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
      toast.success("Organization created");
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
      toast.success("Organization member added");
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
      toast.success("Organization member removed");
    });
  }

  #applySettings(settings: AccountSettingsData): void {
    this.passkeys = settings.passkeys;
    this.sshKeys = settings.sshKeys;
    this.tokens = settings.tokens;
    this.oauthApplications = settings.oauthApplications;
    this.organizations = settings.organizations;
  }

  #cacheSettings(): void {
    const username = this.app.authStatus?.user?.username;
    if (!username) {
      if (this.#cacheUsername) clearAccountSettings(this.#cacheUsername);
      return;
    }
    if (this.#cacheUsername && this.#cacheUsername !== username) {
      clearAccountSettings(this.#cacheUsername);
    }
    this.#cacheUsername = username;
    updateAccountSettings(username, {
      passkeys: this.passkeys,
      sshKeys: this.sshKeys,
      tokens: this.tokens,
      oauthApplications: this.oauthApplications,
      organizations: this.organizations,
    });
  }

  private async run(task: () => Promise<void>): Promise<void> {
    this.working = true;
    this.error = null;
    try {
      await task();
      this.#cacheSettings();
    } catch (caught) {
      this.error =
        caught instanceof ApiFailure || caught instanceof Error
          ? caught.message
          : "The request failed.";
      toast.error(this.error);
    } finally {
      this.working = false;
    }
  }
}
