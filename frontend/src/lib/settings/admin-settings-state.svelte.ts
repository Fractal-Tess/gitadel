import {
  ApiFailure,
  invitationSchema,
  jsonBody,
  requestJson,
  type AuditEvent,
} from "$lib/api.js";
import {
  loadAdminActivity,
  refreshAdminActivity,
} from "$lib/navigation-cache.js";

export class AdminSettingsState {
  auditEvents = $state.raw<AuditEvent[]>([]);
  invitation = $state<string | null>(null);
  invitationHours = $state("72");
  working = $state(false);
  error = $state<string | null>(null);

  async initialize(): Promise<void> {
    await this.run(async () => {
      this.auditEvents = await loadAdminActivity();
    });
  }

  async createInvitation(): Promise<void> {
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
      this.auditEvents = await refreshAdminActivity();
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
