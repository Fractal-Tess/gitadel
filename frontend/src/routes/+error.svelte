<script lang="ts">
  import { page } from "$app/state";

  import NotFound7 from "$lib/components/blocks/not-found-7.svelte";
</script>

<svelte:head>
  <title>{page.status === 404 ? "404" : "Error"} · Gitadel</title>
  <meta
    name="description"
    content={page.status === 404
      ? "The requested page could not be found."
      : "Gitadel could not open the requested page."}
  />
</svelte:head>

<NotFound7
  title={page.status === 404 ? "Nothing on the scope." : "Signal interrupted."}
  description={page.status === 404
    ? "We swept every route twice and could not find what you requested. Search the repositories, or head back to base."
    : "Gitadel could not open this page. Search the repositories, or head back to base and try another route."}
/>
