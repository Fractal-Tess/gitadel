<script lang="ts">
  import ArrowLeft from "@lucide/svelte/icons/arrow-left";
  import Check from "@lucide/svelte/icons/check";
  import Copy from "@lucide/svelte/icons/copy";
  import History from "@lucide/svelte/icons/history";
  import Rocket from "@lucide/svelte/icons/rocket";
  import ShieldAlert from "@lucide/svelte/icons/shield-alert";
  import ShieldCheck from "@lucide/svelte/icons/shield-check";
  import Tag from "@lucide/svelte/icons/tag";
  import { toast } from "svelte-sonner";

  import type { CommitRef } from "$lib/api/repositories.js";
  import { copyText } from "$lib/clipboard.js";
  import ActionStatusBadge from "$lib/components/actions/action-status-badge.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import {
    dayHeading,
    dayKey,
    formatDate,
    formatDay,
  } from "$lib/repository/format.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let { state: repository }: { state: RepositoryPageState } = $props();

  let copiedOid: string | null = $state(null);

  async function copyCommitId(oid: string) {
    try {
      await copyText(oid);
      copiedOid = oid;
      toast.success("Commit ID copied");
      window.setTimeout(() => {
        if (copiedOid === oid) copiedOid = null;
      }, 1600);
    } catch {
      toast.error("Could not copy commit ID");
    }
  }

  function release(refs: CommitRef[]): CommitRef | null {
    return refs.find((reference) => reference.kind === "release") ?? null;
  }

  function releaseSummary(reference: CommitRef): string {
    const published = reference.published_at
      ? `released ${formatDay(reference.published_at)}`
      : "released";
    const since =
      reference.commits_since_previous === null
        ? null
        : `${reference.commits_since_previous} ${reference.commits_since_previous === 1 ? "commit" : "commits"} since the previous release`;
    return since ? `${published} · ${since}` : published;
  }
</script>

