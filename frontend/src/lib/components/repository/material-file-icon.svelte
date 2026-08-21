<script lang="ts">
  import Icon from "@iconify/svelte";

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

<Icon {icon} class={className} aria-hidden="true" />
