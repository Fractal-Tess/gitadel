<script lang="ts">
  import { BarChart3, Check, Copy } from "lucide-svelte";

  import { Button } from "$lib/components/ui/button/index.js";
  import { languageColor } from "$lib/repository/language-colors.js";
  import type {
    CopyTarget,
    RepositoryPageState,
  } from "$lib/repository/repository-page-state.svelte.js";

  let { state }: { state: RepositoryPageState } = $props();

  const cloneKinds: Array<{ id: CopyTarget; label: string }> = [
    { id: "http", label: "HTTP" },
    { id: "ssh", label: "SSH" },
  ];

  const compactUnits = [
    { threshold: 1_000, suffix: "k" },
    { threshold: 1_000_000, suffix: "m" },
    { threshold: 1_000_000_000, suffix: "b" },
  ] as const;
  const compactDecimal = new Intl.NumberFormat("en", {
    maximumFractionDigits: 1,
  });

  function compactCount(value: number): string {
    let unitIndex = -1;
    for (let index = 0; index < compactUnits.length; index += 1) {
      if (value < compactUnits[index].threshold) break;
      unitIndex = index;
    }
    if (unitIndex < 0) return value.toLocaleString("en");

    let unit = compactUnits[unitIndex];
    let rounded = Math.ceil((value / unit.threshold) * 10) / 10;
    if (rounded >= 1_000 && unitIndex < compactUnits.length - 1) {
      unit = compactUnits[unitIndex + 1];
      rounded = Math.ceil((value / unit.threshold) * 10) / 10;
    }
    return `${compactDecimal.format(rounded)}${unit.suffix}`;
  }
</script>

<aside class="order-first flex min-w-0 flex-col xl:order-none">
  <section
    class="order-last border-t p-4 xl:order-first xl:border-t-0 xl:border-b"
  >
    <h2
      class="text-xs font-semibold uppercase tracking-wider text-muted-foreground"
    >
      Clone
    </h2>
    <div class="mt-3 space-y-2">
      {#each cloneKinds as kind (kind.id)}
        <div>
          <p class="mb-1 text-[10px] font-medium text-muted-foreground">
            {kind.label}
          </p>
          <div
            class="flex items-center overflow-hidden rounded-md border bg-background"
          >
            <code
              class="min-w-0 flex-1 truncate px-3 py-2 text-xs text-muted-foreground"
            >
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
    <h2
      class="text-xs font-semibold uppercase tracking-wider text-muted-foreground"
    >
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
      <span
        class="text-[11px] tabular-nums text-muted-foreground"
        title={`${state.totalLines.toLocaleString()} non-blank lines`}
      >
        {compactCount(state.totalLines)}
      </span>
    </div>
    {#if state.stats.length}
      <div class="mt-4 flex h-1.5 overflow-hidden rounded-full bg-muted">
        {#each state.stats as item (item.language)}
          <span
            style:width={`${state.totalLines ? ((item.code + item.comments) / state.totalLines) * 100 : 0}%`}
            style:background={languageColor(item.language)}
          ></span>
        {/each}
      </div>
      <ul class="mt-4 space-y-3">
        {#each state.stats as item (item.language)}
          <li>
            <div class="flex items-center justify-between gap-3 text-xs">
              <span class="flex min-w-0 items-center gap-2 font-medium">
                <span
                  class="size-2 shrink-0 rounded-full"
                  style:background={languageColor(item.language)}
                ></span>
                <span class="truncate">{item.language}</span>
              </span>
              <span
                class="shrink-0 tabular-nums"
                title={`${(item.code + item.comments).toLocaleString()} non-blank lines`}
              >
                {compactCount(item.code + item.comments)}
              </span>
            </div>
            <div
              class="mt-1 flex flex-wrap gap-x-2 pl-4 text-[10px] text-muted-foreground"
            >
              <span>{item.files} file{item.files === 1 ? "" : "s"}</span>
              <span>{compactCount(item.code)} code</span>
              <span>{compactCount(item.comments)} comments</span>
            </div>
          </li>
        {/each}
      </ul>
    {:else}
      <p class="mt-3 text-xs text-muted-foreground">
        No recognized source files.
      </p>
    {/if}
  </section>
</aside>
