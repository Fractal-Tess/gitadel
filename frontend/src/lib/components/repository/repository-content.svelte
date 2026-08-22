<script lang="ts">
  import { Braces, Download } from "lucide-svelte";
  import MaterialFileIcon from "$lib/components/repository/material-file-icon.svelte";

  import { Button } from "$lib/components/ui/button/index.js";
  import { trustedHtml } from "$lib/repository/format.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let { state }: { state: RepositoryPageState } = $props();
</script>

<section class="min-w-0 border-b xl:border-b-0 xl:border-r">
  {#if state.blob}
    <header
      class="flex flex-wrap items-center justify-between gap-3 border-b px-5 py-3 text-sm font-semibold"
    >
      <span class="flex min-w-0 items-center gap-2">
        <MaterialFileIcon name={state.blob.path} class="size-4 shrink-0" />
        <span class="truncate">{state.blob.path}</span>
        <span
          class="shrink-0 rounded border bg-muted/45 px-1.5 py-0.5 font-mono text-[10px] font-normal text-muted-foreground"
        >
          {state.selectedLanguage}
        </span>
        <span class="shrink-0 text-xs font-normal text-muted-foreground">
          {state.blob.size.toLocaleString()} B
        </span>
      </span>
      <div class="flex items-center gap-1">
        {#if !state.blob.binary && !state.blob.too_large}
          <Button
            variant="ghost"
            size="sm"
            class={state.wrapLines
              ? "gap-1.5 bg-accent text-foreground"
              : "gap-1.5 text-muted-foreground"}
            onclick={() => (state.wrapLines = !state.wrapLines)}>Wrap</Button
          >
        {/if}
        <Button
          variant="ghost"
          size="sm"
          class="gap-1.5 text-muted-foreground"
          onclick={() => window.open(state.rawUrl, "_blank", "noopener")}
        >
          <Download class="size-3.5" />Raw
        </Button>
      </div>
    </header>

    {#if state.blob.binary}
      <div
        class="grid min-h-80 place-items-center p-8 text-sm text-muted-foreground"
      >
        Binary files cannot be previewed.
      </div>
    {:else if state.blob.too_large}
      <div
        class="grid min-h-80 place-items-center p-8 text-center text-sm text-muted-foreground"
      >
        <div>
          <p>This file is too large to render inline.</p>
          <Button
            class="mt-4 gap-2"
            variant="outline"
            onclick={() => window.open(state.rawUrl, "_blank", "noopener")}
          >
            <Download class="size-4" />Download raw file
          </Button>
        </div>
      </div>
    {:else if state.blob.rendered_html}
      <div
        class="prose prose-invert max-w-none p-6 prose-img:my-0 prose-img:inline-block prose-code:before:content-none prose-code:after:content-none lg:p-8"
        {@attach trustedHtml(state.blob.rendered_html, {
          namespace: state.namespace,
          name: state.name,
          revision: state.blob.revision,
          path: state.blob.path,
        })}
      ></div>
    {:else}
      <pre
        class={state.wrapLines
          ? "overflow-auto whitespace-pre-wrap break-words bg-background/35 p-5 font-mono text-xs leading-5"
          : "overflow-auto whitespace-pre bg-background/35 p-5 font-mono text-xs leading-5"}><code
          {@attach trustedHtml(state.highlighted)}></code></pre>
    {/if}
  {:else if state.readme?.rendered_html}
    <header
      class="flex items-center gap-2 border-b px-5 py-3 text-sm font-semibold"
    >
      <Braces class="size-4 text-muted-foreground" />{state.readme.path}
    </header>
    <div
      class="prose prose-invert max-w-none p-6 prose-img:my-0 prose-img:inline-block prose-code:before:content-none prose-code:after:content-none lg:p-8"
      {@attach trustedHtml(state.readme.rendered_html, {
        namespace: state.namespace,
        name: state.name,
        revision: state.readme.revision,
        path: state.readme.path,
      })}
    ></div>
  {:else}
    <div
      class="grid min-h-64 place-items-center p-8 text-sm text-muted-foreground"
    >
      Select a file to preview it.
    </div>
  {/if}
</section>
