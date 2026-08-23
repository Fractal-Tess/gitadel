<script lang="ts">
  import {
    Activity,
    ChevronDown,
    ChevronUp,
    History,
    Pencil,
    Radio,
    RotateCw,
    Send,
    Trash2,
    Webhook,
  } from "lucide-svelte";

  import { Button } from "$lib/components/ui/button/index.js";
  import * as Card from "$lib/components/ui/card/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Switch } from "$lib/components/ui/switch/index.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let { state: repository }: { state: RepositoryPageState } = $props();
  let editingId = $state<string | null>(null);
  let editUrl = $state("");
  let editSecret = $state("");
  let openDeliveryId = $state<string | null>(null);

  function formatDelivery(value: string | null) {
    if (!value) return "Not delivered";
    return new Intl.DateTimeFormat(undefined, {
      dateStyle: "medium",
      timeStyle: "short",
    }).format(new Date(value));
  }

  function formatDuration(durationMs: number) {
    if (durationMs < 1000) return `${durationMs} ms`;
    return `${(durationMs / 1000).toFixed(1)} s`;
  }

  function formatPayload(payload: unknown) {
    try {
      return JSON.stringify(payload, null, 2);
    } catch {
      return String(payload);
    }
  }

  function toggleDelivery(deliveryId: string) {
    openDeliveryId = openDeliveryId === deliveryId ? null : deliveryId;
  }

  function startEditing(id: string, url: string) {
    editingId = id;
    editUrl = url;
    editSecret = "";
  }

  async function saveEdit(hook: (typeof repository.webhooks)[number]) {
    try {
      await repository.updateWebhook(hook, editUrl, editSecret);
      editingId = null;
      editSecret = "";
    } catch {
      // The page-level error region explains how to recover.
    }
  }

  function deleteWebhook(id: string, url: string) {
    if (globalThis.confirm(`Delete the webhook for ${url}?`)) {
      void repository.deleteWebhook(id);
    }
  }
</script>

