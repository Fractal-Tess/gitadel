<script lang="ts">
  import { onMount } from "svelte";
  import { resolve } from "$app/paths";
  import { ScrollText } from "lucide-svelte";

  import BrandMark from "$lib/components/brand-mark.svelte";
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

<div class="min-h-screen bg-background">
  <header class="border-b bg-background/95">
    <div
      class="mx-auto flex h-16 max-w-4xl items-center justify-between gap-4 px-5"
    >
      <a
        class="flex items-center gap-2 text-sm font-bold tracking-[-0.035em]"
        href={resolve("/")}
      >
        <BrandMark />
        {app.instance?.site_name ?? "GITADEL"}
      </a>
      {#if version}
        <span class="font-mono text-xs text-muted-foreground">v{version}</span>
      {/if}
    </div>
  </header>

  <main class="mx-auto max-w-4xl px-5 py-10">
    <div class="mb-7 flex items-start gap-3">
      <ScrollText class="mt-1 size-5 text-foreground/70" />
      <div>
        <h1 class="text-2xl font-semibold tracking-tight">Changelog</h1>
        <p class="mt-1.5 text-sm text-muted-foreground">
          Every notable change in Gitadel, as shipped by the version this
          instance is running.
        </p>
      </div>
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
  </main>
</div>
