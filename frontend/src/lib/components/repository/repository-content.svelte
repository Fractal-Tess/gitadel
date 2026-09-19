<script lang="ts">
  import Braces from "@lucide/svelte/icons/braces";
  import Download from "@lucide/svelte/icons/download";
  import Minus from "@lucide/svelte/icons/minus";
  import Plus from "@lucide/svelte/icons/plus";
  import RepositorySubmodule from "$lib/components/repository/repository-submodule.svelte";
  import RepositoryLfs from "$lib/components/repository/repository-lfs.svelte";
  import MaterialFileIcon from "$lib/components/repository/material-file-icon.svelte";

  import { Button } from "$lib/components/ui/button/index.js";
  import { repositoryImageUrl } from "$lib/api/repositories.js";
  import { trustedHtml } from "$lib/repository/format.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let { state: pageState }: { state: RepositoryPageState } = $props();
  let imageScale = $state(1);
  let imageSizing = $state<"fit" | "actual">("fit");
  let imageBackground = $state<"checker" | "light" | "dark">("checker");
  let imageView = $state<"preview" | "source">("preview");
  let imageLoadError = $state<string | null>(null);
  let sourceText = $state("");
  let sourceLoading = $state(false);
  let sourceError = $state<string | null>(null);
  let imageStateKey = "";
  let sourceController: AbortController | null = null;

  const image = $derived(
    pageState.browser.blob?.too_large
      ? null
      : (pageState.browser.blob?.image ?? null),
  );
  const blob = $derived(pageState.browser.blob);
  const imageUrl = $derived(
    blob && image
      ? repositoryImageUrl(
          pageState.namespace,
          pageState.name,
          blob.commit_oid,
          blob.path,
        )
      : "",
  );
  const imageKey = $derived(blob ? `${blob.commit_oid}:${blob.path}` : "");
  const isSvg = $derived(image?.mime_type.toLowerCase() === "image/svg+xml");
  const imageStyle = $derived.by(() => {
    if (!image) return "";
    if (imageSizing === "fit")
      return `max-width:100%;max-height:min(70svh,42rem);width:auto;height:auto;object-fit:contain;`;
    return `width:${Math.round(image.width * imageScale)}px;height:${Math.round(image.height * imageScale)}px;max-width:none;`;
  });
  const backgroundClass = $derived(
    imageBackground === "light"
      ? "bg-white"
      : imageBackground === "dark"
        ? "bg-neutral-950"
        : "[background:repeating-conic-gradient(#d4d4d8_0_25%,#fafafa_0_50%)_50%_/_1rem_1rem]",
  );

  // A keyed image and this guard ensure an error from an old branch/file
  // cannot replace the current preview during rapid navigation.
  $effect(() => {
    if (imageKey === imageStateKey) return;
    imageStateKey = imageKey;
    imageLoadError = null;
    imageView = "preview";
    imageSizing = "fit";
    imageScale = 1;
    sourceText = "";
    sourceLoading = false;
    sourceError = null;
    sourceController?.abort();
    sourceController = null;
  });

  function downloadOriginal() {
    if (pageState.rawUrl) window.open(pageState.rawUrl, "_blank", "noopener");
  }

  async function showSource() {
    if (!isSvg || !blob) return;
    imageView = "source";
    if (sourceText || blob.content) {
      sourceText = blob.content ?? sourceText;
      return;
    }
    const key = imageKey;
    sourceController?.abort();
    const controller = new AbortController();
    sourceController = controller;
    sourceLoading = true;
    sourceError = null;
    try {
      const response = await fetch(pageState.rawUrl, {
        signal: controller.signal,
        credentials: "same-origin",
      });
      if (!response.ok) throw new Error("The SVG source could not be loaded.");
      const text = await response.text();
      if (key === imageStateKey && sourceController === controller)
        sourceText = text;
    } catch (caught) {
      if (
        !(caught instanceof DOMException && caught.name === "AbortError") &&
        key === imageStateKey
      )
        sourceError =
          caught instanceof Error
            ? caught.message
            : "The SVG source could not be loaded.";
    } finally {
      if (sourceController === controller) {
        sourceController = null;
        sourceLoading = false;
      }
    }
  }

  function changeScale(delta: number) {
    imageSizing = "actual";
    imageScale = Math.min(
      4,
      Math.max(0.25, Math.round((imageScale + delta) * 4) / 4),
    );
  }
</script>

<!-- The metadata rail draws the divider on this column's right, so it only owns
     its own stacking border on narrow screens. -->
<section
  class="flex min-w-0 flex-col border-b xl:h-full xl:min-h-0 xl:border-b-0"
