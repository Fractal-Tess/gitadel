<script lang="ts">
  import { onMount } from "svelte";
  import { ScrollText } from "lucide-svelte";

  import { ApiFailure, changelogSchema, requestJson } from "$lib/api.js";
  import { trustedHtml } from "$lib/repository/format.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const app = useAppState();

  let loading = $state(true);
  let version = $state("");
  let html = $state("");
  let error = $state<string | null>(null);

  onMount(() => {
    void (async () => {
      try {
        const changelog = await requestJson(
          "/api/v1/changelog",
          changelogSchema,
        );
        version = changelog.application_version;
        html = changelog.rendered_html;
      } catch (caught) {
        error =
          caught instanceof ApiFailure || caught instanceof Error
            ? caught.message
            : "Could not load the changelog.";
      } finally {
        loading = false;
      }
    })();
  });
</script>

<svelte:head>
  <title>Changelog · {app.instance?.site_name ?? "Gitadel"}</title>
</svelte:head>

<div class="mx-auto max-w-5xl px-5 py-8 lg:px-8">
  <div class="mb-7 flex items-start justify-between gap-4">
    <div class="flex items-start gap-3">
      <ScrollText class="mt-0.5 size-5 text-foreground/70" />
      <div>
        <h1 class="text-lg font-semibold tracking-tight">Changelog</h1>
        <p class="mt-1.5 text-sm text-muted-foreground">
          Every notable change in Gitadel, as shipped by the version this
          instance is running.
        </p>
      </div>
    </div>
    {#if version}
      <span class="shrink-0 font-mono text-xs text-muted-foreground">
        v{version}
      </span>
    {/if}
  </div>

  {#if loading}
    <p class="py-16 text-center text-sm text-muted-foreground">
      Loading changelog…
    </p>
  {:else if error}
    <p
      class="rounded-md border border-destructive/30 bg-destructive/5 p-4 text-sm"
    >
      {error}
    </p>
  {:else}
    <div
      class="prose prose-invert max-w-none prose-code:before:content-none prose-code:after:content-none"
      {@attach trustedHtml(html)}
    ></div>
  {/if}
</div>
