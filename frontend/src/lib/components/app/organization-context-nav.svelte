<script lang="ts">
  import { page } from "$app/state";

  import OrganizationNav, {
    type OrganizationView,
  } from "$lib/components/settings/organization-nav.svelte";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const app = useAppState();
  const slug = $derived(
    page.params.namespace && !page.params.name ? page.params.namespace : "",
  );
  const organization = $derived(
    app.organizations.find((candidate) => candidate.slug === slug) ?? null,
  );
  const active = $derived.by<OrganizationView>(() => {
    const view = page.url.pathname.split("/").at(-1);
    return view === "members" ||
      view === "runners" ||
      view === "integrations" ||
      view === "mirror-credentials" ||
      view === "settings"
      ? view
      : "repositories";
  });
</script>

<!-- Personal namespaces get no tab bar: their runners, integrations, and
     mirror identities are reached from the account menu instead. -->
{#if organization}
  <OrganizationNav
    {slug}
    label={organization.display_name || organization.slug}
    {active}
    canManage={organization.role === "owner"}
  />
{/if}
