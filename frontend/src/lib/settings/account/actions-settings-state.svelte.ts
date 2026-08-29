import { toast } from "svelte-sonner";

import {
  actionRegistrationSchema,
  type ActionRegistration,
  type ActionRunner,
} from "$lib/api/actions.js";
import {
  ApiFailure,
  jsonBody,
  requestEmpty,
  requestJson,
} from "$lib/api/transport.js";
import type { AuthorizationCacheScope } from "$lib/cache-scope.js";
import { takeNamespaceRunners } from "$lib/namespace-preload.js";

type ScopeGuard = (scope: AuthorizationCacheScope) => boolean;

export class ActionsSettingsState {
  actionRunners = $state.raw<Record<string, ActionRunner[]>>({});
  actionRegistration = $state.raw<ActionRegistration | null>(null);
  actionsLoading = $state(false);
  actionsLoadError = $state<string | null>(null);
  actionsWorking = $state<Record<string, boolean>>({});
  actionErrors = $state<Record<string, string | null>>({});
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
  async loadActionRunners(namespaces: string[]): Promise<void> {
    const scope = this.scope;
    this.actionsLoading = true;
    this.actionsLoadError = null;
    const errors: Record<string, string | null> = {};
    const loaded: Record<string, ActionRunner[]> = {};
    let failures = 0;
    await Promise.all(
      namespaces.map(async (namespace) => {
        try {
          loaded[namespace] = await takeNamespaceRunners(namespace, scope);
          errors[namespace] = null;
        } catch (caught) {
          failures += 1;
          loaded[namespace] = this.actionRunners[namespace] ?? [];
          errors[namespace] =
            caught instanceof ApiFailure || caught instanceof Error
              ? caught.message
              : `Could not load runners for ${namespace}.`;
        }
      }),
    );
    if (!this.current(scope)) return;
    this.actionRunners = loaded;
    this.actionErrors = errors;
    if (failures === namespaces.length && namespaces.length > 0)
      this.actionsLoadError = "Could not load runners.";
    this.actionsLoading = false;
  }

  async issueActionRunner(
    namespace: string,
    name: string,
    labels: string[],
  ): Promise<void> {
    const scope = this.scope;
    if (this.actionsWorking[namespace]) return;
    this.actionsWorking[namespace] = true;
    this.actionErrors[namespace] = null;
    try {
      const registration = await requestJson(
        `/api/v1/namespaces/${encodeURIComponent(namespace)}/actions/runner-registration-tokens`,
        actionRegistrationSchema,
        { method: "POST", body: jsonBody({ name, labels }) },
      );
      if (!this.current(scope)) return;
      this.actionRegistration = registration;
    } catch (caught) {
      if (!this.current(scope)) return;
      const message =
        caught instanceof ApiFailure || caught instanceof Error
          ? caught.message
          : "Could not create the runner registration token.";
      this.actionErrors[namespace] = message;
      toast.error(message);
    } finally {
      this.actionsWorking[namespace] = false;
    }
  }

  async removeActionRunner(namespace: string, runnerId: number): Promise<void> {
    const scope = this.scope;
    if (this.actionsWorking[namespace]) return;
    this.actionsWorking[namespace] = true;
    this.actionErrors[namespace] = null;
    try {
      await requestEmpty(
        `/api/v1/namespaces/${encodeURIComponent(namespace)}/actions/runners/${runnerId}`,
        { method: "DELETE" },
      );
      if (!this.current(scope)) return;
      this.actionRunners = {
        ...this.actionRunners,
        [namespace]: (this.actionRunners[namespace] ?? []).filter(
          (runner) => runner.id !== runnerId,
        ),
      };
      toast.success("Runner removed");
    } catch (caught) {
      if (!this.current(scope)) return;
      const message =
        caught instanceof ApiFailure || caught instanceof Error
          ? caught.message
          : "Could not remove the runner.";
      this.actionErrors[namespace] = message;
      toast.error(message);
    } finally {
      this.actionsWorking[namespace] = false;
    }
  }

  closeActionRegistration(): void {
    this.actionRegistration = null;
  }
}
