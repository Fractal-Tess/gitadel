<!--
  What sits above a namespace's repository list: its profile README, pinned
  repositories, and a year of commit activity. With a day picked from the
  graph, the README and pins give way to that day's commits. Each part stays out of the way
  until it has something to show.
-->
<script lang="ts">
  import { resolve } from "$app/paths";
  import ArrowLeft from "@lucide/svelte/icons/arrow-left";

  import {
    namespaceActivitySchema,
    pinnedRepositoriesSchema,
    type NamespaceActivity,
    type PinnedRepositories,
  } from "$lib/api/profile.js";
  import { requestJson } from "$lib/api/transport.js";
  import ActivityDay from "$lib/components/profile/activity-day.svelte";
  import ContributionGraph from "$lib/components/profile/contribution-graph.svelte";
  import PinnedRepositoriesSection from "$lib/components/profile/pinned-repositories.svelte";
  import ProfileReadme from "$lib/components/profile/profile-readme.svelte";
  import { Skeleton } from "$lib/components/ui/skeleton/index.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  let {
    namespace,
    canManage,
    date = null,
  }: {
    namespace: string;
    canManage: boolean;
    /** A day picked from the graph, shown in place of the README and pins. */
    date?: string | null;
  } = $props();

  const profileHref = $derived(resolve("/[namespace]", { namespace }));
  const dayHref = (day: string) => `${profileHref}?date=${day}`;

  const app = useAppState();
  let activity = $state<NamespaceActivity | null>(null);
  let pins = $state<PinnedRepositories | null>(null);
  let activityFailed = $state(false);

  $effect(() => {
    void app.authorizationScope;
    const base = `/api/v1/namespaces/${encodeURIComponent(namespace)}`;
    const controller = new AbortController();
    const init = { signal: controller.signal };
    activity = null;
    pins = null;
    activityFailed = false;
    requestJson(`${base}/activity`, namespaceActivitySchema, init).then(
      (loaded) => (activity = loaded),
      () => {
        if (!controller.signal.aborted) activityFailed = true;
      },
    );
    requestJson(`${base}/pins`, pinnedRepositoriesSchema, init).then(
      (loaded) => (pins = loaded),
      () => {},
    );
    return () => controller.abort();
  });
</script>

<div
  class="mx-auto grid max-w-5xl grid-cols-[minmax(0,1fr)] gap-6 px-5 pt-8 lg:px-8"
>
  {#if date}
    <a
      class="flex w-fit items-center gap-1.5 text-sm text-muted-foreground hover:text-foreground"
      href={profileHref}
    >
      <ArrowLeft class="size-4" />Back to {namespace}
    </a>
  {:else}
    <ProfileReadme {namespace} />
  {/if}

  {#if pins && !date}
    <PinnedRepositoriesSection
      {namespace}
      {pins}
      {canManage}
      onchange={(updated) => (pins = updated)}
    />
  {/if}

  {#if !activityFailed}
    <section
      class="motion-rise rounded-lg border bg-card p-4 sm:p-5"
      style:--stagger={2}
    >
      {#if activity}
        <h2 class="mb-4 text-sm font-semibold">
          {#if activity.total_commits === 0}
            No commits in the last year
          {:else}
            {activity.total_commits.toLocaleString()} commit{activity.total_commits ===
            1
              ? ""
              : "s"} in the last year
          {/if}
          <span class="font-normal text-muted-foreground">
            · {activity.repository_count} repositor{activity.repository_count ===
            1
              ? "y"
              : "ies"}
          </span>
        </h2>
        <ContributionGraph {activity} selected={date} href={dayHref} />
      {:else}
        <Skeleton class="mb-4 h-4 w-56" />
        <Skeleton class="h-28 w-full" />
      {/if}
    </section>
  {/if}

  {#if date}
    <!-- Keyed so each newly picked day settles in afresh. -->
    {#key date}
      <ActivityDay {namespace} {date} />
    {/key}
  {/if}
</div>
