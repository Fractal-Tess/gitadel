import { toast } from "svelte-sonner";

import {
  actionSecretSchema,
  actionSecretsSchema,
  actionVariableSchema,
  actionVariablesSchema,
  type ActionSecret,
  type ActionVariable,
} from "$lib/api/actions.js";
import {
  ApiFailure,
  jsonBody,
  requestEmpty,
  requestJson,
} from "$lib/api/transport.js";

export type ActionValueKind = "secrets" | "variables";

function message(caught: unknown): string {
  return caught instanceof ApiFailure || caught instanceof Error
    ? caught.message
    : "The request failed.";
}

function byName<T extends { name: string }>(left: T, right: T): number {
  return left.name.localeCompare(right.name);
}

/**
 * Secrets and variables of one Actions scope. `base` is the scope's Actions
 * API prefix, for example `/api/v1/namespaces/team/actions`.
 */
export class ActionsSecretsVariablesState {
  secrets = $state.raw<ActionSecret[]>([]);
  variables = $state.raw<ActionVariable[]>([]);
  loading = $state(false);
  loadError = $state<string | null>(null);
  pending = $state(false);

  constructor(private readonly base: string) {}

  #path(kind: ActionValueKind, name?: string): string {
    return name === undefined
      ? `${this.base}/${kind}`
      : `${this.base}/${kind}/${encodeURIComponent(name.trim().toUpperCase())}`;
  }

  async load(): Promise<void> {
    this.loading = true;
    this.loadError = null;
    try {
      const [secrets, variables] = await Promise.all([
        requestJson(this.#path("secrets"), actionSecretsSchema),
        requestJson(this.#path("variables"), actionVariablesSchema),
      ]);
      this.secrets = secrets.secrets;
      this.variables = variables.variables;
    } catch (caught) {
      this.loadError = message(caught);
    } finally {
      this.loading = false;
    }
  }

  async save(
    kind: ActionValueKind,
    name: string,
    value: string,
  ): Promise<boolean> {
    if (this.pending) return false;
    this.pending = true;
    try {
      const init = { method: "PUT", body: jsonBody({ value }) };
      if (kind === "secrets") {
        const saved = await requestJson(
          this.#path(kind, name),
          actionSecretSchema,
          init,
        );
        this.secrets = [
          ...this.secrets.filter((secret) => secret.name !== saved.name),
          saved,
        ].sort(byName);
      } else {
        const saved = await requestJson(
          this.#path(kind, name),
          actionVariableSchema,
          init,
        );
        this.variables = [
          ...this.variables.filter((variable) => variable.name !== saved.name),
          saved,
        ].sort(byName);
      }
      toast.success(kind === "secrets" ? "Secret saved." : "Variable saved.");
      return true;
    } catch (caught) {
      toast.error(message(caught));
      return false;
    } finally {
      this.pending = false;
    }
  }

  async remove(kind: ActionValueKind, name: string): Promise<void> {
    if (this.pending) return;
    this.pending = true;
    try {
      await requestEmpty(this.#path(kind, name), { method: "DELETE" });
      if (kind === "secrets")
        this.secrets = this.secrets.filter((secret) => secret.name !== name);
      else
        this.variables = this.variables.filter(
          (variable) => variable.name !== name,
        );
      toast.success(
        kind === "secrets" ? "Secret deleted." : "Variable deleted.",
      );
    } catch (caught) {
      toast.error(message(caught));
    } finally {
      this.pending = false;
    }
  }
}
