import { toast } from "svelte-sonner";
import { errorMessage, jsonBody, requestJson } from "$lib/api/transport.js";
import { invitationSchema, type AuditEvent } from "$lib/api/instance.js";
import {
  appendAuditPage,
  firstAuditPage,
} from "$lib/settings/audit-pagination.js";
import {
  loadAdminActivity,
  loadAdminActivityPage,
  peekAdminActivity,
  refreshAdminActivity,
  type AdminSettingsView,
} from "$lib/settings/settings-data-cache.js";
import type { AppState } from "$lib/state/app-state.svelte.js";

export class AdminSettingsState {
  auditEvents = $state.raw<AuditEvent[]>([]);
  /** Whether older events than the loaded ones may exist. */
  auditHasMore = $state(false);
  auditLoadingMore = $state(false);
  invitation = $state<string | null>(null);
  invitationHours = $state("72");
  working = $state(false);
  loading = $state(false);
  error = $state<string | null>(null);
  #loadSequence = 0;
  // Advanced whenever the first page is replaced, so an older page that was
  // requested before the replacement is not appended to the new list.
  #activityGeneration = 0;

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
      this.#showFirstPage(cached);
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
      this.#showFirstPage(events);
    } catch (caught) {
      if (
        sequence === this.#loadSequence &&
        this.app.authorizationScope === scope
      ) {
        this.error = errorMessage(caught);
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
        this.#showFirstPage(events);
        this.error = null;
      }
    }, "toast");
  }

  async loadMoreActivity(): Promise<void> {
    if (this.auditLoadingMore || !this.auditHasMore) return;
    const scope = this.app.authorizationScope;
    const generation = this.#activityGeneration;
    this.auditLoadingMore = true;
    try {
      const page = await loadAdminActivityPage(this.auditEvents.length);
      if (
        this.app.authorizationScope !== scope ||
        generation !== this.#activityGeneration
      )
        return;
      const next = appendAuditPage(this.auditEvents, page);
      this.auditEvents = next.events;
      this.auditHasMore = next.hasMore;
    } catch (caught) {
      toast.error(errorMessage(caught));
    } finally {
      this.auditLoadingMore = false;
    }
  }

  #showFirstPage(events: AuditEvent[]): void {
    const first = firstAuditPage(events);
    this.#activityGeneration += 1;
    this.auditEvents = first.events;
    this.auditHasMore = first.hasMore;
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
        this.#showFirstPage(events);
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
      const message = errorMessage(caught);
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
