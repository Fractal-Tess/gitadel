<!--
  One day of a namespace's activity: every commit the graph counted for it,
  grouped by repository. Each commit opens in its repository.
-->
<script lang="ts">
  import { resolve } from "$app/paths";
  import GitCommitHorizontal from "@lucide/svelte/icons/git-commit-horizontal";

  import { namespaceDaySchema, type NamespaceDay } from "$lib/api/profile.js";
  import { requestJson } from "$lib/api/transport.js";
  import RepositoryIcon from "$lib/components/repository/repository-icon.svelte";
  import { Skeleton } from "$lib/components/ui/skeleton/index.js";
  import { formatDate } from "$lib/repository/format.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  let { namespace, date }: { namespace: string; date: string } = $props();

  const app = useAppState();
  let day = $state<NamespaceDay | null>(null);
  let failed = $state(false);

  const heading = $derived(
    new Intl.DateTimeFormat(undefined, {
      dateStyle: "full",
      timeZone: "UTC",
    }).format(new Date(`${date}T00:00:00Z`)),
  );
  const timeFormat = new Intl.DateTimeFormat(undefined, { timeStyle: "short" });

  $effect(() => {
    void app.authorizationScope;
    const controller = new AbortController();
    day = null;
    failed = false;
    requestJson(
      `/api/v1/namespaces/${encodeURIComponent(namespace)}/activity/${encodeURIComponent(date)}`,
      namespaceDaySchema,
      { signal: controller.signal },
    ).then(
      (loaded) => (day = loaded),
      () => {
        if (!controller.signal.aborted) failed = true;
      },
    );
    return () => controller.abort();
  });

  function commitHref(
    repository: { namespace: string; name: string },
    oid: string,
  ): string {
    const base = resolve("/[namespace]/[name]", {
      namespace: repository.namespace,
      name: repository.name,
    });
    return `${base}?view=commit&oid=${encodeURIComponent(oid)}`;
  }
</script>

<section
  class="motion-rise grid gap-4"
  style:--stagger={1}
  aria-labelledby="activity-day-heading"
>
  <h2 id="activity-day-heading" class="text-sm font-semibold">
    {heading}
    {#if day}
      <span class="font-normal text-muted-foreground">
        · {day.total_commits.toLocaleString()} commit{day.total_commits === 1
          ? ""
          : "s"} in {day.repositories.length} repositor{day.repositories
          .length === 1
          ? "y"
          : "ies"}
      </span>
    {/if}
  </h2>

  {#if failed}
    <p class="rounded-lg border border-dashed p-4 text-sm text-destructive">
      Could not load the commits for this day.
    </p>
  {:else if !day}
    <Skeleton class="h-40 w-full" />
  {:else if day.repositories.length === 0}
    <p class="rounded-lg border border-dashed p-4 text-sm text-muted-foreground">
      No commits on this day.
    </p>
  {:else}
    {#each day.repositories as repository, index (`${repository.namespace}/${repository.name}`)}
      <div
        class="motion-rise overflow-hidden rounded-lg border bg-card"
        style:--stagger={index}
      >
        <a
          class="flex items-center gap-2.5 border-b px-4 py-3 text-sm font-semibold hover:bg-accent/55"
          href={resolve("/[namespace]/[name]", {
            namespace: repository.namespace,
            name: repository.name,
          })}
        >
          <RepositoryIcon
            namespace={repository.namespace}
            name={repository.name}
            class="size-5"
          />
          <!-- A person's work can live in other namespaces, so those are
               named in full. -->
          <span class="min-w-0 flex-1 truncate">
            {#if repository.namespace !== namespace}<span
                class="font-normal text-muted-foreground"
                >{repository.namespace}/</span
              >{/if}{repository.name}
          </span>
          <span class="shrink-0 text-xs font-normal text-muted-foreground">
            {repository.commits.length} commit{repository.commits.length === 1
              ? ""
              : "s"}
          </span>
        </a>
        <ul class="divide-y">
          {#each repository.commits as commit (commit.oid)}
            <li>
              <a
                class="flex items-center gap-3 px-4 py-2.5 text-sm hover:bg-accent/55"
                href={commitHref(repository, commit.oid)}
              >
                <GitCommitHorizontal
                  class="size-4 shrink-0 text-muted-foreground"
                />
                <span class="min-w-0 flex-1">
                  <span class="block truncate">
                    {commit.title || "(no message)"}
                  </span>
                  <span class="block truncate text-xs text-muted-foreground">
                    {commit.author_name}
                  </span>
                </span>
                <code class="hidden shrink-0 text-xs text-muted-foreground sm:block"
                  >{commit.short_oid.slice(0, 8)}</code
                >
                <time
                  class="shrink-0 text-xs text-muted-foreground tabular-nums"
                  datetime={new Date(commit.timestamp * 1000).toISOString()}
                  title={formatDate(commit.timestamp)}
                >
                  {timeFormat.format(new Date(commit.timestamp * 1000))}
                </time>
              </a>
            </li>
          {/each}
        </ul>
      </div>
    {/each}
  {/if}
</section>
