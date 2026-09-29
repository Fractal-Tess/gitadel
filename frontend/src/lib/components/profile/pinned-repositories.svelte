<!--
  Up to six repositories a namespace features at the top of its page. Owners
  get a button to choose and order them.
-->
<script lang="ts">
  import { resolve } from "$app/paths";
  import LockKeyhole from "@lucide/svelte/icons/lock-keyhole";
  import Pin from "@lucide/svelte/icons/pin";

  import type { PinnedRepositories } from "$lib/api/profile.js";
  import PinEditorDialog from "$lib/components/profile/pin-editor-dialog.svelte";
  import RepositoryIcon from "$lib/components/repository/repository-icon.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { languageColor } from "$lib/repository/language-colors.js";

  let {
    namespace,
    pins,
    canManage,
    onchange,
  }: {
    namespace: string;
    pins: PinnedRepositories;
    canManage: boolean;
    onchange: (pins: PinnedRepositories) => void;
  } = $props();

  let editing = $state(false);
</script>

{#if pins.repositories.length > 0 || canManage}
  <section aria-labelledby="pinned-heading">
    <div class="mb-3 flex items-center justify-between gap-3">
      <h2
        id="pinned-heading"
        class="flex items-center gap-2 text-sm font-semibold"
      >
        <Pin class="size-4 text-muted-foreground" />Pinned
      </h2>
      {#if canManage}
        <Button variant="ghost" size="sm" onclick={() => (editing = true)}>
          Customize pins
        </Button>
      {/if}
    </div>

    {#if pins.repositories.length === 0}
      <p
        class="rounded-lg border border-dashed p-4 text-sm text-muted-foreground"
      >
        Pin up to {pins.limit} repositories to feature them here.
      </p>
    {:else}
      <ul class="grid gap-3 sm:grid-cols-2">
        {#each pins.repositories as repository (repository.id)}
          {@const language = repository.languages[0]}
          <li>
            <a
              class="flex h-full flex-col gap-2 rounded-lg border bg-card p-4 transition-colors hover:bg-accent/55"
              href={resolve("/[namespace]/[name]", {
                namespace: repository.namespace,
                name: repository.name,
              })}
            >
              <div class="flex items-center gap-2.5">
                <RepositoryIcon
                  namespace={repository.namespace}
                  name={repository.name}
                  iconUpdatedAt={repository.icon_updated_at}
                  class="size-6"
                />
                <h3 class="min-w-0 truncate text-sm font-semibold">
                  {repository.name}
                </h3>
                {#if repository.visibility === "private"}
                  <span
                    class="inline-flex shrink-0 items-center gap-1 rounded border px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground"
                    ><LockKeyhole class="size-2.5" /> Private</span
                  >
                {/if}
              </div>
              <p class="line-clamp-2 flex-1 text-sm text-muted-foreground">
                {repository.description ?? "No description provided."}
              </p>
              <div
                class="flex items-center gap-4 text-xs text-muted-foreground"
              >
                {#if language}
                  <span class="inline-flex items-center gap-1.5">
                    <span
                      class="size-2.5 rounded-full"
                      style:background={languageColor(language.language)}
                    ></span>
                    {language.language}
                  </span>
                {/if}
                <span class="tabular-nums">
                  {repository.commit_count.toLocaleString()} commit{repository.commit_count ===
                  1
                    ? ""
                    : "s"}
                </span>
              </div>
            </a>
          </li>
        {/each}
      </ul>
    {/if}
  </section>

  {#if canManage}
    <PinEditorDialog
      {namespace}
      limit={pins.limit}
      current={pins.repositories}
      bind:open={editing}
      onsaved={onchange}
    />
  {/if}
{/if}
