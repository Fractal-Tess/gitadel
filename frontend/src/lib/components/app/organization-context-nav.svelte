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
  const personal = $derived(slug === app.authStatus?.user?.username);
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

{#if organization}
  <OrganizationNav
    {slug}
    label={organization.display_name || organization.slug}
    {active}
    canManage={organization.role === "owner"}
  />
{:else if personal}
  <OrganizationNav
    {slug}
    label={slug}
    {active}
    showMembers={false}
    canManage
    scope="personal"
  />
{/if}
