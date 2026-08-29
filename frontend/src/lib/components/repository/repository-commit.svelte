<script lang="ts">
  import ArrowLeft from "@lucide/svelte/icons/arrow-left";
  import GitBranch from "@lucide/svelte/icons/git-branch";
  import GitCommitHorizontal from "@lucide/svelte/icons/git-commit-horizontal";
  import Rocket from "@lucide/svelte/icons/rocket";
  import ShieldAlert from "@lucide/svelte/icons/shield-alert";
  import ShieldCheck from "@lucide/svelte/icons/shield-check";
  import Tag from "@lucide/svelte/icons/tag";

  import ActionStatusBadge from "$lib/components/actions/action-status-badge.svelte";
  import PierreDiff from "$lib/components/repository/pierre-diff.svelte";
  import { formatDate } from "$lib/repository/format.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let { state }: { state: RepositoryPageState } = $props();
  const check = $derived(
    state.browser.commit ? state.actions.actionCommitStatuses[state.browser.commit.oid] : undefined,
  );
</script>

{#if state.browser.commit}
  <div class="grid gap-7 lg:grid-cols-[minmax(0,1fr)_15rem]">
    <div class="min-w-0">
      <button
        class="mb-7 inline-flex items-center gap-2 text-sm text-muted-foreground hover:text-foreground"
        onclick={() => state.navigate("history")}
      >
        <ArrowLeft class="size-4" />Back to commits
      </button>
      <header class="border-b pb-5">
        <h2 class="text-2xl font-semibold tracking-tight">
          {state.browser.commit.title || "Untitled commit"}
        </h2>
        <div
          class="mt-3 flex flex-wrap items-center gap-2 text-sm text-muted-foreground"
        >
          <span class="font-medium text-foreground"
            >{state.browser.commit.author.name}</span
          >
          <span>·</span>
          <span>{formatDate(state.browser.commit.committer.timestamp)}</span>
          <span>·</span>
          <span class="inline-flex items-center gap-1.5"
            ><GitBranch class="size-3.5" />{state.revision}</span
          >
        </div>
        {#if state.browser.commit.verification}
          <span
            class={state.browser.commit.verification.verified
              ? "mt-3 inline-flex items-center gap-1.5 rounded-full border border-emerald-500/30 bg-emerald-500/10 px-2.5 py-1 text-xs font-medium text-emerald-700 dark:text-emerald-300"
              : "mt-3 inline-flex items-center gap-1.5 rounded-full border bg-muted/40 px-2.5 py-1 text-xs font-medium text-muted-foreground"}
            title={state.browser.commit.verification.fingerprint ?? undefined}
          >
            {#if state.browser.commit.verification.verified}
              <ShieldCheck class="size-3.5" />
              Verified by {state.browser.commit.verification.signer}
            {:else}
              <ShieldAlert class="size-3.5" />
              {state.browser.commit.verification.reason === "invalid"
                ? "Invalid SSH signature"
                : "Unverified SSH signature"}
            {/if}
          </span>
        {/if}
        {#if state.browser.commit.refs.length}
          <div class="mt-3 flex flex-wrap items-center gap-2">
            {#each state.browser.commit.refs as reference (reference.kind + reference.name)}
              {#if reference.kind === "release"}
                <button
                  type="button"
                  class="inline-flex items-center gap-1.5 rounded-full border border-primary/30 bg-primary/10 px-2.5 py-1 text-xs font-medium text-primary hover:bg-primary/20"
                  onclick={() => state.navigate("releases")}
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
                  onclick={() => state.changeRevision(reference.name)}
                >
                  <Tag class="size-3" />
                  {reference.name}
                </button>
              {/if}
            {/each}
          </div>
        {/if}
        {#if state.browser.commit.message !== state.browser.commit.title}
          <pre
            class="mt-5 whitespace-pre-wrap border-t pt-4 font-sans text-sm leading-6 text-foreground/80">{state
              .browser.commit.message}</pre>
        {/if}
      </header>

      <section class="mt-6">
        <header class="mb-3 flex items-center gap-2 text-sm font-semibold">
          <GitCommitHorizontal class="size-4 text-muted-foreground" />Changes
        </header>
        {#if state.browser.diff?.patch}
          {#key state.browser.commit.oid}
            <PierreDiff patch={state.browser.diff.patch} cacheKey={state.browser.commit.oid} />
          {/key}
        {:else}
          <div
            class="rounded-md border bg-card/25 p-10 text-center text-sm text-muted-foreground"
          >
            No textual changes.
          </div>
        {/if}
        {#if state.browser.diff?.truncated}
          <p
            class="mt-4 rounded-md border border-amber-500/20 bg-amber-500/10 px-4 py-3 text-xs text-amber-800 dark:text-amber-200"
          >
            This large diff was truncated; some files or changes are not shown.
          </p>
        {/if}
      </section>
    </div>

    <aside class="space-y-6 border-l pl-5">
      <div>
        <p
          class="text-xs font-semibold uppercase tracking-wider text-muted-foreground"
        >
          Commit
        </p>
        <code class="mt-3 block break-all text-xs"
          >{state.browser.commit.short_oid}</code
        >
      </div>
      <div>
        <p
          class="text-xs font-semibold uppercase tracking-wider text-muted-foreground"
        >
          Author
        </p>
        <div class="mt-3 rounded-md border bg-card/25 p-3">
          <p class="text-sm font-medium">{state.browser.commit.author.name}</p>
          <p class="mt-1 truncate text-xs text-muted-foreground">
            {state.browser.commit.author.email}
          </p>
        </div>
      </div>
      {#if check && check.total > 0}
        <div>
          <p
            class="text-xs font-semibold uppercase tracking-wider text-muted-foreground"
          >
            Checks
          </p>
          <button
            type="button"
            class="mt-3"
            onclick={() => state.navigate("actions", { commit: state.browser.commit!.oid })}
          >
            <ActionStatusBadge status={check.status} />
          </button>
        </div>
      {/if}
      <div>
        <p
          class="text-xs font-semibold uppercase tracking-wider text-muted-foreground"
        >
          Parents
        </p>
        <p class="mt-3 text-sm">{state.browser.commit.parents.length}</p>
      </div>
    </aside>
  </div>
{/if}
