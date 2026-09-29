<!--
  The README of the repository named after the namespace, shown at the top of
  its page, as on GitHub. Nothing renders when there is no such repository, it
  is empty, or the viewer cannot read it.
-->
<script lang="ts">
  import Braces from "@lucide/svelte/icons/braces";

  import {
    blobSchema,
    repositorySchema,
    treeSchema,
  } from "$lib/api/repositories.js";
  import { requestJson } from "$lib/api/transport.js";
  import { trustedHtml } from "$lib/repository/format.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  let { namespace }: { namespace: string } = $props();

  const app = useAppState();
  const api = $derived(
    `/api/v1/repositories/${encodeURIComponent(namespace)}/${encodeURIComponent(namespace)}`,
  );

  type Readme = {
    path: string;
    revision: string;
    commit_oid: string;
    html: string;
  };
  let readme = $state<Readme | null>(null);

  async function load(signal: AbortSignal): Promise<Readme | null> {
    const init = { signal };
    const repository = await requestJson(api, repositorySchema, init);
    if (!repository.default_branch) return null;
    const query = new URLSearchParams({ rev: repository.default_branch });
    const tree = await requestJson(`${api}/tree?${query}`, treeSchema, init);
    const entry = tree.entries.find(
      (candidate) =>
        candidate.kind === "blob" && /^readme(?:\.[^.]+)?$/i.test(candidate.name),
    );
    if (!entry) return null;
    const blob = await requestJson(
      `${api}/blob?${new URLSearchParams({ rev: tree.commit_oid, path: entry.path })}`,
      blobSchema,
      init,
    );
    if (!blob.rendered_html) return null;
    return {
      path: blob.path,
      revision: repository.default_branch,
      commit_oid: blob.commit_oid,
      html: blob.rendered_html,
    };
  }

  $effect(() => {
    void app.authorizationScope;
    void namespace;
    const controller = new AbortController();
    readme = null;
    load(controller.signal).then(
      (loaded) => {
        if (!controller.signal.aborted) readme = loaded;
      },
      // A missing, empty, or unreadable profile repository is the usual case.
      () => {},
    );
    return () => controller.abort();
  });
</script>

{#if readme}
  <section class="overflow-hidden rounded-lg border bg-card">
    <header
      class="flex min-h-11 items-center gap-2 border-b px-4 py-2 text-sm font-semibold"
    >
      <Braces class="size-4 text-muted-foreground" />
      <span class="text-muted-foreground">{namespace} /</span>
      {readme.path}
    </header>
    <div
      class="prose max-w-none p-6 prose-img:my-0 prose-img:inline-block prose-code:before:content-none prose-code:after:content-none dark:prose-invert"
      {@attach trustedHtml(readme.html, {
        namespace,
        name: namespace,
        revision: readme.revision,
        commit_oid: readme.commit_oid,
        path: readme.path,
      })}
    ></div>
  </section>
{/if}
