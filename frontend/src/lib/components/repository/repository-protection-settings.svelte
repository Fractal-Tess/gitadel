<script lang="ts">
  import GitBranch from "@lucide/svelte/icons/git-branch";
  import ShieldCheck from "@lucide/svelte/icons/shield-check";
  import Tag from "@lucide/svelte/icons/tag";
  import Trash2 from "@lucide/svelte/icons/trash-2";
  import { z } from "zod";
  import { toast } from "svelte-sonner";

  import {
    protectionRuleSchema,
    type ProtectionRule,
    type ProtectionRuleInput,
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
  import * as Select from "$lib/components/ui/select/index.js";
  import { Switch } from "$lib/components/ui/switch/index.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";
  import { errorMessage, repositoryApi } from "$lib/repository/state/shared.js";

  let { state: repository }: { state: RepositoryPageState } = $props();

  const endpoint = $derived(
    repositoryApi(
      { namespace: repository.namespace, name: repository.name },
      "/protection-rules",
    ),
  );

  let rules = $state.raw<ProtectionRule[]>([]);
  let loading = $state(true);
  let loadError = $state<string | null>(null);
  let pendingId = $state<string | null>(null);
  let creating = $state(false);

  let kind = $state<"branch" | "tag">("branch");
  let pattern = $state("");
  let blockForcePush = $state(true);
  let blockDeletion = $state(true);
  let blockUpdate = $state(true);
  let restrictPushes = $state(false);
  let adminsBypass = $state(true);
  let allowedUsers = $state("");

  const defaultBranch = $derived(repository.repository?.default_branch ?? null);
  const defaultBranchProtected = $derived(
    rules.some(
      (rule) => rule.kind === "branch" && rule.pattern === defaultBranch,
    ),
  );

  $effect(() => {
    const url = endpoint;
    const controller = new AbortController();
    loading = true;
    loadError = null;
    requestJson(url, z.array(protectionRuleSchema), {
      signal: controller.signal,
    })
      .then((loaded) => {
        rules = loaded;
      })
      .catch((caught) => {
        if (!controller.signal.aborted) loadError = errorMessage(caught);
      })
      .finally(() => {
        if (!controller.signal.aborted) loading = false;
      });
    return () => controller.abort();
  });

  function usernames(value: string): string[] {
    return value
      .split(/[\s,]+/)
      .map((name) => name.trim())
      .filter(Boolean);
  }

  function sortRules(list: ProtectionRule[]): ProtectionRule[] {
    return [...list].sort(
      (a, b) =>
        a.kind.localeCompare(b.kind) || a.pattern.localeCompare(b.pattern),
    );
  }

  async function createRule(input: ProtectionRuleInput) {
    creating = true;
    try {
      const rule = await requestJson(endpoint, protectionRuleSchema, {
        method: "POST",
        body: jsonBody(input),
      });
      rules = sortRules([...rules, rule]);
      toast.success(`Protected ${rule.kind} ${rule.pattern}.`);
      return true;
    } catch (caught) {
      toast.error(errorMessage(caught));
      return false;
    } finally {
      creating = false;
    }
  }

  async function submit() {
    const created = await createRule({
      kind,
      pattern,
      block_force_push: kind === "branch" && blockForcePush,
      block_deletion: blockDeletion,
      block_update: kind === "tag" && blockUpdate,
      restrict_pushes: restrictPushes,
      admins_bypass: adminsBypass,
      allowed_users: restrictPushes ? usernames(allowedUsers) : [],
    });
    if (created) {
      pattern = "";
      allowedUsers = "";
      restrictPushes = false;
    }
  }

  async function updateRule(rule: ProtectionRule, change: ProtectionRuleInput) {
    pendingId = rule.id;
    try {
      const updated = await requestJson(
        `${endpoint}/${rule.id}`,
        protectionRuleSchema,
        { method: "PATCH", body: jsonBody(change) },
      );
      rules = rules.map((item) => (item.id === updated.id ? updated : item));
    } catch (caught) {
      toast.error(errorMessage(caught));
    } finally {
      pendingId = null;
    }
  }

  async function deleteRule(rule: ProtectionRule) {
    pendingId = rule.id;
    try {
      await requestEmpty(`${endpoint}/${rule.id}`, { method: "DELETE" });
      rules = rules.filter((item) => item.id !== rule.id);
      toast.success(`Removed protection for ${rule.pattern}.`);
    } catch (caught) {
      toast.error(errorMessage(caught));
    } finally {
      pendingId = null;
    }
  }
</script>

<div
  class="divide-y divide-border overflow-hidden rounded-xl bg-card/20 ring-1 ring-foreground/15"
>
  <section
    class="grid gap-5 p-5 md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10 md:p-6"
    aria-labelledby="repository-protection-heading"
  >
    <header class="flex items-start gap-3">
      <ShieldCheck class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
      <div>
        <h2 id="repository-protection-heading" class="font-semibold">
          Protected branches and tags
        </h2>
        <p class="mt-1 max-w-xs text-sm leading-5 text-muted-foreground">
          Rules apply to every push over HTTP and SSH and to commits made in
          the browser. Patterns match exact names or globs such as
          <code class="font-mono text-xs">release/*</code> or
          <code class="font-mono text-xs">v*</code>.
        </p>
      </div>
    </header>

    <div class="grid max-w-2xl gap-4">
      {#if defaultBranch && !loading && !loadError && !defaultBranchProtected}
        <div
          class="flex flex-wrap items-center justify-between gap-3 rounded-md border bg-muted/30 p-3"
        >
          <p class="text-sm text-muted-foreground">
            The default branch <span class="font-mono text-foreground"
              >{defaultBranch}</span
            > is not protected.
          </p>
          <Button
            type="button"
            size="sm"
            disabled={creating}
            onclick={() =>
              void createRule({ kind: "branch", pattern: defaultBranch })}
          >
            <ShieldCheck data-icon="inline-start" />Protect {defaultBranch}
          </Button>
        </div>
      {/if}

      {#if loading}
        <p class="text-sm text-muted-foreground">Loading protection rules…</p>
      {:else if loadError}
        <p class="text-sm text-destructive">{loadError}</p>
      {:else if rules.length === 0}
        <p class="text-sm text-muted-foreground">
          No branches or tags are protected.
        </p>
      {:else}
        <ul class="grid gap-3">
          {#each rules as rule (rule.id)}
            <li class="grid gap-3 rounded-md border p-4">
              <div class="flex flex-wrap items-center justify-between gap-2">
                <div class="flex min-w-0 items-center gap-2">
                  {#if rule.kind === "branch"}
                    <GitBranch class="size-4 shrink-0 text-muted-foreground" />
                  {:else}
                    <Tag class="size-4 shrink-0 text-muted-foreground" />
                  {/if}
                  <span class="truncate font-mono text-sm">{rule.pattern}</span>
                  <Badge variant="outline">{rule.kind}</Badge>
                </div>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  disabled={pendingId === rule.id}
                  aria-label={`Remove protection for ${rule.pattern}`}
                  onclick={() => void deleteRule(rule)}
                >
                  <Trash2 class="size-3.5" />
                </Button>
              </div>
              <div class="grid gap-2 text-sm sm:grid-cols-2">
                {#if rule.kind === "branch"}
                  {@render toggle(
                    rule,
                    "Block force-push",
                    rule.block_force_push,
                    (value) => ({ block_force_push: value }),
                  )}
                {:else}
                  {@render toggle(
                    rule,
                    "Block moving existing tags",
                    rule.block_update,
                    (value) => ({ block_update: value }),
                  )}
                {/if}
                {@render toggle(
                  rule,
                  "Block deletion",
                  rule.block_deletion,
                  (value) => ({ block_deletion: value }),
                )}
                {@render toggle(
                  rule,
                  "Restrict who can push",
                  rule.restrict_pushes,
                  (value) => ({ restrict_pushes: value }),
                )}
                {#if rule.restrict_pushes}
                  {@render toggle(
                    rule,
                    "Admins may always push",
                    rule.admins_bypass,
                    (value) => ({ admins_bypass: value }),
                  )}
                {/if}
              </div>
              {#if rule.restrict_pushes}
                <p class="text-xs text-muted-foreground">
                  {rule.allowed_users.length
                    ? `Allowed: ${rule.allowed_users.join(", ")}`
                    : "No users are allowed to push."}
                </p>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  </section>

  <section
    class="grid gap-5 p-5 md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10 md:p-6"
    aria-labelledby="repository-protection-add-heading"
  >
    <header>
      <h2 id="repository-protection-add-heading" class="font-semibold">
        Add a rule
      </h2>
      <p class="mt-1 max-w-xs text-sm leading-5 text-muted-foreground">
        <code class="font-mono text-xs">*</code> stays within one path
        component; <code class="font-mono text-xs">**</code> also matches
        slashes.
      </p>
    </header>

    <form
      class="grid max-w-2xl gap-4"
      onsubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      <div class="grid gap-4 sm:grid-cols-[10rem_minmax(0,1fr)]">
        <Field.Field>
          <Field.Label>Applies to</Field.Label>
          <Select.Root type="single" bind:value={kind}>
            <Select.Trigger class="w-full">
              {kind === "branch" ? "Branches" : "Tags"}
            </Select.Trigger>
            <Select.Content>
              <Select.Item value="branch">Branches</Select.Item>
              <Select.Item value="tag">Tags</Select.Item>
            </Select.Content>
          </Select.Root>
        </Field.Field>
        <Field.Field>
          <Field.Label for="protection-pattern">Pattern</Field.Label>
          <Input
            id="protection-pattern"
            bind:value={pattern}
            placeholder={kind === "branch" ? "release/*" : "v*"}
            maxlength={255}
            autocomplete="off"
            spellcheck={false}
            required
          />
        </Field.Field>
      </div>

      <div class="grid gap-3 text-sm sm:grid-cols-2">
        {#if kind === "branch"}
          <label class="flex items-center justify-between gap-3">
            Block force-push
            <Switch bind:checked={blockForcePush} />
          </label>
        {:else}
          <label class="flex items-center justify-between gap-3">
            Block moving existing tags
            <Switch bind:checked={blockUpdate} />
          </label>
        {/if}
        <label class="flex items-center justify-between gap-3">
          Block deletion
          <Switch bind:checked={blockDeletion} />
        </label>
        <label class="flex items-center justify-between gap-3">
          Restrict who can push
          <Switch bind:checked={restrictPushes} />
        </label>
        {#if restrictPushes}
          <label class="flex items-center justify-between gap-3">
            Admins may always push
            <Switch bind:checked={adminsBypass} />
          </label>
        {/if}
      </div>

      {#if restrictPushes}
        <Field.Field>
          <Field.Label for="protection-users">Users allowed to push</Field.Label>
          <Input
            id="protection-users"
            bind:value={allowedUsers}
            placeholder="alice, bob"
            autocomplete="off"
            spellcheck={false}
          />
          <Field.Description>
            Usernames separated by commas or spaces. Deploy keys are never on
            this list.
          </Field.Description>
        </Field.Field>
      {/if}

      <div class="flex justify-end">
        <Button type="submit" disabled={creating || !pattern.trim()}>
          {creating ? "Adding…" : "Add rule"}
        </Button>
      </div>
    </form>
  </section>
</div>

{#snippet toggle(
  rule: ProtectionRule,
  label: string,
  checked: boolean,
  change: (value: boolean) => ProtectionRuleInput,
)}
  <label class="flex items-center justify-between gap-3">
    {label}
    <Switch
      {checked}
      disabled={pendingId === rule.id}
      onCheckedChange={(value) => void updateRule(rule, change(value))}
    />
  </label>
{/snippet}
