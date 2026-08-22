<script lang="ts">
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const app = useAppState();
  let version = $derived(
    encodeURIComponent(app.instance?.updated_at ?? "default"),
  );
</script>

<span class="block size-6 shrink-0" aria-hidden="true">
  <img
    class="size-full"
    src={`/api/v1/instance/favicon/dark?v=${version}&r=2`}
    alt=""
  />
</span>
