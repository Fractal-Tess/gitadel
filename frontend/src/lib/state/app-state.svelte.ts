import { getContext, setContext } from "svelte";

import {
  authStatusSchema,
  instanceSettingsSchema,
  requestJson,
  type AuthStatus,
  type InstanceSettings,
} from "$lib/api.js";

const APP_STATE = Symbol("gitadel-app-state");

export class AppState {
  authStatus = $state.raw<AuthStatus | null>(null);
  instance = $state.raw<InstanceSettings | null>(null);
  loading = $state(true);
  error = $state<string | null>(null);

  #initializedAt = 0;
  #initializing: Promise<AuthStatus> | null = null;

  async initialize(): Promise<AuthStatus> {
    if (
      this.authStatus &&
      this.instance &&
      Date.now() - this.#initializedAt < 30_000
    ) {
      return this.authStatus;
    }
    if (this.#initializing) return this.#initializing;

    this.#initializing = this.#loadInitialState();
    try {
      return await this.#initializing;
    } finally {
      this.#initializing = null;
    }
  }

  async #loadInitialState(): Promise<AuthStatus> {
    this.loading = true;
    this.error = null;
    try {
      const [status, instance] = await Promise.all([
        requestJson("/api/v1/auth/status", authStatusSchema),
        requestJson("/api/v1/instance", instanceSettingsSchema),
      ]);
      this.authStatus = status;
      this.instance = instance;
      this.#initializedAt = Date.now();
      return status;
    } catch (caught) {
      this.error =
        caught instanceof Error ? caught.message : "Could not load Gitadel.";
      throw caught;
    } finally {
      this.loading = false;
    }
  }

  async refreshInstance(): Promise<InstanceSettings> {
    const instance = await requestJson(
      "/api/v1/instance",
      instanceSettingsSchema,
    );
    this.instance = instance;
    return instance;
  }

  async refreshAuth(): Promise<AuthStatus> {
    this.loading = true;
    this.error = null;
    try {
      const status = await requestJson("/api/v1/auth/status", authStatusSchema);
      this.authStatus = status;
      this.#initializedAt = Date.now();
      return status;
    } catch (caught) {
      this.error =
        caught instanceof Error ? caught.message : "Could not load Gitadel.";
      throw caught;
    } finally {
      this.loading = false;
    }
  }
}

export function provideAppState(): AppState {
  const state = new AppState();
  setContext(APP_STATE, state);
  return state;
}

export function useAppState(): AppState {
  return getContext<AppState>(APP_STATE);
}
