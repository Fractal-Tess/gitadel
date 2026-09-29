<script lang="ts">
  import KeyRound from "@lucide/svelte/icons/key-round";
  import Trash2 from "@lucide/svelte/icons/trash-2";
  import { z } from "zod";
  import { toast } from "svelte-sonner";

  import {
    deployKeySchema,
    type DeployKey,
  } from "$lib/api/repository-security.js";
  import {
    jsonBody,
    requestEmpty,
    requestJson,
  } from "$lib/api/transport.js";
  import { Badge } from "$lib/components/ui/badge/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Switch } from "$lib/components/ui/switch/index.js";
  import { Textarea } from "$lib/components/ui/textarea/index.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";
  import { errorMessage, repositoryApi } from "$lib/repository/state/shared.js";

  let { state: repository }: { state: RepositoryPageState } = $props();

  const endpoint = $derived(
    repositoryApi(
      { namespace: repository.namespace, name: repository.name },
      "/deploy-keys",
    ),
  );

  let keys = $state.raw<DeployKey[]>([]);
  let loading = $state(true);
  let loadError = $state<string | null>(null);
  let deletingId = $state<string | null>(null);
  let creating = $state(false);
  let title = $state("");
  let publicKey = $state("");
  let allowWrite = $state(false);

  $effect(() => {
    const url = endpoint;
    const controller = new AbortController();
    loading = true;
    loadError = null;
    requestJson(url, z.array(deployKeySchema), { signal: controller.signal })
      .then((loaded) => {
        keys = loaded;
      })
      .catch((caught) => {
        if (!controller.signal.aborted) loadError = errorMessage(caught);
      })
      .finally(() => {
        if (!controller.signal.aborted) loading = false;
      });
    return () => controller.abort();
  });

  function formatDate(value: string | null) {
    if (!value) return "Never used";
    return new Intl.DateTimeFormat(undefined, {
      dateStyle: "medium",
      timeStyle: "short",
    }).format(new Date(value));
  }

  async function createKey() {
    creating = true;
    try {
      const key = await requestJson(endpoint, deployKeySchema, {
        method: "POST",
        body: jsonBody({ title, key: publicKey, read_only: !allowWrite }),
      });
      keys = [...keys, key];
      title = "";
      publicKey = "";
      allowWrite = false;
      toast.success("Deploy key added.");
    } catch (caught) {
      toast.error(errorMessage(caught));
    } finally {
      creating = false;
    }
  }

  async function deleteKey(key: DeployKey) {
    deletingId = key.id;
    try {
      await requestEmpty(`${endpoint}/${key.id}`, { method: "DELETE" });
      keys = keys.filter((item) => item.id !== key.id);
      toast.success("Deploy key removed.");
    } catch (caught) {
      toast.error(errorMessage(caught));
    } finally {
      deletingId = null;
    }
  }
</script>

<div
  class="divide-y divide-border overflow-hidden rounded-xl bg-card/20 ring-1 ring-foreground/15"
>
  <section
    class="grid gap-5 p-5 md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10 md:p-6"
    aria-labelledby="repository-deploy-keys-heading"
  >
    <header class="flex items-start gap-3">
      <KeyRound class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
      <div>
        <h2 id="repository-deploy-keys-heading" class="font-semibold">
          Deploy keys
        </h2>
        <p class="mt-1 max-w-xs text-sm leading-5 text-muted-foreground">
          SSH keys that can clone, and optionally push to, this repository
          only. A key cannot also be registered to a user account.
        </p>
      </div>
    </header>

    <div class="grid max-w-2xl gap-3">
      {#if loading}
        <p class="text-sm text-muted-foreground">Loading deploy keys…</p>
      {:else if loadError}
        <p class="text-sm text-destructive">{loadError}</p>
      {:else if keys.length === 0}
        <p class="text-sm text-muted-foreground">No deploy keys.</p>
      {:else}
        <ul class="motion-list grid gap-3">
          {#each keys as key (key.id)}
            <li
              class="flex flex-wrap items-start justify-between gap-3 rounded-md border p-4"
            >
              <div class="min-w-0">
                <div class="flex items-center gap-2">
                  <span class="font-medium">{key.title}</span>
                  <Badge variant="outline">
                    {key.read_only ? "Read-only" : "Read-write"}
                  </Badge>
                </div>
                <p
                  class="mt-1 truncate font-mono text-xs text-muted-foreground"
                >
                  {key.fingerprint}
                </p>
                <p class="mt-1 text-xs text-muted-foreground">
                  Added {formatDate(key.created_at)} · {key.last_used_at
                    ? `Last used ${formatDate(key.last_used_at)}`
                    : "Never used"}
                </p>
              </div>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                disabled={deletingId === key.id}
                aria-label={`Remove deploy key ${key.title}`}
                onclick={() => void deleteKey(key)}
              >
                <Trash2 class="size-3.5" />
              </Button>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  </section>

  <section
    class="grid gap-5 p-5 md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10 md:p-6"
    aria-labelledby="repository-deploy-key-add-heading"
  >
    <header>
      <h2 id="repository-deploy-key-add-heading" class="font-semibold">
        Add a deploy key
      </h2>
      <p class="mt-1 max-w-xs text-sm leading-5 text-muted-foreground">
        Pushes with a deploy key are attributed to you and still follow the
        branch protection rules.
      </p>
    </header>

    <form
      class="grid max-w-2xl gap-4"
      onsubmit={(event) => {
        event.preventDefault();
        void createKey();
      }}
    >
      <Field.Field>
        <Field.Label for="deploy-key-title">Title</Field.Label>
        <Input
          id="deploy-key-title"
          bind:value={title}
          maxlength={128}
          placeholder="Production server"
          required
        />
      </Field.Field>
      <Field.Field>
        <Field.Label for="deploy-key-public-key">Public key</Field.Label>
        <Textarea
          id="deploy-key-public-key"
          bind:value={publicKey}
          class="font-mono text-xs"
          rows={4}
          placeholder="ssh-ed25519 AAAA…"
          spellcheck={false}
          required
        />
      </Field.Field>
      <label class="flex items-center justify-between gap-3 text-sm">
        Allow write access
        <Switch bind:checked={allowWrite} />
      </label>
      <div class="flex justify-end">
        <Button
          type="submit"
          disabled={creating || !title.trim() || !publicKey.trim()}
        >
          {creating ? "Adding…" : "Add deploy key"}
        </Button>
      </div>
    </form>
  </section>
</div>