<section>
  <button
    class="mb-7 inline-flex items-center gap-2 text-sm text-muted-foreground hover:text-foreground"
    onclick={() => repository.navigate("overview")}
  >
    <ArrowLeft class="size-4" />Back to repository
  </button>
  <header class="mb-6 flex items-center gap-2">
    <History class="size-4 text-muted-foreground" />
    <h2 class="text-sm font-semibold">Commit history</h2>
    <span class="text-xs text-muted-foreground">
      Page {repository.browser.history?.page ?? repository.historyPage} on {repository.revision}
    </span>
  </header>
  <ol>
    {#each repository.browser.history?.commits ?? [] as item, index (item.oid)}
      {#if index === 0 || dayKey(item.committer.timestamp) !== dayKey(repository.browser.history?.commits[index - 1].committer.timestamp ?? 0)}
        <li
          class="mb-3 mt-7 text-xs font-medium tracking-[0.12em] text-muted-foreground first:mt-0"
        >
          {dayHeading(item.committer.timestamp)}
        </li>
      {/if}
      {@const released = release(item.refs)}
      {@const check = repository.actions.actionCommitStatuses[item.oid]}
      {#if released}
        <li
          class="flex items-center gap-3 border-l border-border py-4 pl-5 text-xs"
        >
          <Rocket class="size-3.5 shrink-0 text-primary" />
          <span class="font-medium">{released.name}</span>
          <span class="text-muted-foreground">{releaseSummary(released)}</span>
          <span class="h-px flex-1 bg-border"></span>
        </li>
      {/if}
      <li class="relative border-l border-border pl-5">
        {#if released}
          <span
            class="absolute -left-[3.5px] top-[27px] size-[7px] rounded-full bg-primary"
            aria-hidden="true"
          ></span>
        {/if}
        <div
          class="grid w-full gap-4 py-4 sm:grid-cols-[minmax(0,1fr)_auto] sm:items-center"
        >
          <button
            class="group min-w-0 text-left"
            onclick={() => repository.navigate("commit", { oid: item.oid })}
          >
            <p class="truncate text-sm font-medium group-hover:underline">
              {item.title || "Untitled commit"}
            </p>
            <p class="mt-1.5 text-xs text-muted-foreground">
              <span class="font-medium text-foreground/80"
                >{item.author.name}</span
              >
              · {formatDate(item.committer.timestamp)}
            </p>
          </button>
          <div class="flex flex-wrap items-center gap-2.5 sm:justify-end">
            {#if item.verification}
              <span
                class={item.verification.verified
                  ? "inline-flex items-center gap-1 rounded-full border border-emerald-500/30 bg-emerald-500/10 px-2 py-1 text-[0.6875rem] font-medium text-emerald-700 dark:text-emerald-300"
                  : "inline-flex items-center gap-1 rounded-full border bg-muted/40 px-2 py-1 text-[0.6875rem] font-medium text-muted-foreground"}
                title={item.verification.fingerprint ?? undefined}
              >
                {#if item.verification.verified}
                  <ShieldCheck class="size-3" />Verified
                {:else}
                  <ShieldAlert class="size-3" />Unverified
                {/if}
              </span>
            {/if}
            {#each item.refs as reference (reference.kind + reference.name)}
              {#if reference.kind === "release"}
                <button
                  type="button"
                  class="inline-flex items-center gap-1.5 rounded-full border border-primary/30 bg-primary/10 px-2.5 py-1 text-xs font-medium text-primary hover:bg-primary/20"
                  onclick={() => repository.navigate("releases")}
                >
                  <Rocket class="size-3" />
                  {reference.name}
                  {#if reference.latest}
                    <span class="text-[10px] tracking-wide uppercase"
                      >Latest</span
                    >
                  {:else if reference.prerelease}
                    <span class="text-[10px] tracking-wide uppercase">Pre</span>
                  {/if}
                </button>
              {:else}
                <button
                  type="button"
                  class="inline-flex items-center gap-1.5 rounded-full border px-2.5 py-1 font-mono text-xs text-muted-foreground hover:bg-muted/40 hover:text-foreground"
                  onclick={() => repository.changeRevision(reference.name)}
                >
                  <Tag class="size-3" />
                  {reference.name}
                </button>
              {/if}
            {/each}
            {#if check && check.total > 0}
              <button
                type="button"
                onclick={() =>
                  repository.navigate("actions", { commit: item.oid })}
              >
                <ActionStatusBadge status={check.status} />
              </button>
            {/if}
            {#if item.insertions > 0 || item.deletions > 0}
              <span class="text-xs font-medium whitespace-nowrap tabular-nums">
                <span class="text-emerald-500">+{item.insertions}</span>
                <span class="ml-1.5 text-red-500">−{item.deletions}</span>
              </span>
            {/if}
            <span
              class="inline-flex items-stretch overflow-hidden rounded-md border bg-card"
            >
              <code class="px-2.5 py-1.5 text-xs text-muted-foreground">
                {item.short_oid}
              </code>
              <button
                type="button"
                class="grid w-7 shrink-0 place-items-center border-l text-muted-foreground hover:bg-muted/40 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                aria-label="Copy commit ID"
                onclick={() => void copyCommitId(item.oid)}
              >
                {#if copiedOid === item.oid}
                  <Check class="size-3 text-emerald-500" />
                {:else}
                  <Copy class="size-3" />
                {/if}
              </button>
            </span>
          </div>
        </div>
      </li>
    {:else}
      <li class="py-16 text-center text-sm text-muted-foreground">
        No commits found.
      </li>
    {/each}
  </ol>

  {#if repository.browser.history && (repository.browser.history.page > 1 || repository.browser.history.has_next)}
    <footer class="mt-8 flex justify-between border-t pt-4">
      <Button
        variant="outline"
        size="sm"
        disabled={repository.browser.history.page <= 1}
        onclick={() =>
          repository.navigate("history", { page: repository.browser.history!.page - 1 })}
      >
        Previous
      </Button>
      <Button
        variant="outline"
        size="sm"
        disabled={!repository.browser.history.has_next}
        onclick={() =>
          repository.navigate("history", { page: repository.browser.history!.page + 1 })}
      >
        Next
      </Button>
    </footer>
  {/if}
</section>
