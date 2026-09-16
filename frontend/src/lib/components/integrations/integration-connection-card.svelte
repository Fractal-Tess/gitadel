<script lang="ts">
  import {
    SiForgejo,
    SiGitea,
    SiGithub,
    SiGitlab,
  } from "@icons-pack/svelte-simple-icons";
  import Check from "@lucide/svelte/icons/check";
  import ExternalLink from "@lucide/svelte/icons/external-link";
  import Globe2 from "@lucide/svelte/icons/globe-2";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import Rocket from "@lucide/svelte/icons/rocket";
  import Settings2 from "@lucide/svelte/icons/settings-2";
  import Trash2 from "@lucide/svelte/icons/trash-2";
  import type { Snippet } from "svelte";

  import { Button } from "$lib/components/ui/button/index.js";
  import * as Alert from "$lib/components/ui/alert/index.js";
  import { Switch } from "$lib/components/ui/switch/index.js";

  let {
    name,
    provider,
    providerName,
    icon = null,
    subtitle,
    description,
    enabled,
    statusLabel = enabled ? "Enabled" : "Disabled",
    statusHealthy = enabled,
    detailLabel = null,
    busy = false,
    compact = false,
    selected = false,
    error = null,
    onselect = null,
    onenabledchange = null,
    onconfigure = null,
    configureLabel = "Configure",
    configureExternal = false,
    onremove = null,
    actions = null,
    details = null,
  }: {
    name: string;
    provider: string;
    providerName: string;
    icon?: Snippet | null;
    subtitle: string;
    description: string;
    enabled: boolean;
    statusLabel?: string;
    statusHealthy?: boolean;
    detailLabel?: string | null;
    busy?: boolean;
    compact?: boolean;
    selected?: boolean;
    error?: string | null;
    onselect?: (() => void) | null;
    onenabledchange?: ((enabled: boolean) => void) | null;
    onconfigure?: (() => void) | null;
    configureLabel?: string;
    configureExternal?: boolean;
    onremove?: (() => void) | null;
    actions?: Snippet | null;
    details?: Snippet | null;
  } = $props();

  function providerLogo(slug: string) {
    return slug === "dokploy" ? "/integrations/dokploy.svg" : null;
  }
</script>

{#snippet content()}
  <header class="flex items-start gap-4">
    <span
      class="grid size-12 shrink-0 place-items-center rounded-xl border bg-zinc-950 shadow-sm"
      aria-hidden="true"
    >
      {#if icon}
        {@render icon()}
      {:else if provider === "github"}
        <SiGithub size={24} />
      {:else if provider === "gitlab"}
        <SiGitlab size={24} />
      {:else if provider === "gitea"}
        <SiGitea size={24} />
      {:else if provider === "forgejo"}
        <SiForgejo size={24} />
      {:else if provider === "public"}
        <Globe2 class="size-6 text-primary" />
      {:else if providerLogo(provider)}
        <img
          class="size-7 object-contain"
          src={providerLogo(provider) ?? ""}
          alt=""
        />
      {:else}
        <Rocket class="size-6 text-primary" />
      {/if}
    </span>
    <span class="min-w-0 flex-1">
      <span class="block truncate font-semibold">{name}</span>
      <span class="mt-0.5 block truncate text-xs text-muted-foreground">
        {providerName} · {subtitle}
      </span>
    </span>
    {#if onenabledchange}
      <Switch
        checked={enabled}
        disabled={busy}
        aria-label={`${enabled ? "Disable" : "Enable"} ${name}`}
        onCheckedChange={onenabledchange}
      />
    {:else if selected}
      <span
        class="grid size-7 shrink-0 place-items-center rounded-full bg-primary text-primary-foreground"
      >
        <Check class="size-4" />
      </span>
    {/if}
  </header>

  <p
    class={[
      "text-sm leading-6 text-muted-foreground",
      compact ? "mt-4 line-clamp-2" : "mt-5 line-clamp-3",
    ]}
  >
    {description}
  </p>
  <span class="mt-4 flex items-center gap-2 text-xs">
    <span
      class={[
        "rounded-full border px-2.5 py-1",
        statusHealthy
          ? "border-emerald-500/30 text-emerald-400"
          : "border-amber-500/30 text-amber-400",
      ]}
    >
      {statusLabel}
    </span>
    {#if detailLabel}
      <span class="truncate text-muted-foreground">{detailLabel}</span>
    {/if}
  </span>
  {#if error}
    <Alert.Root class="mt-3 p-2 text-xs" variant="destructive">
      <Alert.Title>Connection error</Alert.Title>
      <Alert.Description>{error}</Alert.Description>
    </Alert.Root>
  {/if}

  {#if details}
    {@render details()}
  {/if}

  {#if actions}
    <span class="mt-auto flex items-center gap-2 pt-5">
      {@render actions()}
    </span>
  {:else if onconfigure || onremove}
    <span class="mt-auto flex items-center gap-2 pt-5">
      {#if onconfigure}
        <Button
          variant="outline"
          size="sm"
          class="flex-1 gap-2"
          onclick={onconfigure}
        >
          {#if configureExternal}
            <ExternalLink class="size-3.5" />
          {:else}
            <Settings2 class="size-3.5" />
          {/if}
          {configureLabel}
        </Button>
      {:else}
        <span class="flex-1"></span>
      {/if}
      {#if onremove}
        <Button
          variant="ghost"
          size="icon-sm"
          class="text-muted-foreground hover:text-destructive"
          disabled={busy}
          aria-label={`Remove ${name}`}
          onclick={onremove}
        >
          {#if busy}
            <Spinner class="size-4" />
          {:else}
            <Trash2 class="size-4" />
          {/if}
        </Button>
      {/if}
    </span>
  {/if}
{/snippet}

{#if onselect}
  <button
    type="button"
    class={[
      "group flex w-full flex-col rounded-xl border bg-card/40 p-5 text-left shadow-sm transition-colors hover:border-foreground/25 hover:bg-card/60 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
      compact ? "min-h-52" : "min-h-72",
      selected && "border-primary/60 bg-primary/5",
    ]}
    onclick={onselect}
  >
    {@render content()}
  </button>
{:else}
  <article
    class={[
      "flex flex-col rounded-xl border bg-card/40 p-5 shadow-sm transition-colors hover:bg-card/60",
      compact ? "min-h-52" : "min-h-72",
    ]}
  >
    {@render content()}
  </article>
{/if}
