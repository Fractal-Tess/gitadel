<script lang="ts">
  import { page } from "$app/state";
  import { resolve } from "$app/paths";

  import RepositoryList from "$lib/components/repository/repository-list.svelte";
  import { preloadNamespaceTabs } from "$lib/namespace-preload.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const app = useAppState();
  const namespace = $derived(page.params.namespace ?? "");
  const personal = $derived(namespace === app.authStatus?.user?.username);
  const organization = $derived(
    app.organizations.find((candidate) => candidate.slug === namespace) ?? null,
  );
  const canManage = $derived(personal || organization?.role === "owner");
  const manageHref = $derived(
    canManage
      ? organization
        ? resolve("/[namespace]/members", { namespace })
        : resolve("/[namespace]/runners", { namespace })
      : null,
  );

  $effect(() => {
    if (!app.authStatus?.authenticated || !namespace) return;
    preloadNamespaceTabs(namespace, app.authorizationScope, {
      members: Boolean(organization),
      management: personal || organization?.role === "owner",
    });
  });
</script>

{#key namespace}
  <RepositoryList {namespace} {manageHref} />
{/key}
