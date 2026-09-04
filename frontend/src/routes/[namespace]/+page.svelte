<script lang="ts">
  import { page } from "$app/state";
  import { resolve } from "$app/paths";
  import { z } from "zod";

  import NotFound7 from "$lib/components/blocks/not-found-7.svelte";
  import RepositoryList from "$lib/components/repository/repository-list.svelte";
  import * as Alert from "$lib/components/ui/alert/index.js";
  import { ApiFailure, requestJson } from "$lib/api/transport.js";
  import { preloadNamespaceTabs } from "$lib/namespace-preload.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const namespaceSchema = z.object({
    slug: z.string(),
    kind: z.enum(["user", "organization"]),
  });

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
  let namespaceStatus = $state<"loading" | "ready" | "not-found" | "error">(
    "loading",
  );
  let validationSequence = 0;

  $effect(() => {
    const currentNamespace = namespace;
    const sequence = ++validationSequence;
    namespaceStatus = "loading";
    void requestJson(
      `/api/v1/namespaces/${encodeURIComponent(currentNamespace)}`,
      namespaceSchema,
    ).then(
      () => {
        if (sequence === validationSequence) namespaceStatus = "ready";
      },
      (caught) => {
        if (sequence !== validationSequence) return;
        namespaceStatus =
          caught instanceof ApiFailure && caught.status === 404
            ? "not-found"
            : "error";
      },
    );
  });

  $effect(() => {
    if (
      namespaceStatus !== "ready" ||
      !app.authStatus?.authenticated ||
      !namespace
    )
      return;
    preloadNamespaceTabs(namespace, app.authorizationScope, {
      members: Boolean(organization),
      management: personal || organization?.role === "owner",
    });
  });
</script>

<svelte:head>
  <title>{namespaceStatus === "not-found" ? "404" : namespace} · Gitadel</title>
</svelte:head>

{#if namespaceStatus === "loading"}
  <div class="mx-auto max-w-xl px-5 py-16 text-center">
    <p class="text-sm text-muted-foreground">Opening namespace…</p>
  </div>
{:else if namespaceStatus === "not-found"}
  <NotFound7
    title="Namespace off the scope."
    description="This namespace does not exist, or it is no longer available. Search the repositories, or head back to base."
  />
{:else if namespaceStatus === "error"}
  <Alert.Root class="mx-auto mt-16 max-w-lg" variant="destructive">
    <Alert.Title>Namespace unavailable</Alert.Title>
    <Alert.Description>
      Gitadel could not verify this namespace. Reload the page to try again.
    </Alert.Description>
  </Alert.Root>
{:else}
  {#key namespace}
    <RepositoryList {namespace} {manageHref} />
  {/key}
{/if}
