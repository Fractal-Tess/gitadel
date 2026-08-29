import {
  ApiFailure,
  jsonBody,
  requestJson,
} from "$lib/api/transport.js";
import { invitationSchema, type AuditEvent } from "$lib/api/instance.js";
import {
  loadAdminActivity,
  refreshAdminActivity,
} from "$lib/navigation-cache.js";
import type { AppState } from "$lib/state/app-state.svelte.js";

export class AdminSettingsState {
  auditEvents = $state.raw<AuditEvent[]>([]);
  invitation = $state<string | null>(null);
  invitationHours = $state("72");
  working = $state(false);
  error = $state<string | null>(null);

  constructor(private readonly app: AppState) {}

  async initialize(): Promise<void> {
    const scope = this.app.authorizationScope;
    await this.run(async () => {
      const events = await loadAdminActivity(scope);
      if (this.app.authorizationScope === scope) this.auditEvents = events;
    });
  }

  async createInvitation(): Promise<void> {
    const scope = this.app.authorizationScope;
    await this.run(async () => {
      const response = await requestJson(
        "/api/v1/invitations",
        invitationSchema,
        {
          method: "POST",
          body: jsonBody({ expires_in_hours: Number(this.invitationHours) }),
        },
      );
      this.invitation = response.token;
      const events = await refreshAdminActivity(scope);
      if (this.app.authorizationScope === scope) this.auditEvents = events;
    });
  }

  private async run(task: () => Promise<void>): Promise<void> {
    this.working = true;
    this.error = null;
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