>
  {#if pageState.browser.submodule}
    <RepositorySubmodule state={pageState} />
  {:else if pageState.browser.blob?.lfs}
    {#key `${pageState.browser.blob.commit_oid}:${pageState.browser.blob.path}`}
      <RepositoryLfs state={pageState} />
    {/key}
  {:else if pageState.browser.blob}
    <header
      class="flex min-h-12 shrink-0 flex-wrap items-center justify-between gap-3 border-b px-5 py-2 text-sm font-semibold"
    >
      <span class="flex min-w-0 items-center gap-2">
        <MaterialFileIcon
          name={pageState.browser.blob.path}
          class="size-4 shrink-0"
        />
        <span class="truncate">{pageState.browser.blob.path}</span>
        {#if image}
          <span
            class="shrink-0 rounded border bg-muted/45 px-1.5 py-0.5 font-mono text-[10px] font-normal text-muted-foreground"
          >
            {image.mime_type}
          </span>
        {:else}
          <span
            class="shrink-0 rounded border bg-muted/45 px-1.5 py-0.5 font-mono text-[10px] font-normal text-muted-foreground"
          >
            {pageState.browser.selectedLanguage}
          </span>
        {/if}
        <span class="shrink-0 text-xs font-normal text-muted-foreground">
          {pageState.browser.blob.size.toLocaleString()} B
        </span>
      </span>
      <div class="flex items-center gap-1">
        {#if image}
          {#if isSvg}
            <Button
              variant="ghost"
              size="sm"
              class={imageView === "preview"
                ? "bg-accent"
                : "text-muted-foreground"}
              onclick={() => (imageView = "preview")}>Preview</Button
            >
            <Button
              variant="ghost"
              size="sm"
              class={imageView === "source"
                ? "bg-accent"
                : "text-muted-foreground"}
              onclick={() => void showSource()}>Source</Button
            >
          {/if}
        {:else if !pageState.browser.blob.binary && !pageState.browser.blob.too_large}
          <Button
            variant="ghost"
            size="sm"
            class={pageState.browser.wrapLines
              ? "gap-1.5 bg-accent text-foreground"
              : "gap-1.5 text-muted-foreground"}
            onclick={() =>
              (pageState.browser.wrapLines = !pageState.browser.wrapLines)}
            >Wrap</Button
          >
        {/if}
        <Button
          variant="ghost"
          size="sm"
          class="gap-1.5 text-muted-foreground"
          onclick={downloadOriginal}
        >
          <Download class="size-3.5" />Raw
        </Button>
      </div>
    </header>

    <div
      class="min-h-0 flex-1 overflow-x-auto xl:overflow-auto xl:overscroll-contain"
    >
      {#if image}
        <div class="grid min-h-full content-start gap-3 p-4">
          <div
            class="flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground"
          >
            <span
              >{image.width} × {image.height} · {image.mime_type} · {pageState.browser.blob.size.toLocaleString()}
              B</span
            >
            {#if imageView === "preview"}
              <div class="flex flex-wrap items-center gap-1">
                <Button
                  variant={imageSizing === "fit" ? "secondary" : "ghost"}
                  size="sm"
                  onclick={() => {
                    imageSizing = "fit";
                    imageScale = 1;
                  }}>Fit</Button
                >
                <Button
                  variant={imageSizing === "actual" && imageScale === 1
                    ? "secondary"
                    : "ghost"}
                  size="sm"
                  onclick={() => {
                    imageSizing = "actual";
                    imageScale = 1;
                  }}>Actual size</Button
                >
                <Button
                  variant="ghost"
                  size="icon-sm"
                  aria-label="Zoom out"
                  disabled={imageScale <= 0.25}
                  onclick={() => changeScale(-0.25)}><Minus /></Button
                >
                <span class="min-w-12 text-center"
                  >{Math.round(imageScale * 100)}%</span
                >
                <Button
                  variant="ghost"
                  size="icon-sm"
                  aria-label="Zoom in"
                  disabled={imageScale >= 4}
                  onclick={() => changeScale(0.25)}><Plus /></Button
                >
                <Button
                  variant={imageBackground === "checker"
                    ? "secondary"
                    : "ghost"}
                  size="sm"
                  onclick={() => (imageBackground = "checker")}>Checker</Button
                >
                <Button
                  variant={imageBackground === "light" ? "secondary" : "ghost"}
                  size="sm"
                  onclick={() => (imageBackground = "light")}>Light</Button
                >
                <Button
                  variant={imageBackground === "dark" ? "secondary" : "ghost"}
                  size="sm"
                  onclick={() => (imageBackground = "dark")}>Dark</Button
                >
              </div>
            {/if}
          </div>
          {#if pageState.browser.blob.image_error}
            <div
              class="grid min-h-80 place-items-center rounded-md border p-8 text-center text-sm text-muted-foreground"
            >
              <div>
                <p>Image preview unavailable.</p>
                <p class="mt-1 text-xs">{pageState.browser.blob.image_error}</p>
                <Button
                  class="mt-4"
                  variant="outline"
                  onclick={downloadOriginal}
                  ><Download data-icon="inline-start" />Download original</Button
                >
              </div>
            </div>
          {:else if imageView === "source"}
            <div class="rounded-md border bg-muted/20 p-4">
              {#if sourceLoading}
                <p class="text-sm text-muted-foreground">Loading SVG source…</p>
              {:else if sourceError}
                <p class="text-sm text-destructive">{sourceError}</p>
              {:else}
                <pre
                  class="whitespace-pre-wrap break-words font-mono text-xs leading-5">{sourceText ||
                    pageState.browser.blob.content ||
                    "SVG source is unavailable."}</pre>
              {/if}
            </div>
          {:else}
            <div
              class={`relative grid min-h-80 place-items-center overflow-auto rounded-md border p-4 ${backgroundClass}`}
            >
              {#key imageUrl}
                <img
                  src={imageUrl}
                  alt={`Preview of ${pageState.browser.blob.path}`}
                  class="block"
                  style={imageStyle}
                  onerror={() =>
                    (imageLoadError = "This image could not be rendered.")}
                />
              {/key}
              {#if imageLoadError}
                <div
                  class="absolute grid place-items-center p-8 text-center text-sm text-muted-foreground"
                >
                  <p>{imageLoadError}</p>
                  <Button
                    class="mt-3"
                    variant="outline"
                    onclick={downloadOriginal}>Download original</Button
                  >
                </div>
              {/if}
            </div>
          {/if}
        </div>
      {:else if pageState.browser.blob.image_error}
        <div
          class="grid min-h-80 place-items-center p-8 text-center text-sm text-muted-foreground"
        >
          <div>
            <p>Image preview unavailable.</p>
            <p class="mt-1 text-xs">{pageState.browser.blob.image_error}</p>
            <Button class="mt-4" variant="outline" onclick={downloadOriginal}
              ><Download data-icon="inline-start" />Download original</Button
            >
          </div>
        </div>
      {:else if pageState.browser.blob.binary}
        <div
          class="grid min-h-80 place-items-center p-8 text-sm text-muted-foreground"
        >
          Binary files cannot be previewed.
        </div>
      {:else if pageState.browser.blob.too_large}
        <div
          class="grid min-h-80 place-items-center p-8 text-center text-sm text-muted-foreground"
        >
          <div>
            <p>This file is too large to render inline.</p>
            <Button class="mt-4" variant="outline" onclick={downloadOriginal}
              ><Download data-icon="inline-start" />Download raw file</Button
            >
          </div>
        </div>
      {:else if pageState.browser.blob.rendered_html}
        <div
          class="prose max-w-none p-6 prose-img:my-0 prose-img:inline-block prose-code:before:content-none prose-code:after:content-none dark:prose-invert lg:p-8"
          {@attach trustedHtml(pageState.browser.blob.rendered_html, {
            namespace: pageState.namespace,
            name: pageState.name,
            revision: pageState.browser.blob.revision,
            commit_oid: pageState.browser.blob.commit_oid,
            path: pageState.browser.blob.path,
          })}
        ></div>
      {:else}
        <pre
          class={pageState.browser.wrapLines
            ? "whitespace-pre-wrap break-words bg-background/35 p-5 font-mono text-xs leading-5"
            : "w-max min-w-full whitespace-pre bg-background/35 p-5 font-mono text-xs leading-5"}><code
            {@attach trustedHtml(pageState.browser.highlighted)}></code></pre>
      {/if}
    </div>
  {:else if pageState.browser.readme?.rendered_html}
    <header
      class="flex min-h-12 shrink-0 items-center gap-2 border-b px-5 py-2 text-sm font-semibold"
    >
      <Braces class="size-4 text-muted-foreground" />{pageState.browser.readme
        .path}
    </header>
    <div class="min-h-0 flex-1 xl:overflow-y-auto xl:overscroll-contain">
      <div
        class="prose max-w-none p-6 prose-img:my-0 prose-img:inline-block prose-code:before:content-none prose-code:after:content-none dark:prose-invert lg:p-8"
        {@attach trustedHtml(pageState.browser.readme.rendered_html, {
          namespace: pageState.namespace,
          name: pageState.name,
          revision: pageState.browser.readme.revision,
          commit_oid: pageState.browser.readme.commit_oid,
          path: pageState.browser.readme.path,
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
