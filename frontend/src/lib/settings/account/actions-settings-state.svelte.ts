import { toast } from "svelte-sonner";

import {
  actionRegistrationSchema,
  actionRunnersSchema,
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

export type ActionRunnerScope = {
  key: string;
  label: string;
  namespace: string | null;
};

export function namespaceRunnerScope(
  namespace: string,
  label: string,
): ActionRunnerScope {
  return { key: namespace, label, namespace };
}

export const systemRunnerScope: ActionRunnerScope = {
  key: "system",
  label: "All repositories",
  namespace: null,
};

function runnerBase(scope: ActionRunnerScope): string {
  return scope.namespace === null
    ? "/api/v1/admin/actions"
    : `/api/v1/namespaces/${encodeURIComponent(scope.namespace)}/actions`;
}

export class ActionsSettingsState {
  actionRunners = $state.raw<Record<string, ActionRunner[]>>({});
  actionRegistration = $state.raw<ActionRegistration | null>(null);
  actionsLoading = $state(false);
  actionsLoadError = $state<string | null>(null);
  actionLoadErrors = $state<Record<string, string | null>>({});
  actionsWorking = $state<Record<string, boolean>>({});
  scope = $state.raw<AuthorizationCacheScope>({ viewer: null, epoch: 0 });
  constructor(
    scope: AuthorizationCacheScope,
    private readonly current: ScopeGuard,
  ) {
    this.scope = scope;
  }

  setScope(scope: AuthorizationCacheScope): void {
    this.scope = scope;
  }
  async loadActionRunners(scopes: ActionRunnerScope[]): Promise<void> {
    const authorizationScope = this.scope;
    this.actionsLoading = true;
    this.actionsLoadError = null;
    const loadErrors: Record<string, string | null> = {};
    const loaded: Record<string, ActionRunner[]> = {};
    let failures = 0;
    await Promise.all(
      scopes.map(async (runnerScope) => {
        try {
          loaded[runnerScope.key] =
            runnerScope.namespace === null
              ? (
                  await requestJson(
                    `${runnerBase(runnerScope)}/runners`,
                    actionRunnersSchema,
                  )
                ).runners
              : await takeNamespaceRunners(
                  runnerScope.namespace,
                  authorizationScope,
                );
          loadErrors[runnerScope.key] = null;
        } catch (caught) {
          failures += 1;
          loaded[runnerScope.key] =
            this.actionRunners[runnerScope.key] ?? [];
          loadErrors[runnerScope.key] =
            caught instanceof ApiFailure || caught instanceof Error
              ? caught.message
              : `Could not load runners for ${runnerScope.label}.`;
        }
      }),
    );
    if (!this.current(authorizationScope)) return;
    this.actionRunners = loaded;
    this.actionLoadErrors = loadErrors;
    if (failures === scopes.length && scopes.length > 0)
      this.actionsLoadError = "Could not load runners.";
    this.actionsLoading = false;
  }

  async issueActionRunner(
    runnerScope: ActionRunnerScope,
    name: string,
    labels: string[],
  ): Promise<void> {
    const authorizationScope = this.scope;
    if (this.actionsWorking[runnerScope.key]) return;
    this.actionsWorking[runnerScope.key] = true;
    try {
      const registration = await requestJson(
        `${runnerBase(runnerScope)}/runner-registration-tokens`,
        actionRegistrationSchema,
        { method: "POST", body: jsonBody({ name, labels }) },
      );
      if (!this.current(authorizationScope)) return;
      this.actionRegistration = registration;
    } catch (caught) {
      if (!this.current(authorizationScope)) return;
      const message =
        caught instanceof ApiFailure || caught instanceof Error
          ? caught.message
          : "Could not create the runner registration token.";
      toast.error(message);
    } finally {
      this.actionsWorking[runnerScope.key] = false;
    }
  }

  async removeActionRunner(
    runnerScope: ActionRunnerScope,
    runnerId: number,
  ): Promise<boolean> {
    const authorizationScope = this.scope;
    if (this.actionsWorking[runnerScope.key]) return false;
    this.actionsWorking[runnerScope.key] = true;
    try {
      await requestEmpty(`${runnerBase(runnerScope)}/runners/${runnerId}`, {
        method: "DELETE",
      });
      if (!this.current(authorizationScope)) return false;
      this.actionRunners = {
        ...this.actionRunners,
        [runnerScope.key]: (
          this.actionRunners[runnerScope.key] ?? []
        ).filter((runner) => runner.id !== runnerId),
      };
      toast.success("Runner removed");
      return true;
    } catch (caught) {
      if (!this.current(authorizationScope)) return false;
      const message =
        caught instanceof ApiFailure || caught instanceof Error
          ? caught.message
          : "Could not remove the runner.";
      toast.error(message);
      return false;
    } finally {
      this.actionsWorking[runnerScope.key] = false;
    }
  }

  closeActionRegistration(): void {
    this.actionRegistration = null;
  }
}
