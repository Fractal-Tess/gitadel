import { z } from "zod";
import { toast } from "svelte-sonner";
import {
  webhookDeliverySchema,
  webhookSchema,
  type Webhook,
  type WebhookDelivery,
} from "$lib/api/webhooks.js";
import { jsonBody, requestEmpty, requestJson } from "$lib/api/transport.js";
import { loadRepositoryWebhooks } from "$lib/repository/repository-data-cache.js";
import type { RepositoryFeatureContext } from "./shared.js";
import { errorMessage, repositoryHooksApi } from "./shared.js";

type WebhookCallbacks = {
  setError(message: string): void;
  canManage(): boolean;
  isScopeCurrent(): boolean;
};
export class RepositoryWebhooksState {
  readonly namespace: string;
  readonly name: string;
  readonly scope: RepositoryFeatureContext["scope"];
  readonly setError: WebhookCallbacks["setError"];
  readonly canManage: WebhookCallbacks["canManage"];
  readonly isScopeCurrent: WebhookCallbacks["isScopeCurrent"];
  webhooks = $state.raw<Webhook[]>([]);
  webhookDeliveries = $state<Record<string, WebhookDelivery[]>>({});
  expandedWebhookId = $state<string | null>(null);
  webhookDeliveriesLoadingId = $state<string | null>(null);
  redeliveringDeliveryId = $state<string | null>(null);
  webhookUrl = $state("");
  webhookSecret = $state("");
  webhookActive = $state(true);
  webhooksLoading = $state(false);
  webhooksLoaded = $state(false);
  webhookCreating = $state(false);
  webhookUpdatingId = $state<string | null>(null);
  webhookPingingId = $state<string | null>(null);
  webhookDeletingId = $state<string | null>(null);
  webhookActionPending = $derived(
    this.webhookUpdatingId !== null ||
      this.webhookPingingId !== null ||
      this.webhookDeletingId !== null,
  );
  #destroyed = false;
  #timers: number[] = [];
  constructor(context: RepositoryFeatureContext, callbacks: WebhookCallbacks) {
    this.namespace = context.locator.namespace;
    this.name = context.locator.name;
    this.scope = context.scope;
    this.setError = callbacks.setError;
    this.canManage = callbacks.canManage;
    this.isScopeCurrent = callbacks.isScopeCurrent;
  }
  destroy(): void {
    this.#destroyed = true;
    for (const timer of this.#timers) window.clearTimeout(timer);
    this.#timers = [];
  }
  async loadWebhooks(init: RequestInit): Promise<void> {
    if (!this.canManage() || (this.webhooksLoaded && init.signal)) return;
    this.webhooksLoading = true;
    try {
      const webhooks = await loadRepositoryWebhooks(
        this.namespace,
        this.name,
        this.scope,
        !init.signal,
      );
      if (init.signal?.aborted || this.#destroyed || !this.isScopeCurrent())
        return;
      this.webhooks = webhooks;
      this.webhooksLoaded = true;
    } finally {
      this.webhooksLoading = false;
    }
  }
  async createWebhook(): Promise<void> {
    this.webhookCreating = true;
    try {
      const hook = await requestJson(repositoryHooksApi(this), webhookSchema, {
        method: "POST",
        body: jsonBody({
          name: "web",
          active: this.webhookActive,
          events: ["push"],
          config: {
            url: this.webhookUrl,
            content_type: "json",
            ...(this.webhookSecret && { secret: this.webhookSecret }),
          },
        }),
      });
      if (!this.isScopeCurrent()) return;
      this.webhooks = [...this.webhooks, hook];
      this.webhooksLoaded = true;
      this.webhookUrl = "";
      this.webhookSecret = "";
      this.webhookActive = true;
      toast.success("Webhook created. A ping delivery has been queued.");
      this.defer(() => this.refreshWebhooks());
    } catch (caught) {
      toast.error(errorMessage(caught));
    } finally {
      this.webhookCreating = false;
    }
  }
  async updateWebhook(
    hook: Webhook,
    url: string,
    secret: string,
  ): Promise<void> {
    this.webhookUpdatingId = hook.id;
    try {
      const updated = await requestJson(
        `${repositoryHooksApi(this)}/${hook.id}`,
        webhookSchema,
        {
          method: "PATCH",
          body: jsonBody({
            config: { url, content_type: "json", ...(secret && { secret }) },
          }),
        },
      );
      if (!this.isScopeCurrent()) return;
      this.webhooks = this.webhooks.map((item) =>
        item.id === updated.id ? updated : item,
      );
      toast.success("Webhook updated. Send a ping to verify the endpoint.");
    } catch (caught) {
      toast.error(errorMessage(caught));
      throw caught;
    } finally {
      this.webhookUpdatingId = null;
    }
  }
  async setWebhookActive(hook: Webhook, active: boolean): Promise<void> {
    this.webhookUpdatingId = hook.id;
    try {
      const updated = await requestJson(
        `${repositoryHooksApi(this)}/${hook.id}`,
        webhookSchema,
        { method: "PATCH", body: jsonBody({ active }) },
      );
      if (!this.isScopeCurrent()) return;
      this.webhooks = this.webhooks.map((item) =>
        item.id === updated.id ? updated : item,
      );
      toast.success(active ? "Webhook enabled." : "Webhook disabled.");
    } catch (caught) {
      toast.error(errorMessage(caught));
    } finally {
      this.webhookUpdatingId = null;
    }
  }
  async pingWebhook(id: string): Promise<void> {
    this.webhookPingingId = id;
    try {
      await requestEmpty(`${repositoryHooksApi(this)}/${id}/pings`, {
        method: "POST",
      });
      if (!this.isScopeCurrent()) return;
      toast.success("Ping delivery queued.");
      this.defer(() => this.refreshWebhookActivity(id));
    } catch (caught) {
      toast.error(errorMessage(caught));
    } finally {
      this.webhookPingingId = null;
    }
  }
  async toggleWebhookDeliveries(hookId: string): Promise<void> {
    if (this.expandedWebhookId === hookId) {
      this.expandedWebhookId = null;
      return;
    }
    this.expandedWebhookId = hookId;
    if (!this.webhookDeliveries[hookId])
      await this.loadWebhookDeliveries(hookId);
  }
  async redeliverWebhookDelivery(
    hookId: string,
    deliveryId: string,
  ): Promise<void> {
    this.redeliveringDeliveryId = deliveryId;
    try {
      await requestEmpty(
        `${repositoryHooksApi(this)}/${hookId}/deliveries/${deliveryId}/attempts`,
        { method: "POST" },
      );
      if (!this.isScopeCurrent()) return;
      toast.success("Redelivery queued.");
      this.defer(() => this.refreshWebhookActivity(hookId));
    } catch (caught) {
      toast.error(errorMessage(caught));
    } finally {
      this.redeliveringDeliveryId = null;
    }
  }
  async deleteWebhook(id: string): Promise<void> {
    this.webhookDeletingId = id;
    try {
      await requestEmpty(`${repositoryHooksApi(this)}/${id}`, {
        method: "DELETE",
      });
      if (!this.isScopeCurrent()) return;
      this.webhooks = this.webhooks.filter((hook) => hook.id !== id);
      if (this.expandedWebhookId === id) this.expandedWebhookId = null;
      const { [id]: removed, ...deliveries } = this.webhookDeliveries;
      this.webhookDeliveries = deliveries;
      toast.success("Webhook deleted.");
    } catch (caught) {
      toast.error(errorMessage(caught));
    } finally {
      this.webhookDeletingId = null;
    }
  }
  private async loadWebhookDeliveries(hookId: string): Promise<void> {
    this.webhookDeliveriesLoadingId = hookId;
    try {
      const deliveries = await requestJson(
        `${repositoryHooksApi(this)}/${encodeURIComponent(hookId)}/deliveries`,
        z.array(webhookDeliverySchema),
      );
      if (!this.#destroyed && this.isScopeCurrent())
        this.webhookDeliveries = {
          ...this.webhookDeliveries,
          [hookId]: deliveries,
        };
    } catch (caught) {
      this.setError(errorMessage(caught));
    } finally {
      this.webhookDeliveriesLoadingId = null;
    }
  }
  private async refreshWebhookActivity(hookId: string): Promise<void> {
    await this.refreshWebhooks();
    if (this.expandedWebhookId === hookId)
      await this.loadWebhookDeliveries(hookId);
  }
  private async refreshWebhooks(): Promise<void> {
    try {
      const webhooks = await loadRepositoryWebhooks(
        this.namespace,
        this.name,
        this.scope,
        true,
      );
      if (!this.#destroyed && this.isScopeCurrent()) this.webhooks = webhooks;
    } catch (caught) {
      this.setError(errorMessage(caught));
    }
  }
  private defer(callback: () => void): void {
    const timer = window.setTimeout(callback, 1500);
    this.#timers.push(timer);
  }
}
