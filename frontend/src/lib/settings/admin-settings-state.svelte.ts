import { toast } from "svelte-sonner";
import { ApiFailure, jsonBody, requestJson } from "$lib/api/transport.js";
import { invitationSchema, type AuditEvent } from "$lib/api/instance.js";
import {
  loadAdminActivity,
  peekAdminActivity,
  refreshAdminActivity,
  type AdminSettingsView,
} from "$lib/settings/settings-data-cache.js";
import type { AppState } from "$lib/state/app-state.svelte.js";

export class AdminSettingsState {
  auditEvents = $state.raw<AuditEvent[]>([]);
  invitation = $state<string | null>(null);
  invitationHours = $state("72");
  working = $state(false);
  loading = $state(false);
  error = $state<string | null>(null);
  #loadSequence = 0;

  constructor(private readonly app: AppState) {}

  async initialize(view: AdminSettingsView): Promise<void> {
    const sequence = ++this.#loadSequence;
    if (view !== "activity") {
      this.loading = false;
      this.error = null;
      return;
    }
    const scope = this.app.authorizationScope;
    const cached = peekAdminActivity(scope);
    if (cached) {
      this.auditEvents = cached;
      this.loading = false;
      this.error = null;
      return;
    }
    this.loading = true;
    this.error = null;
    try {
      const events = await loadAdminActivity(scope);
      if (
        sequence !== this.#loadSequence ||
        this.app.authorizationScope !== scope
      )
        return;
      this.auditEvents = events;
    } catch (caught) {
      if (
        sequence === this.#loadSequence &&
        this.app.authorizationScope === scope
      ) {
        this.error =
          caught instanceof ApiFailure || caught instanceof Error
            ? caught.message
            : "The request failed.";
      }
    } finally {
      if (
        sequence === this.#loadSequence &&
        this.app.authorizationScope === scope
      )
        this.loading = false;
    }
  }

  async refreshActivity(): Promise<void> {
    const scope = this.app.authorizationScope;
    await this.run(async () => {
      const events = await refreshAdminActivity(scope);
      if (this.app.authorizationScope === scope) {
        this.auditEvents = events;
        this.error = null;
      }
    }, "toast");
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
      if (this.app.authorizationScope === scope) {
        this.auditEvents = events;
        this.error = null;
      }
    }, "toast");
  }

  private async run(
    task: () => Promise<void>,
    failureTarget: "inline" | "toast",
  ): Promise<void> {
    this.working = true;
    if (failureTarget === "inline") this.error = null;
    try {
      await task();
    } catch (caught) {
      const message =
        caught instanceof ApiFailure || caught instanceof Error
          ? caught.message
          : "The request failed.";
      if (failureTarget === "inline") {
        this.error = message;
      } else {
        toast.error(message);
      }
    } finally {
      this.working = false;
    }
  }
}
