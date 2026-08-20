<script lang="ts">
  import { BarChart3, Check, Copy } from "lucide-svelte";

  import { Button } from "$lib/components/ui/button/index.js";
  import type {
    CopyTarget,
    RepositoryPageState,
  } from "$lib/repository/repository-page-state.svelte.js";

  let { state }: { state: RepositoryPageState } = $props();

  const cloneKinds: Array<{ id: CopyTarget; label: string }> = [
    { id: "http", label: "HTTP" },
    { id: "ssh", label: "SSH" },
  ];
</script>

<aside class="order-first flex min-w-0 flex-col xl:order-none">
  <section class="order-last border-t p-4 xl:order-first xl:border-t-0 xl:border-b">
    <h2 class="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
      Clone
    </h2>
    <div class="mt-3 space-y-2">
      {#each cloneKinds as kind (kind.id)}
        <div>
          <p class="mb-1 text-[10px] font-medium text-muted-foreground">{kind.label}</p>
          <div class="flex items-center overflow-hidden rounded-md border bg-background">
            <code class="min-w-0 flex-1 truncate px-3 py-2 text-xs text-muted-foreground">
              {kind.id === "http"
                ? state.httpCloneUrl
                : state.repository?.ssh_clone_url}
            </code>
            <Button
              variant="ghost"
              size="icon-sm"
              class="shrink-0 rounded-none border-l text-muted-foreground"
              onclick={() => void state.copyCloneUrl(kind.id)}
              aria-label={`Copy ${kind.label} clone URL`}
            >
              {#if state.copied === kind.id}
                <Check class="size-3.5 text-emerald-500" />
              {:else}
                <Copy class="size-3.5" />
              {/if}
            </Button>
          </div>
        </div>
      {/each}
    </div>
  </section>

  <section class="order-first p-4 xl:order-2">
    <h2 class="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
      About
    </h2>
    <p class="mt-3 text-sm leading-6">
      {state.repository?.description ?? "No description provided."}
    </p>
    <dl class="mt-5 space-y-3 border-t pt-4 text-xs">
      <div class="flex justify-between gap-4">
        <dt class="text-muted-foreground">Default branch</dt>
        <dd class="font-mono">{state.repository?.default_branch}</dd>
      </div>
      <div class="flex justify-between gap-4">
        <dt class="text-muted-foreground">Object format</dt>
        <dd>{state.repository?.object_format.toUpperCase()}</dd>
      </div>
      <div class="flex justify-between gap-4">
        <dt class="text-muted-foreground">Branches</dt>
        <dd>{state.refs?.branches.length ?? 0}</dd>
      </div>
      <div class="flex justify-between gap-4">
        <dt class="text-muted-foreground">Tags</dt>
        <dd>{state.refs?.tags.length ?? 0}</dd>
      </div>
    </dl>
  </section>

  <section class="order-2 border-t p-4 xl:order-last">
    <div class="flex items-center justify-between gap-3">
      <h2
        class="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider text-muted-foreground"
      >
        <BarChart3 class="size-3.5" />Statistics
      </h2>
      <span class="text-[11px] tabular-nums text-muted-foreground">
        {state.totalCode.toLocaleString()} LOC total
      </span>
    </div>
    {#if state.stats.length}
      <div class="mt-4 flex h-1.5 overflow-hidden rounded-full bg-muted">
        {#each state.stats as item, index (item.language)}
          <span
            style={`width:${state.totalCode ? (item.code / state.totalCode) * 100 : 0}%;background:hsl(${(index * 67 + 218) % 360} 62% 52%)`}
          ></span>
        {/each}
      </div>
      <ul class="mt-4 space-y-3">
        {#each state.stats as item, index (item.language)}
          <li>
            <div class="flex items-center justify-between gap-3 text-xs">
              <span class="flex min-w-0 items-center gap-2 font-medium">
                <span
                  class="size-2 shrink-0 rounded-full"
                  style={`background:hsl(${(index * 67 + 218) % 360} 62% 52%)`}
                ></span>
                <span class="truncate">{item.language}</span>
              </span>
              <span class="shrink-0 tabular-nums">{item.code.toLocaleString()} LOC</span>
            </div>
            <div
              class="mt-1 flex flex-wrap gap-x-2 pl-4 text-[10px] text-muted-foreground"
            >
              <span>{item.files} file{item.files === 1 ? "" : "s"}</span>
              <span>{item.comments.toLocaleString()} comments</span>
              <span>{item.blanks.toLocaleString()} blank</span>
            </div>
          </li>
        {/each}
      </ul>
    {:else}
      <p class="mt-3 text-xs text-muted-foreground">No recognized source files.</p>
    {/if}
  </section>
</aside>
