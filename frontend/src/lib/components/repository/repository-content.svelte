<script lang="ts">
  import { Braces, Download } from "lucide-svelte";
  import MaterialFileIcon from "$lib/components/repository/material-file-icon.svelte";

  import { Button } from "$lib/components/ui/button/index.js";
  import { trustedHtml } from "$lib/repository/format.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let { state }: { state: RepositoryPageState } = $props();
</script>

<!-- The metadata rail draws the divider on this column's right, so it only owns
     its own stacking border on narrow screens. -->
<section class="flex min-w-0 flex-col border-b xl:min-h-0 xl:border-b-0">
  {#if state.blob}
    <header
      class="flex min-h-12 shrink-0 flex-wrap items-center justify-between gap-3 border-b px-5 py-2 text-sm font-semibold"
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

    <!-- One scroller per column, and it owns both axes so a long line's
         horizontal bar sits at the bottom of the column rather than at the
         bottom of the file. -->
    <div
      class="min-h-0 flex-1 overflow-x-auto xl:overflow-auto xl:overscroll-contain"
    >
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
        <!-- `w-max` keeps the code background painted under long unwrapped
             lines now that the surrounding column does the scrolling. -->
        <pre
          class={state.wrapLines
            ? "whitespace-pre-wrap break-words bg-background/35 p-5 font-mono text-xs leading-5"
            : "w-max min-w-full whitespace-pre bg-background/35 p-5 font-mono text-xs leading-5"}><code
            {@attach trustedHtml(state.highlighted)}></code></pre>
      {/if}
    </div>
  {:else if state.readme?.rendered_html}
    <header
      class="flex min-h-12 shrink-0 items-center gap-2 border-b px-5 py-2 text-sm font-semibold"
    >
      <Braces class="size-4 text-muted-foreground" />{state.readme.path}
    </header>
    <div class="min-h-0 flex-1 xl:overflow-y-auto xl:overscroll-contain">
      <div
        class="prose prose-invert max-w-none p-6 prose-img:my-0 prose-img:inline-block prose-code:before:content-none prose-code:after:content-none lg:p-8"
        {@attach trustedHtml(state.readme.rendered_html, {
          namespace: state.namespace,
          name: state.name,
          revision: state.readme.revision,
          path: state.readme.path,
        })}
      ></div>
    </div>
  {:else}
    <div
      class="grid min-h-64 flex-1 place-items-center p-8 text-sm text-muted-foreground"
    >
      Select a file to preview it.
    </div>
  {/if}
</section>
