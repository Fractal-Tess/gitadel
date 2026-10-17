<script lang="ts">
  import CircleCheck from "@lucide/svelte/icons/circle-check";
  import TriangleAlert from "@lucide/svelte/icons/triangle-alert";

  import * as Alert from "$lib/components/ui/alert/index.js";

  let { result }: { result: { steps: string[]; warning: string | null } } =
    $props();
  // Several deployments can be listed on one page.
  const uid = $props.id();
  const headingId = `${uid}-setup-result-heading`;
</script>

<section
  class="rounded-xl border bg-card p-5 shadow-sm"
  aria-labelledby={headingId}
>
  <h2 id={headingId} class="font-semibold">Deployment set up</h2>
  <ul class="mt-3 grid gap-1.5" aria-label="Completed steps">
    {#each result.steps as step, index (index)}
      <li class="flex items-start gap-2 text-sm">
        <CircleCheck
          class="mt-0.5 size-4 shrink-0 text-emerald-500"
          aria-hidden="true"
        />
        <span><span class="sr-only">Done: </span>{step}</span>
      </li>
    {/each}
  </ul>
  {#if result.warning}
    <Alert.Root class="mt-4 border-amber-500/40 bg-amber-500/5">
      <TriangleAlert class="size-4 shrink-0 text-amber-500" />
      <Alert.Title>One step did not finish</Alert.Title>
      <Alert.Description>{result.warning}</Alert.Description>
    </Alert.Root>
  {/if}
</section>
