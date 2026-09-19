<script lang="ts">
  import Copy from "@lucide/svelte/icons/copy";

  import { toast } from "svelte-sonner";

  import { copyText } from "$lib/clipboard.js";
  import MaterialFileIcon from "$lib/components/repository/material-file-icon.svelte";
  import { Badge } from "$lib/components/ui/badge/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Card from "$lib/components/ui/card/index.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let { state: pageState }: { state: RepositoryPageState } = $props();
  const entry = $derived(pageState.browser.blob);
  const actualBytes = $derived(BigInt(entry?.lfs?.size ?? "0"));
  const humanSize = $derived(formatLfsSize(actualBytes));
  const exactSize = $derived(`${actualBytes.toLocaleString()} bytes`);
  let exactBytes = $state(false);
  let rawPointerVisible = $state(false);

  const sizeUnits = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"] as const;

  function formatLfsSize(bytes: bigint): string {
    if (bytes === 0n) return "0 B";

    let unitIndex = 0;
    let factor = 1n;
    while (unitIndex < sizeUnits.length - 1 && bytes >= factor * 1024n) {
      factor *= 1024n;
      unitIndex += 1;
    }
    if (unitIndex === 0) return `${bytes.toLocaleString()} B`;

    const hundredths = (bytes * 100n + factor / 2n) / factor;
    return `${(Number(hundredths) / 100).toLocaleString(undefined, {
      maximumFractionDigits: 2,
    })} ${sizeUnits[unitIndex]}`;
  }

  async function copy(value: string, label: string): Promise<void> {
    try {
      await copyText(value);
      toast.success(`${label} copied`);
    } catch {
      toast.error(`Could not copy ${label.toLowerCase()}.`);
    }
  }
</script>

{#if entry?.lfs}
  <header
    class="flex min-h-12 shrink-0 flex-wrap items-center justify-between gap-3 border-b px-5 py-2 text-sm font-semibold"
  >
    <span class="flex min-w-0 items-center gap-2">
      <MaterialFileIcon name={entry.path} class="size-4 shrink-0" />
      <span class="truncate" title={entry.path}>{entry.path}</span>
    </span>
    <Badge variant="secondary" class="shrink-0">Git LFS</Badge>
  </header>
  <div class="min-h-0 flex-1 overflow-y-auto p-5 xl:overscroll-contain lg:p-8">
    <div class="mx-auto flex max-w-3xl flex-col gap-6">
      <Card.Root>
        <Card.Header>
          <Card.Title>Large file stored with Git LFS</Card.Title>
          <Card.Description>
            Git records a small pointer for this file. Its contents are stored
            separately with Git LFS.
          </Card.Description>
        </Card.Header>
        <Card.Content class="flex flex-col gap-5">
          <dl class="flex flex-col gap-5 text-sm">
            <div class="flex min-w-0 flex-col gap-1.5">
              <dt class="text-muted-foreground">Actual object size</dt>
              <dd class="flex flex-wrap items-center gap-2">
                <span class="font-medium tabular-nums"
                  >{exactBytes ? exactSize : humanSize}</span
                >
                <Button
                  variant="ghost"
                  size="sm"
                  onclick={() => (exactBytes = !exactBytes)}
                  >{exactBytes ? "Show human size" : "Show exact bytes"}</Button
                >
              </dd>
            </div>
            <div class="flex min-w-0 flex-col gap-1.5">
              <dt class="text-muted-foreground">SHA-256 object ID</dt>
              <dd class="flex min-w-0 items-start gap-2">
                <code class="min-w-0 break-all text-xs leading-6"
                  >{entry.lfs.oid}</code
                >
                <Button
                  variant="ghost"
                  size="icon-sm"
                  class="shrink-0"
                  aria-label="Copy SHA-256 object ID"
                  onclick={() => copy(entry.lfs!.oid, "SHA-256 object ID")}
                  ><Copy /></Button
                >
              </dd>
            </div>
          </dl>
        </Card.Content>
        <Card.Footer class="flex-wrap gap-2">
          <Button
            variant="ghost"
            aria-pressed={rawPointerVisible}
            onclick={() => (rawPointerVisible = !rawPointerVisible)}
            >{rawPointerVisible ? "Hide pointer" : "View pointer"}</Button
          >
        </Card.Footer>
      </Card.Root>

      {#if rawPointerVisible}
        <section class="flex flex-col gap-3" aria-label="Raw LFS pointer">
          <h3 class="text-sm font-semibold">Raw pointer</h3>
          <pre
            class="overflow-x-auto whitespace-pre-wrap break-words rounded-md border bg-muted/20 p-4 font-mono text-xs leading-5">{entry.content ??
              "Pointer content is unavailable."}</pre>
        </section>
      {/if}
    </div>
  </div>
{/if}
