import { toast } from "svelte-sonner";

import {
  createdOauthApplicationSchema,
  type OauthApplication,
} from "$lib/api/account.js";
import {
  ApiFailure,
  jsonBody,
  requestEmpty,
  requestJson,
} from "$lib/api/transport.js";
import type { AuthorizationCacheScope } from "$lib/cache-scope.js";

type ScopeGuard = (scope: AuthorizationCacheScope) => boolean;
type Changed = (applications: OauthApplication[]) => void;

export class OAuthSettingsState {
  oauthApplications = $state.raw<OauthApplication[]>([]);
  oauthApplicationName = $state("");
  oauthRedirectUri = $state("");
  createdOauthClientId = $state<string | null>(null);
  createdOauthClientSecret = $state<string | null>(null);
  working = $state(false);
  private scope: AuthorizationCacheScope;
  constructor(
    initial: OauthApplication[],
    scope: AuthorizationCacheScope,
    private readonly current: ScopeGuard,
    private readonly changed: Changed,
  ) {
    this.scope = scope;
    this.oauthApplications = initial;
  }

  setScope(scope: AuthorizationCacheScope): void {
    this.scope = scope;
  }

  async createOauthApplication(): Promise<boolean> {
    const scope = this.scope;
    return this.run(scope, async () => {
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
      if (!this.current(scope)) return;
      this.oauthApplications = [
        ...this.oauthApplications,
        response.application,
      ];
      this.createdOauthClientId = response.application.client_id;
      this.createdOauthClientSecret = response.client_secret;
      this.oauthApplicationName = "";
      this.oauthRedirectUri = "";
      this.changed(this.oauthApplications);
      toast.success("OAuth application created", {
        description: "Save the client secret now.",
      });
    });
  }

  async deleteOauthApplication(id: string): Promise<boolean> {
    const scope = this.scope;
    return this.run(scope, async () => {
      await requestEmpty(`/api/v1/me/oauth-applications/${id}`, {
        method: "DELETE",
      });
      if (!this.current(scope)) return;
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
      this.changed(this.oauthApplications);
      toast.success("OAuth application revoked");
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
