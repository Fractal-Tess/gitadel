<script lang="ts">
  import { onMount } from "svelte";

  const MAX_RICH_DIFF_BYTES = 256 * 1024;
  const MAX_RICH_DIFF_FILES = 80;
  const MAX_RICH_DIFF_LINES = 6_000;
  const MAX_PLAIN_DIFF_BYTES = 1024 * 1024;

  let { patch, cacheKey }: { patch: string; cacheKey: string } = $props();
  let container = $state<HTMLDivElement>();
  let rendering = $state(true);
  const renderRichDiff = $derived(canRenderRichDiff(patch));
  const plainPatch = $derived(
    patch.length > MAX_PLAIN_DIFF_BYTES
      ? patch.slice(0, MAX_PLAIN_DIFF_BYTES)
      : patch,
  );

  function canRenderRichDiff(value: string) {
    if (value.length > MAX_RICH_DIFF_BYTES) return false;
    let files = 0;
    let lines = 1;
    for (let index = 0; index < value.length; index += 1) {
      if (value.charCodeAt(index) !== 10) continue;
      lines += 1;
      if (lines > MAX_RICH_DIFF_LINES) return false;
      if (value.startsWith("diff --git ", index + 1)) {
        files += 1;
        if (files > MAX_RICH_DIFF_FILES) return false;
      }
    }
    return true;
  }

  function nextFrame() {
    return new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
  }

  onMount(() => {
    if (!renderRichDiff) {
      rendering = false;
      return;
    }

    let cancelled = false;
    const instances: Array<{ cleanUp(): void }> = [];
    void (async () => {
      const [{ FileDiff, processPatch }] = await Promise.all([
        import("@pierre/diffs"),
        import("@pierre/diffs/web-components"),
      ]);
      await nextFrame();
      if (!container || cancelled) return;

      const files = processPatch(patch, cacheKey).files;
      for (const [index, file] of files.entries()) {
        if (cancelled) return;
        const host = document.createElement("diffs-container");
        host.className = "overflow-hidden rounded-md border bg-card/25";
        container.append(host);
        const instance = new FileDiff({
          diffStyle: "split",
          overflow: "scroll",
          lineDiffType: "word",
          themeType: "system",
        });
        instance.render({ fileContainer: host, fileDiff: file });
        instances.push(instance);
        if ((index + 1) % 8 === 0) await nextFrame();
      }
      rendering = false;
    })().catch(() => {
      rendering = false;
    });

    return () => {
      cancelled = true;
      instances.forEach((instance) => instance.cleanUp());
    };
  });
</script>

{#if renderRichDiff}
  {#if rendering}
    <p class="rounded-md border bg-card/25 px-4 py-3 text-sm text-muted-foreground" role="status">
      Rendering changes…
    </p>
  {/if}
  <div
    bind:this={container}
    class={["grid gap-4", rendering && "hidden"]}
    aria-label="Side-by-side file changes"
    aria-busy={rendering}
  ></div>
{:else}
  <div class="overflow-hidden rounded-md border bg-card/25">
    <p class="border-b px-4 py-3 text-xs text-muted-foreground">
      Large diff shown in the faster unified view.
      {#if patch.length > MAX_PLAIN_DIFF_BYTES}
        Only the first 1 MB is displayed.
      {/if}
    </p>
    <pre class="max-h-[70svh] overflow-auto p-4 font-mono text-xs leading-5"><code>{plainPatch}</code></pre>
  </div>
{/if}
