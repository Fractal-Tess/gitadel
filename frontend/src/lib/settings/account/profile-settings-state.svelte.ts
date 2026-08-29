import { toast } from "svelte-sonner";

import { authResponseSchema } from "$lib/api/auth.js";
import {
  ApiFailure,
  jsonBody,
  requestEmpty,
  requestJson,
} from "$lib/api/transport.js";
import type { AuthorizationCacheScope } from "$lib/cache-scope.js";
import type { AppState } from "$lib/state/app-state.svelte.js";

type ScopeGuard = (scope: AuthorizationCacheScope) => boolean;

export class ProfileSettingsState {
  username = $state("");
  defaultRepositoryVisibility = $state<"public" | "private">("private");
  working = $state(false);
  error = $state<string | null>(null);

  private scope: AuthorizationCacheScope;
  constructor(
    private readonly app: AppState,
    scope: AuthorizationCacheScope,
    private readonly current: ScopeGuard,
    private readonly scopeChanged: () => void = () => {},
  ) {
    this.scope = scope;
    this.username = app.authStatus?.user?.username ?? "";
    this.defaultRepositoryVisibility =
      app.authStatus?.user?.default_repository_visibility ?? "private";
  }

  setScope(scope: AuthorizationCacheScope): void {
    this.scope = scope;
  }

  async updateUsername(): Promise<void> {
    const scope = this.scope;
    const username = this.username.trim().toLowerCase();
    this.username = username;
    const previousUsername = this.app.authStatus?.user?.username;
    if (!username || username === previousUsername) return;
    await this.run(scope, async () => {
      const response = await requestJson(
        "/api/v1/me/username",
        authResponseSchema,
        {
          method: "PUT",
          body: jsonBody({ username }),
        },
      );
      if (!this.current(scope)) return;
      this.username = response.user.username;
      await this.app.refreshAuth();
      this.scopeChanged();
      toast.success("Username updated", {
        description:
          "Update remotes that use your previous repository namespace.",
      });
    });
  }

  async updateRepositoryVisibility(
    visibility: "public" | "private",
  ): Promise<void> {
    const scope = this.scope;
    const previous = this.defaultRepositoryVisibility;
    if (visibility === previous) return;
    this.defaultRepositoryVisibility = visibility;
    await this.run(scope, async () => {
      try {
        const response = await requestJson(
          "/api/v1/me/repository-preferences",
          authResponseSchema,
          {
            method: "PUT",
            body: jsonBody({ default_repository_visibility: visibility }),
          },
        );
        if (!this.current(scope)) return;
        this.defaultRepositoryVisibility =
          response.user.default_repository_visibility;
        await this.app.refreshAuth();
        this.scopeChanged();
        toast.success("Repository default updated");
      } catch (error) {
        if (this.current(scope)) this.defaultRepositoryVisibility = previous;
        throw error;
      }
    });
  }

  private async run(
    scope: AuthorizationCacheScope,
    task: () => Promise<void>,
  ): Promise<void> {
    this.working = true;
    this.error = null;
    try {
      await task();
    } catch (caught) {
      if (!this.current(scope)) return;
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

export class PasswordSettingsState {
  currentPassword = $state("");
  newPassword = $state("");
  confirmPassword = $state("");
  working = $state(false);
  error = $state<string | null>(null);

  private scope: AuthorizationCacheScope;
  constructor(
    scope: AuthorizationCacheScope,
    private readonly current: ScopeGuard,
  ) {
    this.scope = scope;
  }

  setScope(scope: AuthorizationCacheScope): void {
    this.scope = scope;
  }
  async updatePassword(): Promise<void> {
    if (this.newPassword !== this.confirmPassword) {
      this.error = "The new passwords do not match.";
      toast.error(this.error);
      return;
    }
    const scope = this.scope;
    await this.run(scope, async () => {
      await requestEmpty("/api/v1/me/password", {
        method: "PUT",
        body: jsonBody({
          current_password: this.currentPassword,
          new_password: this.newPassword,
        }),
      });
      if (!this.current(scope)) return;
      this.currentPassword = "";
      this.newPassword = "";
      this.confirmPassword = "";
      toast.success("Password updated", {
        description: "Other browser sessions were signed out.",
      });
    });
  }

  private async run(
    scope: AuthorizationCacheScope,
    task: () => Promise<void>,
  ): Promise<void> {
    this.working = true;
    this.error = null;
    try {
      await task();
    } catch (caught) {
      if (!this.current(scope)) return;
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
