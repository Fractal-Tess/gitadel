<script lang="ts">
  import { mode } from "mode-watcher";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  interface Props {
    /** Forces the mark drawn for one theme, e.g. on an always-dark surface. */
    theme?: "light" | "dark";
    class?: string;
  }

  let { theme, class: className = "size-6" }: Props = $props();

  const app = useAppState();
  let version = $derived(
    encodeURIComponent(app.instance?.updated_at ?? "default"),
  );
</script>

<span class={["block shrink-0", className]} aria-hidden="true">
  <img
    class="size-full"
    src={`/api/v1/instance/favicon/${theme ?? mode.current ?? "light"}?v=${version}&r=2`}
    alt=""
  />
</span>
