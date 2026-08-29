<script lang="ts">
  import { goto } from "$app/navigation";
  import { resolve } from "$app/paths";
  import { page } from "$app/state";

  $effect(() => {
    const view =
      page.url.searchParams.get("view") === "applications"
        ? "oauth-applications"
        : "profile";
    void goto(resolve("/-/account/[view]", { view }), {
      replaceState: true,
    });
  });
</script>

<p class="grid min-h-full place-items-center text-sm text-muted-foreground">
  Opening account settings…
</p>