<Card.Root class="self-start">
  <Card.Header class="border-b">
    <div class="flex items-start gap-3">
      <Webhook class="mt-0.5 size-4 shrink-0 text-foreground/70" />
      <div>
        <h2 class="text-base font-medium leading-snug">Webhooks</h2>
        <Card.Description class="text-foreground/70">
          Send GitHub-style payloads when this repository receives a push.
        </Card.Description>
      </div>
    </div>
  </Card.Header>
  <Card.Content
    class="grid gap-5 lg:grid-cols-[minmax(0,1.25fr)_minmax(18rem,0.75fr)]"
  >
    <ul
      class="divide-y rounded-lg border"
      aria-busy={repository.webhooksLoading}
    >
      {#if repository.webhooksLoading && !repository.webhooksLoaded}
        <li class="px-5 py-12 text-center text-sm text-foreground/70">
          Loading webhooks…
        </li>
      {:else if !repository.webhooksLoaded}
        <li class="px-5 py-12 text-center text-sm text-foreground/70">
          Webhooks are unavailable. Retry by reopening repository settings.
        </li>
      {:else}
        {#each repository.webhooks as hook (hook.id)}
          <li class="grid gap-4 p-4">
            <div class="min-w-0">
              <div class="flex items-center gap-2">
                <span
                  class={hook.active
                    ? "size-2 shrink-0 rounded-full bg-emerald-500"
                    : "size-2 shrink-0 rounded-full bg-foreground/30"}
                  aria-hidden="true"
                ></span>
                <p class="truncate font-mono text-sm">{hook.config.url}</p>
              </div>
              <div
                class="mt-2 flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-foreground/70"
              >
                <span class="inline-flex items-center gap-1.5">
                  <Activity class="size-3.5" />Push event
                </span>
                <span
                  class={[
                    hook.last_response.status !== "unused" && "font-medium",
                    hook.last_response.status === "ok" && "text-emerald-500",
                    hook.last_response.status === "failed" &&
                      "text-destructive",
                  ]}
                >
                  {hook.last_response.code
                    ? `HTTP ${hook.last_response.code}`
                    : hook.last_response.status === "failed"
                      ? "Delivery failed"
                      : "Not delivered"}
                </span>
                <span>{formatDelivery(hook.last_delivery_at)}</span>
              </div>
              {#if hook.last_response.message}
                <p
                  class="mt-2 line-clamp-2 text-xs leading-5 text-foreground/70"
                  title={hook.last_response.message}
                >
                  {hook.last_response.message}
                </p>
              {/if}
            </div>

            <!-- deliveries section -->

            <div class="flex flex-wrap items-center justify-end gap-2">
              <label
                class="mr-1 flex min-h-11 items-center gap-2 text-xs text-foreground/70"
              >
                <span>{hook.active ? "Active" : "Paused"}</span>
                <Switch
                  size="sm"
                  checked={hook.active}
                  disabled={repository.webhookActionPending}
                  aria-label={`${hook.active ? "Disable" : "Enable"} webhook for ${hook.config.url}`}
                  onclick={() =>
                    void repository.setWebhookActive(hook, !hook.active)}
                />
              </label>
              <Button
                type="button"
                size="icon-sm"
                variant="ghost"
                class="max-sm:size-11"
                disabled={repository.webhookActionPending}
                aria-label={`Edit webhook for ${hook.config.url}`}
                onclick={() => startEditing(hook.id, hook.config.url)}
              >
                <Pencil class="size-3.5" />
              </Button>
              <Button
                type="button"
                size="sm"
                variant="outline"
                class="gap-2 max-sm:h-11"
                disabled={repository.webhookActionPending}
                onclick={() => void repository.pingWebhook(hook.id)}
              >
                <Send class="size-3.5" />
                {repository.webhookPingingId === hook.id ? "Pinging…" : "Ping"}
              </Button>
              <Button
                type="button"
                size="icon-sm"
                variant="ghost"
                class="text-foreground/70 hover:text-destructive max-sm:size-11"
                disabled={repository.webhookActionPending}
                aria-label={`Delete webhook for ${hook.config.url}`}
                onclick={() => deleteWebhook(hook.id, hook.config.url)}
              >
                <Trash2 class="size-3.5" />
              </Button>
            </div>

            {#if editingId === hook.id}
              <form
                class="grid gap-3 border-t pt-4 sm:grid-cols-2"
                aria-label={`Edit webhook for ${hook.config.url}`}
                onsubmit={(event) => {
                  event.preventDefault();
                  void saveEdit(hook);
                }}
              >
                <Field.Field>
                  <Field.Label for={`edit-webhook-url-${hook.id}`}
                    >Payload URL</Field.Label
                  >
                  <Input
                    id={`edit-webhook-url-${hook.id}`}
                    class="placeholder:text-foreground/60"
                    type="url"
                    bind:value={editUrl}
                    maxlength={2048}
                    required
                  />
                </Field.Field>
                <Field.Field>
                  <Field.Label for={`edit-webhook-secret-${hook.id}`}
                    >Rotate secret</Field.Label
                  >
                  <Input
                    id={`edit-webhook-secret-${hook.id}`}
                    class="placeholder:text-foreground/60"
                    type="password"
                    bind:value={editSecret}
                    placeholder="Leave blank to keep the current secret"
                    autocomplete="new-password"
                    maxlength={256}
                  />
                </Field.Field>
                <div class="flex justify-end gap-2 sm:col-span-2">
                  <Button
                    type="button"
                    variant="ghost"
                    class="max-sm:h-11"
                    onclick={() => (editingId = null)}
                  >
                    Cancel
                  </Button>
                  <Button
                    type="submit"
                    class="max-sm:h-11"
                    disabled={repository.webhookActionPending}
                  >
                    {repository.webhookUpdatingId === hook.id
                      ? "Saving…"
                      : "Save changes"}
                  </Button>
                </div>
              </form>
            {/if}

            <div class="border-t pt-3">
              <Button
                type="button"
                size="sm"
                variant="ghost"
                class="gap-2"
                aria-expanded={repository.expandedWebhookId === hook.id}
                onclick={() => void repository.toggleWebhookDeliveries(hook.id)}
              >
                <History class="size-3.5" />
                Recent deliveries
                {#if repository.expandedWebhookId === hook.id}
                  <ChevronUp class="size-3.5" />
                {:else}
                  <ChevronDown class="size-3.5" />
                {/if}
              </Button>
              {#if repository.expandedWebhookId === hook.id}
                {#if repository.webhookDeliveriesLoadingId === hook.id && !repository.webhookDeliveries[hook.id]}
                  <p class="mt-2 text-xs text-foreground/70">
                    Loading deliveries…
                  </p>
                {:else if !repository.webhookDeliveries[hook.id]?.length}
                  <p class="mt-2 text-xs leading-5 text-foreground/70">
                    No deliveries recorded yet. Use Ping or push to this
                    repository to trigger one.
                  </p>
                {:else}
                  <ul
                    class="mt-2 divide-y rounded-md border text-sm"
                    aria-label={`Recent deliveries for ${hook.config.url}`}
                  >
                    {#each repository.webhookDeliveries[hook.id] as delivery (delivery.id)}
                      <li>
                        <div
                          class="flex flex-wrap items-center gap-x-3 gap-y-1 px-3 py-2"
                        >
                          <span
                            class={[
                              "size-2 shrink-0 rounded-full",
                              delivery.status === "ok"
                                ? "bg-emerald-500"
                                : "bg-destructive",
                            ]}
                            aria-hidden="true"
                          ></span>
                          <span
                            class={[
                              "font-mono text-xs",
                              delivery.status === "ok"
                                ? "text-emerald-500"
                                : "text-destructive",
                            ]}
                          >
                            {delivery.status_code
                              ? `HTTP ${delivery.status_code}`
                              : "Failed"}
                          </span>
                          <span class="font-medium text-xs"
                            >{delivery.event}</span
                          >
                          <span class="text-xs text-foreground/70">
                            {formatDelivery(delivery.delivered_at)}
                          </span>
                          <span class="text-xs text-foreground/70">
                            {formatDuration(delivery.duration_ms)}
                          </span>
                          <div class="ml-auto flex items-center gap-1">
                            <Button
                              type="button"
                              size="icon-sm"
                              variant="ghost"
                              class="max-sm:size-8"
                              aria-expanded={openDeliveryId === delivery.id}
                              aria-label={openDeliveryId === delivery.id
                                ? `Hide delivery details for ${delivery.event}`
                                : `Show delivery details for ${delivery.event}`}
                              onclick={() => toggleDelivery(delivery.id)}
                            >
                              {#if openDeliveryId === delivery.id}
                                <ChevronUp class="size-3.5" />
                              {:else}
                                <ChevronDown class="size-3.5" />
                              {/if}
                            </Button>
                            <Button
                              type="button"
                              size="sm"
                              variant="outline"
                              class="gap-1.5 max-sm:h-8"
                              disabled={repository.redeliveringDeliveryId !==
                                null || repository.webhookActionPending}
                              aria-label={`Redeliver ${delivery.event} delivery`}
                              onclick={() =>
                                void repository.redeliverWebhookDelivery(
                                  hook.id,
                                  delivery.id,
                                )}
                            >
                              <RotateCw class="size-3" />
                              {repository.redeliveringDeliveryId === delivery.id
                                ? "Redelivering…"
                                : "Redeliver"}
                            </Button>
                          </div>
                        </div>
                        {#if openDeliveryId === delivery.id}
                          <div class="grid gap-3 border-t bg-card/50 px-3 py-3">
                            <div class="min-w-0">
                              <h4 class="text-xs font-semibold">
                                Request payload
                              </h4>
                              <pre
                                class="mt-1 max-h-64 overflow-auto rounded bg-muted/40 p-2 font-mono text-xs leading-5 whitespace-pre-wrap break-all">{formatPayload(
                                  delivery.payload,
                                )}</pre>
                            </div>
                            {#if delivery.response_body}
                              <div class="min-w-0">
                                <h4 class="text-xs font-semibold">Response</h4>
                                <pre
                                  class="mt-1 max-h-40 overflow-auto rounded bg-muted/40 p-2 font-mono text-xs leading-5 whitespace-pre-wrap break-all">{delivery.response_body}</pre>
                              </div>
                            {/if}
                          </div>
                        {/if}
                      </li>
                    {/each}
                  </ul>
                {/if}
              {/if}
            </div>
          </li>
        {:else}
          <li class="grid justify-items-center gap-2 px-5 py-12 text-center">
            <Radio class="size-5 text-foreground/70" />
            <p class="text-sm font-medium">No webhooks configured</p>
            <p class="max-w-sm text-xs leading-5 text-foreground/70">
              Add an endpoint to notify deployments and other external services
              after a successful push.
            </p>
          </li>
        {/each}
      {/if}
    </ul>

    <form
      class="grid content-start gap-4 rounded-lg border bg-card/25 p-5"
      onsubmit={(event) => {
        event.preventDefault();
        void repository.createWebhook();
      }}
    >
      <div>
        <h3 class="text-sm font-semibold">Add webhook</h3>
        <p class="mt-1 text-xs leading-5 text-foreground/70">
          JSON requests include GitHub event headers and an optional HMAC-SHA256
          signature.
        </p>
      </div>
      <Field.Field>
        <Field.Label for="webhook-url">Payload URL</Field.Label>
        <Input
          id="webhook-url"
          class="placeholder:text-foreground/60"
          type="url"
          bind:value={repository.webhookUrl}
          placeholder="https://deploy.example.com/hooks/gitadel"
          autocomplete="url"
          maxlength={2048}
          required
        />
      </Field.Field>
      <Field.Field>
        <Field.Label for="webhook-secret">Secret</Field.Label>
        <Input
          id="webhook-secret"
          class="placeholder:text-foreground/60"
          type="password"
          bind:value={repository.webhookSecret}
          placeholder="Optional signing secret"
          autocomplete="new-password"
          maxlength={256}
        />
        <Field.Description class="text-foreground/70">
          Sent in X-Hub-Signature-256. Redirects are not followed.
        </Field.Description>
      </Field.Field>
      <label
        class="flex items-center justify-between gap-4 rounded-md border p-3"
      >
        <span>
          <span class="block text-sm font-medium">Active</span>
          <span class="mt-0.5 block text-xs text-foreground/70"
            >Deliver push events immediately.</span
          >
        </span>
        <Switch bind:checked={repository.webhookActive} />
      </label>
      <Button type="submit" disabled={repository.webhookCreating}>
        {repository.webhookCreating ? "Adding webhook…" : "Add webhook"}
      </Button>
    </form>
  </Card.Content>
</Card.Root>
