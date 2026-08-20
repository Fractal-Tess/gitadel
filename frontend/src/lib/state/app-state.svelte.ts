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

  async initialize(): Promise<AuthStatus> {
    this.loading = true;
    this.error = null;
    try {
      const [status, instance] = await Promise.all([
        requestJson("/api/v1/auth/status", authStatusSchema),
        requestJson("/api/v1/instance", instanceSettingsSchema),
      ]);
      this.authStatus = status;
      this.instance = instance;
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
      return status;
    } catch (caught) {
      this.error = caught instanceof Error ? caught.message : "Could not load Gitadel.";
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
