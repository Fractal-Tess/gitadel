<script lang="ts">
  import { onMount } from "svelte";

  let { patch, cacheKey }: { patch: string; cacheKey: string } = $props();
  let container = $state<HTMLDivElement>();

  onMount(() => {
    const instances: Array<{ cleanUp(): void }> = [];

    void Promise.all([
      import("@pierre/diffs"),
      import("@pierre/diffs/web-components"),
    ]).then(([{ FileDiff, processPatch }]) => {
      if (!container) return;
      const files = processPatch(patch, cacheKey).files;
      for (const file of files) {
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
      }
    });

    return () => instances.forEach((instance) => instance.cleanUp());
  });
</script>

<div bind:this={container} class="grid gap-4" aria-label="Side-by-side file changes"></div>
