<script lang="ts">
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
</script>

{#await icon}
  <span class={["inline-block", className]} aria-hidden="true"></span>
{:then source}
  {#if source}
    <img
      src={source}
      alt=""
      draggable={false}
      class={className}
      aria-hidden="true"
    />
  {:else}
    <span class={["inline-block", className]} aria-hidden="true"></span>
  {/if}
{/await}
