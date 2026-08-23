<script lang="ts">
  import { goto } from "$app/navigation";
  import { resolve } from "$app/paths";
  import { onMount } from "svelte";

  onMount(() => {
    void goto(resolve("/settings?view=administration"), {
      replaceState: true,
    });
  });
</script>

<svelte:head>
  <title>Administration</title>
</svelte:head>

<div class="py-16 text-center text-sm text-muted-foreground">
  Redirecting to account settings…
</div>
