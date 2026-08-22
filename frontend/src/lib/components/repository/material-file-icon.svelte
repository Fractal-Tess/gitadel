<script lang="ts">
  import { File, Folder } from "lucide-svelte";

  import {
    materialFileIcon,
    materialFolderIcon,
  } from "$lib/repository/material-file-icons.js";

  let {
    name,
    directory = false,
    expanded = false,
    class: className = "",
  }: {
    name: string;
    directory?: boolean;
    expanded?: boolean;
    class?: string;
  } = $props();

  const icon = $derived(
    directory ? materialFolderIcon(name, expanded) : materialFileIcon(name),
  );
  const FallbackIcon = $derived(directory ? Folder : File);
</script>

{#await icon}
  <FallbackIcon
    class={`${className} text-muted-foreground/70`}
    aria-hidden="true"
  />
{:then source}
  {#if source}
    <img
      src={source}
      alt=""
      draggable={false}
      decoding="async"
      class={className}
      aria-hidden="true"
    />
  {:else}
    <FallbackIcon
      class={`${className} text-muted-foreground/70`}
      aria-hidden="true"
    />
  {/if}
{:catch}
  <FallbackIcon
    class={`${className} text-muted-foreground/70`}
    aria-hidden="true"
  />
{/await}
